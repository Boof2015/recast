"""Regression checks for pinned media-worker build adaptations."""
import importlib.util
import os
from pathlib import Path
import platform
import shlex
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'audio_builder', Path(__file__).resolve().parents[1] / 'prepare-audio-backend.py'
)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


class MesonToolchainTests(unittest.TestCase):
    def test_native_static_library_build_install_and_execution_with_spaced_paths(self):
        # Runs before the long codec builds in CI. On Windows this exercises the
        # actual MSYS orchestrator -> UCRT Python/Meson/Ninja/compiler boundary.
        system = 'windows' if os.environ.get('MSYSTEM') else platform.system().lower()
        arch = {'amd64': 'x86_64', 'aarch64': 'arm64'}.get(platform.machine().lower(), platform.machine().lower())
        with tempfile.TemporaryDirectory(prefix='recast meson test ') as temporary:
            work = Path(temporary)
            source, build, stage = (work / name for name in ['source dir', 'build dir', 'install dir'])
            source.mkdir()
            (source / 'meson.build').write_text(f'''project('recast-meson-check', 'c')
assert(host_machine.system() == '{system}', 'Incorrect target OS')
lib = library('recast_probe', 'probe.c', install: true)
import('pkgconfig').generate(lib, name: 'recast-probe', filebase: 'recast-probe',
  description: 'Recast build toolchain check', version: '1.0')
install_headers('probe.h')
executable('recast_probe_check', 'main.c', link_with: lib, install: true)
''')
            (source / 'probe.h').write_text('int recast_probe(void);\n')
            (source / 'probe.c').write_text('#include "probe.h"\nint recast_probe(void) { return 42; }\n')
            (source / 'main.c').write_text('#include "probe.h"\nint main(void) { return recast_probe() != 42; }\n')
            env = os.environ | {'PKG_CONFIG_LIBDIR': str(stage / 'lib/pkgconfig')}
            command, machine_config, env = builder.meson_toolchain(work, env, system, arch)
            args = [*command, 'setup', builder.meson_path(build, system), builder.meson_path(source, system),
                    f'--prefix={builder.meson_path(stage, system)}', '--libdir=lib',
                    '--default-library=static', '--wrap-mode=nodownload', *machine_config]
            for step in [args,
                         [*command, 'compile', '-C', builder.meson_path(build, system), '-j', '2'],
                         [*command, 'install', '-C', builder.meson_path(build, system)]]:
                result = subprocess.run(step, env=env, capture_output=True, text=True, timeout=90)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((stage / 'lib/librecast_probe.a').is_file())
            self.assertTrue((stage / 'include/probe.h').is_file())
            executable = stage / 'bin' / ('recast_probe_check.exe' if system == 'windows' else 'recast_probe_check')
            subprocess.run([str(executable)], check=True, timeout=10)

            # The surrounding FFmpeg build consumes the native Meson install
            # from MSYS. Check that the .pc paths link correctly across it too.
            flags = subprocess.check_output(['pkg-config', '--static', '--cflags', '--libs', 'recast-probe'],
                                            env=env, text=True, timeout=10)
            compiler = '/ucrt64/bin/gcc' if system == 'windows' else 'cc'
            linked = work / ('linked.exe' if system == 'windows' else 'linked')
            result = subprocess.run([compiler, str(source / 'main.c'), *shlex.split(flags), '-o', str(linked)],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            subprocess.run([str(linked)], check=True, timeout=10)


# Compatibility block from oneVPL 2.16.0's mfx_dispatcher_defs.h.
LEGACY_STRINGS = r'''#if _MSC_VER < 1400
    #define wcscpy_s(to, to_size, from) \
        (void)(to_size);                \
        wcscpy(to, from)
    #define wcscat_s(to, to_size, from) \
        (void)(to_size);                \
        wcscat(to, from)
#endif
'''


class OneVplCompilerTests(unittest.TestCase):
    def test_mingw_secure_string_calls_remain_valid_expressions(self):
        with tempfile.TemporaryDirectory(prefix='recast-vpl-test-') as temporary:
            source = Path(temporary)
            header = source / 'libvpl/src/windows/mfx_dispatcher_defs.h'
            header.parent.mkdir(parents=True)
            header.write_text(LEGACY_STRINGS)
            consumer = source / 'consumer.cpp'
            consumer.write_text('''#include <wchar.h>
#include "libvpl/src/windows/mfx_dispatcher_defs.h"
#ifdef _WIN32
#include <windows.h>
#else
// Let non-Windows CI exercise the same expression context as stralign.h.
extern "C" int (wcscpy_s)(wchar_t *, size_t, const wchar_t *);
extern "C" int (wcscat_s)(wchar_t *, size_t, const wchar_t *);
#endif
wchar_t *copy(wchar_t *destination, size_t size, const wchar_t *source) {
    return (wcscpy_s(destination, size, source) == 0 ? destination : nullptr);
}
wchar_t *append(wchar_t *destination, size_t size, const wchar_t *source) {
    return (wcscat_s(destination, size, source) == 0 ? destination : nullptr);
}
''')
            compiler = '/ucrt64/bin/g++' if os.environ.get('MSYSTEM') else 'c++'
            command = [compiler, '-std=c++11', '-c', str(consumer), '-o', str(source / 'consumer.o')]
            original = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(original.returncode, 0, 'Fixture must reproduce the header collision')
            self.assertIn('wcscpy_s', original.stderr)

            builder.patch_vpl_mingw_string_functions(source)
            corrected = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(corrected.returncode, 0, corrected.stdout + corrected.stderr)
            patched = header.read_bytes()
            builder.patch_vpl_mingw_string_functions(source)
            self.assertEqual(header.read_bytes(), patched)

    def test_unrecognized_upstream_guard_requires_review(self):
        with tempfile.TemporaryDirectory(prefix='recast-vpl-test-') as temporary:
            source = Path(temporary)
            header = source / 'libvpl/src/windows/mfx_dispatcher_defs.h'
            header.parent.mkdir(parents=True)
            header.write_text(LEGACY_STRINGS.replace('_MSC_VER < 1400', '_MSC_VER < 1500'))
            with self.assertRaisesRegex(SystemExit, 'oneVPL MinGW patch no longer matches'):
                builder.patch_vpl_mingw_string_functions(source)
