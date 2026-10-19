import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('package_check', ROOT / 'scripts/check-package.py')
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class AudioPackageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / 'licenses').mkdir()
        for name in ['licenses/COPYING.LGPLv2.1', 'licenses/LICENSE.md', 'SOURCE.txt']:
            (self.root / name).write_text('notice')
        lock = json.loads((ROOT / 'scripts/audio-backend.lock.json').read_text())
        for dependency in lock['dependencies']:
            for name in dependency['licenses']:
                (self.root / 'licenses' / f'{dependency["name"]}-{name}').write_text('notice')
        self.manifest = {'source': lock['source'], 'dependencies': lock['dependencies'], 'system': 'linux', 'binaries': {}}
        for name in ['ffmpeg', 'ffprobe']:
            content = (name + ' executable fixture').encode()
            (self.root / name).write_bytes(content)
            self.manifest['binaries'][name] = {'sha256': hashlib.sha256(content).hexdigest()}
        self.save()

    def save(self):
        (self.root / 'build-manifest.json').write_text(json.dumps(self.manifest))

    def test_detects_tampered_inspector_even_when_worker_is_unchanged(self):
        CHECK.check_audio_manifest(self.root)
        (self.root / 'ffprobe').write_text('substituted inspector')
        with self.assertRaisesRegex(SystemExit, 'ffprobe'):
            CHECK.check_audio_manifest(self.root)

    def test_detects_missing_binary_inventory(self):
        del self.manifest['binaries']['ffprobe']
        self.save()
        with self.assertRaisesRegex(SystemExit, 'inventory'):
            CHECK.check_audio_manifest(self.root)

    def test_detects_wrong_source(self):
        self.manifest['source']['sha256'] = '0' * 64
        self.save()
        with self.assertRaisesRegex(SystemExit, 'source lock'):
            CHECK.check_audio_manifest(self.root)

    def test_requires_source_and_license_notices(self):
        (self.root / 'SOURCE.txt').unlink()
        with self.assertRaisesRegex(SystemExit, 'notice'):
            CHECK.check_audio_manifest(self.root)

    def test_detects_wrong_encoder_dependency(self):
        self.manifest['dependencies'][0]['sha256'] = '0' * 64
        self.save()
        with self.assertRaisesRegex(SystemExit, 'dependencies'):
            CHECK.check_audio_manifest(self.root)

    def test_requires_encoder_license_notice(self):
        (self.root / 'licenses/lame-COPYING').unlink()
        with self.assertRaisesRegex(SystemExit, 'dependency license'):
            CHECK.check_audio_manifest(self.root)
