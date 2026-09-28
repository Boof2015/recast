"""Exercise source-download integrity from an empty cache, without network access."""
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


if __name__ == '__main__':
    unittest.main()
