#!/usr/bin/env python3
"""Inspect a built package and run real-worker tests against its extracted resources."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def command(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def mac_binary(path, minimum, architecture):
    linked = subprocess.check_output(['otool', '-L', str(path)], text=True).splitlines()[1:]
    if any(not line.strip().startswith(('/usr/lib/', '/System/Library/')) for line in linked):
        raise SystemExit(f'Non-system runtime library in {path}: {linked}')
    headers = subprocess.check_output(['otool', '-l', str(path)], text=True)
    versions = re.findall(r'\bminos\s+(\S+)', headers)
    if not versions or any(tuple(map(int, version.split('.'))) > tuple(map(int, minimum.split('.'))) for version in versions):
        raise SystemExit(f'Unexpected minimum macOS version in {path}: {versions}')
    actual = subprocess.check_output(['lipo', '-archs', str(path)], text=True).strip()
    if actual != architecture:
        raise SystemExit(f'Wrong architecture in {path}: {actual} != {architecture}')
    return {'path': path.name, 'minimumMacOS': versions, 'architecture': actual, 'dynamicLibraries': [line.strip() for line in linked]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', required=True, type=Path, help='.app, .deb, NSIS .exe, or directory containing one package')
    parser.add_argument('--output', type=Path, help='copy package and verification report into this artifact directory')
    args = parser.parse_args()
    package = args.package.resolve()
    if package.is_dir() and package.suffix != '.app':
        candidates = [p for p in package.iterdir() if p.suffix in ('.app', '.deb', '.exe')]
        if len(candidates) != 1:
            raise SystemExit(f'Expected exactly one package in {package}; found {candidates}')
        package = candidates[0]
    lock = json.loads((ROOT / 'scripts/image-backend.lock.json').read_text())
    report = {'package': package.name, 'host': platform.platform(), 'checks': []}
    with tempfile.TemporaryDirectory(prefix='recast-package-') as temporary:
        unpacked = Path(temporary) / 'unpacked'
        if package.suffix == '.app':
            unpacked = package
        elif package.suffix == '.deb':
            command(['dpkg-deb', '-x', str(package), str(unpacked)])
        elif package.suffix == '.exe':
            command(['7z', 'x', str(package), f'-o{unpacked}', '-y'], stdout=subprocess.DEVNULL)
        else:
            raise SystemExit(f'Unsupported package: {package}')
        workers = [p for p in unpacked.rglob('*') if p.name in ('magick', 'magick.exe') and p.parent.name == 'image-backend']
        if len(workers) != 1:
            raise SystemExit(f'Expected one packaged image worker; found {workers}')
        worker = workers[0]
        resources = worker.parent
        manifest = json.loads((resources / 'build-manifest.json').read_text())
        if manifest['source'] != lock['source'] or manifest['dependencies'] != lock['dependencies']:
            raise SystemExit('Packaged source/delegate manifest differs from the source lock.')
        actual = hashlib.sha256(worker.read_bytes()).hexdigest()
        if actual != manifest['binarySha256']:
            raise SystemExit('Packaged worker differs from the build manifest.')
        if hashlib.sha256((resources / 'sRGB.icc').read_bytes()).hexdigest() != lock['profileSha256']:
            raise SystemExit('Packaged color profile differs from the lock.')
        for dependency in lock['dependencies']:
            for name in dependency['licenses']:
                if not (resources / 'licenses' / f'{dependency["name"]}-{Path(name).name}').is_file():
                    raise SystemExit(f'Missing packaged license: {dependency["name"]}/{name}')
        if not (resources / 'licenses/ImageMagick-LICENSE').is_file():
            raise SystemExit('Missing ImageMagick license.')
        if (resources / 'policy.xml').read_bytes() != (ROOT / 'scripts/image-policy.xml').read_bytes():
            raise SystemExit('Packaged policy differs from the source policy.')
        report['checks'].append('worker hash, color profile, policy, and license inventory')
        spec = importlib.util.spec_from_file_location('image_builder', ROOT / 'scripts/prepare-image-backend.py')
        builder = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(builder)
        report['worker'] = builder.audit(worker, manifest['system'], manifest['architecture'])
        if package.suffix == '.app':
            import plistlib
            plist = plistlib.loads((package / 'Contents/Info.plist').read_bytes())
            if plist.get('LSMinimumSystemVersion') != lock['minimumMacOS']:
                raise SystemExit('App Info.plist does not match the locked deployment target.')
            report['application'] = mac_binary(package / 'Contents/MacOS' / plist['CFBundleExecutable'], lock['minimumMacOS'], manifest['architecture'])
        # A worker must operate without developer converter locations on PATH.
        env = os.environ.copy()
        env['PATH'] = os.environ.get('SystemRoot', r'C:\Windows') + r'\System32' if os.name == 'nt' else '/usr/bin:/bin'
        command([str(worker), '-version'], env=env)
        report['checks'].append('worker starts with a system-only PATH')
        env = os.environ.copy()
        env['RECAST_TEST_BACKEND'] = str(resources)
        command(['cargo', 'test', '--locked', '--manifest-path', str(ROOT / 'src-tauri/Cargo.toml')], cwd=ROOT, env=env)
        report['checks'].append('real conversion and job tests against extracted worker')
    print(json.dumps(report, indent=2))
    if args.output:
        args.output.mkdir(parents=True, exist_ok=True)
        (args.output / 'package-check.json').write_text(json.dumps(report, indent=2) + '\n')
        target = args.output / package.name
        if package.suffix == '.app':
            command(['ditto', '-c', '-k', '--keepParent', str(package), str(target.with_suffix('.app.zip'))])
        else:
            shutil.copy2(package, target)


if __name__ == '__main__':
    main()
