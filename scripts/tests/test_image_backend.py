"""Exercise source-download integrity and platform dependency audits."""
import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import URLError

SPEC = importlib.util.spec_from_file_location(
    'image_builder', Path(__file__).resolve().parents[1] / 'prepare-image-backend.py'
)
builder = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(builder)


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
        self.downloads = builder.CACHE / 'downloads'

    def test_empty_cache_downloads_verifies_and_publishes_source(self):
        result = builder.download(self.source)
        self.assertEqual(result.read_bytes(), self.payload)
        self.assertEqual(list(self.downloads.iterdir()), [result])
        with patch.object(builder.urllib.request, 'urlretrieve', side_effect=AssertionError('cache should avoid download')):
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

        with patch.object(builder.urllib.request, 'urlretrieve', side_effect=interrupt):
            with self.assertRaises(URLError):
                builder.download(self.source)
        self.assertEqual(list(self.downloads.iterdir()), [])

    def test_corrupt_cached_source_is_rejected_before_build(self):
        self.downloads.mkdir(parents=True)
        cached = self.downloads / self.source['archive']
        cached.write_bytes(b'corrupt cache')
        with self.assertRaisesRegex(SystemExit, 'checksum mismatch'):
            builder.download(self.source)


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
