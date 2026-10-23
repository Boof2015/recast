"""Exercise source integrity, compiler isolation and platform dependency audits."""
import hashlib
import http.client
import importlib.util
import os
from pathlib import Path
import platform
import shlex
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import URLError

SPEC = importlib.util.spec_from_file_location(
    'image_builder', Path(__file__).resolve().parents[1] / 'prepare-image-backend.py'
)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


class CompilerIsolationTests(unittest.TestCase):
    def test_pinned_imported_headers_take_precedence_over_host_headers(self):
        # Apple Clang adds -I/usr/local/include on Intel Macs. A competing
        # regular include must not outrank our pinned imported CMake targets.
        with tempfile.TemporaryDirectory(prefix='recast-header-test-') as temporary:
            root = Path(temporary)
            stage = root / 'install'
            pinned = stage / 'include'
            host = root / 'host-include'
            pinned.mkdir(parents=True)
            host.mkdir()
            (pinned / 'codec.h').write_text('#define CODEC_ABI 62\n')
            (host / 'codec.h').write_text('#error "Picked up the incompatible host codec header"\n')
            (root / 'consumer.c').write_text('#include <codec.h>\n_Static_assert(CODEC_ABI == 62, "Wrong codec ABI");\n')
            (root / 'CMakeLists.txt').write_text('''cmake_minimum_required(VERSION 3.16)
project(HeaderIsolation C)
add_library(PinnedCodec INTERFACE IMPORTED)
set_target_properties(PinnedCodec PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_INSTALL_PREFIX}/include")
add_library(consumer OBJECT consumer.c)
target_link_libraries(consumer PRIVATE PinnedCodec)
''')
            system = 'windows' if os.environ.get('MSYSTEM') else platform.system().lower()
            arch = {'aarch64': 'arm64', 'amd64': 'x86_64'}.get(platform.machine().lower(), platform.machine().lower())
            env = os.environ.copy()
            env['CC'] = '/ucrt64/bin/gcc' if system == 'windows' else 'cc'
            env['CFLAGS'] = f'-I{shlex.quote(str(host))}'
            build = root / 'build'
            commands = [
                ['cmake', '-S', str(root), '-B', str(build), *builder.cmake_arguments(stage, system, arch)],
                ['cmake', '--build', str(build)],
            ]
            for command in commands:
                result = subprocess.run(command, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


class SourceDownloadTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='recast-download-test-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.payload = b'pinned source archive contents\x00\xff'
        upstream = self.root / 'upstream.tar.gz'
        upstream.write_bytes(self.payload)
        self.source = {
            'archive': 'source.tar.gz',
            'url': upstream.as_uri(),
            'sha256': hashlib.sha256(self.payload).hexdigest(),
        }
        cache = patch.object(builder, 'CACHE', self.root / 'cache')
        cache.start()
        self.addCleanup(cache.stop)
        sleep = patch.object(builder.time, 'sleep')
        sleep.start()
        self.addCleanup(sleep.stop)
        self.downloads = builder.CACHE / 'downloads'

    def test_empty_cache_downloads_verifies_and_publishes_source(self):
        result = builder.download(self.source)
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(list(self.downloads.iterdir()), [result])
        with patch.object(builder, 'retrieve_source', side_effect=AssertionError('cache should avoid download')):
            self.assertEqual(builder.download(self.source), result)

    def test_checksum_mismatch_never_publishes_source_or_leaves_partial(self):
        self.source['sha256'] = '0' * 64
        with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
            builder.download(self.source)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_interrupted_download_cleans_up_partial_archive(self):
        def interrupt(_url, destination):
            Path(destination).write_bytes(b'incomplete download')
            raise URLError('connection interrupted')

        with patch.object(builder, 'retrieve_source', side_effect=interrupt) as retrieve:
            with self.assertRaisesRegex(SystemExit, 'after 3 attempts.*connection interrupted'):
                builder.download(self.source)
            self.assertEqual(retrieve.call_count, 3)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_corrupt_cached_source_is_rejected_before_build(self):
        self.downloads.mkdir(parents=True)
        cached = self.downloads / self.source['archive']
        cached.write_bytes(b'corrupt cache')
        with patch.object(builder, 'retrieve_source', side_effect=AssertionError('do not replace corrupt cache')):
            with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
                builder.download(self.source)

    def test_bad_response_is_retried_without_accepting_or_retaining_it(self):
        attempts = []

        def retrieve(_url, destination):
            self.assertEqual(list(self.downloads.iterdir()), [destination])
            attempts.append(destination)
            destination.write_bytes(b'<html>temporary error</html>' if len(attempts) == 1 else self.payload)

        with patch.object(builder, 'retrieve_source', side_effect=retrieve):
            result = builder.download(self.source)
        self.assertEqual(len(attempts), 2)
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(list(self.downloads.iterdir()), [result])

    def test_mirror_requires_the_same_pinned_hash(self):
        self.source['mirrors'] = ['https://mirror.invalid/source.tar.gz']

        def retrieve(url, destination):
            destination.write_bytes(self.payload if url in self.source['mirrors'] else b'wrong response')

        with patch.object(builder, 'retrieve_source', side_effect=retrieve) as download:
            result = builder.download(self.source)
        self.assertEqual([call.args[0] for call in download.call_args_list],
                         [self.source['url'], *self.source['mirrors']])
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(list(self.downloads.iterdir()), [result])

    def test_bad_mirror_is_rejected_with_diagnostic_and_bounded_attempts(self):
        self.source['mirrors'] = ['https://mirror.invalid/source.tar.gz']

        def retrieve(_url, destination):
            destination.write_bytes(b'wrong')

        with patch.object(builder, 'retrieve_source', side_effect=retrieve) as download:
            with self.assertRaisesRegex(SystemExit, r'checksum mismatch: https://mirror.invalid/.*got .*\(5 bytes\)'):
                builder.download(self.source)
        self.assertEqual(download.call_count, 6)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_transport_errors_can_fall_back_to_verified_mirror(self):
        self.source['mirrors'] = ['https://mirror.invalid/source.tar.gz']
        for error in [URLError('unavailable'), TimeoutError('timed out'),
                      ConnectionResetError('reset'), http.client.IncompleteRead(b'partial', 20)]:
            with self.subTest(error=error):
                def retrieve(url, destination):
                    if url == self.source['url']:
                        destination.write_bytes(b'partial')
                        raise error
                    destination.write_bytes(self.payload)

                with patch.object(builder, 'retrieve_source', side_effect=retrieve):
                    result = builder.download(self.source)
                self.assertEqual(result.read_bytes(), self.payload)
                self.assertEqual(list(self.downloads.iterdir()), [result])
                result.unlink()


class WindowsAuditTests(unittest.TestCase):
    def test_windows_system_dlls_are_allowed(self):
        headers = '\n'.join(f'DLL Name: {name}' for name in ['KERNEL32.dll', 'urlmon.dll', 'api-ms-win-crt-runtime-l1-1-0.dll'])
        with patch.object(builder.subprocess, 'check_output', return_value=headers):
            result = builder.audit(Path('magick.exe'), 'windows', 'x86_64')
        self.assertIn('urlmon.dll', result['dynamicLibraries'])

    def test_build_environment_dlls_are_rejected(self):
        for dependency in ['libwinpthread-1.dll', 'zlib1.dll', 'libstdc++-6.dll']:
            with self.subTest(dependency=dependency):
                with patch.object(builder.subprocess, 'check_output', return_value=f'DLL Name: {dependency}'):
                    with self.assertRaisesRegex(SystemExit, 'non-system DLLs'):
                        builder.audit(Path('magick.exe'), 'windows', 'x86_64')


class LinuxAuditTests(unittest.TestCase):
    # Dependency list reported by the Ubuntu 24.04 ARM64 CI worker.
    ARM_HEADERS = '\n'.join(
        f' 0x0000000000000001 (NEEDED) Shared library: [{name}]'
        for name in ['libm.so.6', 'libc.so.6', 'ld-linux-aarch64.so.1']
    )

    def test_system_loader_is_allowed_as_a_direct_dependency(self):
        with patch.object(builder.subprocess, 'check_output', side_effect=[self.ARM_HEADERS, 'libc.so.6 => /lib/aarch64-linux-gnu/libc.so.6']):
            result = builder.audit(Path('magick'), 'linux', 'arm64')
        self.assertEqual(result['dynamicLibraries'], ['libm.so.6', 'libc.so.6', 'ld-linux-aarch64.so.1'])

    def test_loader_for_another_architecture_is_rejected(self):
        with patch.object(builder.subprocess, 'check_output', return_value=self.ARM_HEADERS):
            with self.assertRaisesRegex(SystemExit, 'unexpected shared libraries'):
                builder.audit(Path('magick'), 'linux', 'x86_64')

    def test_dynamic_codec_dependency_is_still_rejected(self):
        headers = self.ARM_HEADERS + '\n (NEEDED) Shared library: [libwebp.so.7]'
        with patch.object(builder.subprocess, 'check_output', return_value=headers):
            with self.assertRaisesRegex(SystemExit, 'unexpected shared libraries'):
                builder.audit(Path('magick'), 'linux', 'arm64')

    def test_unresolved_system_dependency_is_rejected(self):
        with patch.object(builder.subprocess, 'check_output', side_effect=[self.ARM_HEADERS, 'libm.so.6 => not found']):
            with self.assertRaisesRegex(SystemExit, 'Missing worker dependency'):
                builder.audit(Path('magick'), 'linux', 'arm64')


if __name__ == '__main__':
    unittest.main()
