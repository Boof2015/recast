"""Regression checks for pinned media-worker build adaptations."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'audio_builder', Path(__file__).resolve().parents[1] / 'prepare-audio-backend.py'
)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)

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
