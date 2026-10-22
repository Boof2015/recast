import copy
import json
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

SHARED = Path(__file__).resolve().parents[2] / 'extensions' / 'shared'
sys.path.insert(0, str(SHARED))
import package
import draft_release


class ExtensionPackages(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = self.root / 'payload'
        (self.payload / 'licenses').mkdir(parents=True)
        (self.payload / 'licenses' / 'test.txt').write_text('Test package license\n')
        (self.payload / 'worker').write_text('not executable test data\n')
        self.metadata = {'id': 'test.extension', 'name': 'Test extension', 'version': '1.0.0', 'publisher': 'Test', 'description': 'Test package', 'platform': 'macos-arm64', 'minimumAppVersion': '0.1.0'}

    def build(self, platform='macos-arm64'):
        return package.build(dict(self.metadata, platform=platform), self.payload, self.root / 'out')

    def test_package_roundtrip_and_exclusive_output(self):
        path = self.build()
        manifest = package.verify(path)
        self.assertEqual(manifest['package']['platform'], 'macos-arm64')
        before = path.read_bytes()
        with self.assertRaises(FileExistsError):
            self.build()
        self.assertEqual(path.read_bytes(), before)

    def test_build_rejects_symlinks_and_output_inside_payload(self):
        with self.assertRaises(ValueError):
            package.build(self.metadata, self.payload, self.payload / 'out')
        if sys.platform != 'win32':
            (self.payload / 'link').symlink_to(self.payload / 'worker')
            with self.assertRaises(ValueError):
                self.build()

    def test_mutated_payload_and_extra_archive_entries_fail(self):
        path = self.build()
        with zipfile.ZipFile(path) as source:
            contents = {info.filename: source.read(info) for info in source.infolist()}
        contents['payload/worker'] = b'changed'
        bad = self.root / 'bad.recast-extension'
        with zipfile.ZipFile(bad, 'w') as target:
            for name, data in contents.items():
                target.writestr(name, data)
        with self.assertRaises(ValueError):
            package.verify(bad)
        with zipfile.ZipFile(path, 'a') as target:
            target.writestr('../escape', 'bad')
        with self.assertRaises(ValueError):
            package.verify(path)

    def test_paths_and_case_aliases(self):
        manifest = package.verify(self.build())
        for path in ['../escape', '/absolute', 'C:/file', 'a\\b', 'AUX.txt', 'COM1', 'name.', 'name ', 'a//b', 'x:stream']:
            with self.subTest(path=path):
                bad = copy.deepcopy(manifest)
                bad['files'][0]['path'] = path
                with self.assertRaises(ValueError):
                    package.validate_manifest(bad)
        bad = copy.deepcopy(manifest)
        extra = dict(bad['files'][0], path=bad['files'][0]['path'].upper())
        bad['files'].append(extra)
        bad['package']['installedBytes'] += extra['bytes']
        with self.assertRaises(ValueError):
            package.validate_manifest(bad)

    def test_draft_requires_all_platforms_and_never_publishes_or_overwrites(self):
        self.build()
        calls = []
        def run(args, **kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, 0, stdout='[[]]')
        with self.assertRaises(ValueError):
            draft_release.create_draft(self.root / 'out', 'a' * 40, run)
        self.assertEqual(calls, [])
        for platform in package.PLATFORMS - {'macos-arm64'}:
            self.build(platform)
        draft_release.create_draft(self.root / 'out', 'a' * 40, run)
        command = calls[-1]
        self.assertIn('--draft', command)
        self.assertIn('--latest=false', command)
        self.assertEqual(command[command.index('--repo') + 1], 'Boof2015/recast-extensions')
        self.assertNotIn('--clobber', command)
        index = json.loads((self.root / 'out' / 'extension-release.json').read_text())
        self.assertEqual(len(index['packages']), 5)
        def exists(args, **kwargs):
            self.assertEqual(args[1], 'api')
            return subprocess.CompletedProcess(args, 0, stdout='[[{"tag_name":"test.extension-v1.0.0","draft":true}]]')
        with self.assertRaises(ValueError):
            draft_release.create_draft(self.root / 'out', 'a' * 40, exists)

    def test_release_rejects_mixed_versions(self):
        self.build()
        self.metadata['version'] = '2.0.0'
        self.build('linux-arm64')
        with self.assertRaises(ValueError):
            package.release(self.root / 'out', 'a' * 40)
