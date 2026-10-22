#!/usr/bin/env python3
"""Build/verify Recast packages. No package-supplied code is executed."""
import argparse
import hashlib
import json
import re
import stat
import zipfile
from pathlib import Path

PLATFORMS = {'macos-arm64', 'macos-x86_64', 'windows-x86_64', 'linux-arm64', 'linux-x86_64'}
MAX_ARCHIVE = 1024 ** 3
MAX_INSTALLED = 4 * 1024 ** 3
MAX_MANIFEST = 8 * 1024 ** 2
MAX_FILES = 30_000
ASSETS = 'https://github.com/Boof2015/recast-extensions/releases/download/'
SEMVER = re.compile(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?')
SUMMARY_KEYS = {'id', 'name', 'version', 'description', 'publisher', 'platform', 'minimumAppVersion', 'installedBytes'}


def require(value, message):
    if not value:
        raise ValueError(message)


def valid_version(value):
    if not isinstance(value, str) or len(value) > 64:
        return False
    match = SEMVER.fullmatch(value)
    return bool(match) and all(not part.isdigit() or len(part) == 1 or not part.startswith('0') for part in (match[4] or '').split('.'))


def portable_path(value):
    if not isinstance(value, str) or not value or len(value) > 240:
        return False
    for part in value.split('/'):
        stem = part.split('.')[0].upper()
        if not part or part in ('.', '..') or part.endswith(('.', ' ')):
            return False
        if any(ord(c) < 32 or ord(c) >= 127 or c in '\\:*?"<>|' for c in part):
            return False
        if stem in {'CON', 'PRN', 'AUX', 'NUL'} or re.fullmatch(r'(COM|LPT)[1-9]', stem):
            return False
    return True


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def validate_manifest(manifest):
    require(set(manifest) == {'schemaVersion', 'package', 'files'} and manifest['schemaVersion'] == 1, 'Unsupported package format')
    package = manifest['package']
    require(set(package) == SUMMARY_KEYS, 'Invalid package metadata keys')
    require(isinstance(package['id'], str) and re.fullmatch(r'[a-z][a-z0-9.-]{0,79}', package['id']) and portable_path(package['id']), 'Invalid package ID')
    for key, limit in [('name', 100), ('description', 500), ('publisher', 100)]:
        value = package[key]
        require(isinstance(value, str) and (key == 'description' or value.strip()) and len(value.encode()) <= limit and all(ord(c) >= 32 and ord(c) != 127 for c in value), f'Invalid {key}')
    require(valid_version(package['version']) and valid_version(package['minimumAppVersion']), 'Invalid version')
    require(package['platform'] in PLATFORMS, 'Invalid platform')
    require(type(package['installedBytes']) is int and 0 < package['installedBytes'] <= MAX_INSTALLED, 'Invalid installed size')
    files = manifest['files']
    require(isinstance(files, list) and 0 < len(files) <= MAX_FILES, 'Invalid file inventory')
    paths, parents = set(), set()
    total = 0
    for entry in files:
        require(set(entry) == {'path', 'bytes', 'sha256', 'executable'}, 'Invalid file information')
        require(portable_path(entry['path']), 'Unsafe package path')
        path = entry['path'].lower()
        require(path not in paths, 'Duplicate package path')
        paths.add(path)
        parent = path
        while '/' in parent:
            parent = parent.rsplit('/', 1)[0]
            parents.add(parent)
        require(type(entry['bytes']) is int and 0 <= entry['bytes'] <= MAX_INSTALLED, 'Invalid file size')
        require(isinstance(entry['sha256'], str) and re.fullmatch('[0-9a-f]{64}', entry['sha256']), 'Invalid file checksum')
        require(type(entry['executable']) is bool, 'Invalid executable flag')
        total += entry['bytes']
    require(paths.isdisjoint(parents), 'Conflicting package paths')
    require(total == package['installedBytes'], 'Incorrect installed size')
    require(any(path.startswith('licenses/') for path in paths), 'Include dependency licenses under licenses/')
    return manifest


def build(metadata, payload, output):
    payload = Path(payload).resolve(strict=True)
    files = []
    for path in sorted(payload.rglob('*')):
        require(not path.is_symlink(), 'Stage regular files, not symlinks; preserve needed aliases as copies')
        if path.is_dir():
            continue
        require(path.is_file(), 'Only regular files are supported')
        relative = path.relative_to(payload).as_posix()
        files.append({'path': relative, 'bytes': path.stat().st_size, 'sha256': digest(path), 'executable': bool(path.stat().st_mode & 0o111)})
    package = dict(metadata, installedBytes=sum(entry['bytes'] for entry in files))
    manifest = validate_manifest({'schemaVersion': 1, 'package': package, 'files': files})
    output = Path(output)
    require(not output.resolve().is_relative_to(payload), 'Output directory must be outside the payload')
    output.mkdir(parents=True, exist_ok=True)
    name = f"{package['id']}-{package['version']}-{package['platform']}.recast-extension"
    destination = output / name
    try:
        with zipfile.ZipFile(destination, 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            archive.writestr('manifest.json', json.dumps(manifest, sort_keys=True))
            for entry in files:
                archive.write(payload / entry['path'], 'payload/' + entry['path'])
        verify(destination)
    except BaseException:
        # Never remove an existing output if exclusive creation failed.
        if 'archive' in locals():
            destination.unlink(missing_ok=True)
        raise
    return destination


def verify(path):
    path = Path(path)
    require(0 < path.stat().st_size <= MAX_ARCHIVE, 'Invalid archive size')
    with zipfile.ZipFile(path) as archive:
        require(len(archive.infolist()) <= MAX_FILES + 1, 'Too many archive entries')
        info = archive.getinfo('manifest.json')
        require(info.file_size <= MAX_MANIFEST, 'Manifest too large')
        manifest = validate_manifest(json.loads(archive.read(info)))
        expected = {'payload/' + entry['path']: entry for entry in manifest['files']}
        require(len(archive.infolist()) == len(expected) + 1, 'Archive inventory mismatch')
        seen = set()
        for info in archive.infolist():
            mode = info.external_attr >> 16
            require(info.filename not in seen and not info.is_dir() and stat.S_IFMT(mode) in (0, stat.S_IFREG) and not info.flag_bits & 1, 'Unsupported or duplicate archive entry')
            seen.add(info.filename)
            if info.filename == 'manifest.json':
                continue
            require(info.filename in expected, 'Undeclared archive file')
            entry = expected[info.filename]
            require(info.file_size == entry['bytes'], 'Incorrect archive file size')
            with archive.open(info) as source:
                hasher, count = hashlib.sha256(), 0
                while chunk := source.read(128 * 1024):
                    count += len(chunk)
                    require(count <= entry['bytes'], 'File exceeds declared size')
                    hasher.update(chunk)
            require(count == entry['bytes'] and hasher.hexdigest() == entry['sha256'], 'Checksum mismatch')
    return manifest


def release(directory, source_commit, require_all=True):
    directory = Path(directory)
    packages = sorted(directory.rglob('*.recast-extension'))
    require(packages, 'No extension packages found')
    require(re.fullmatch('[0-9a-f]{40}', source_commit), 'A complete source commit is required')
    offers, seen = [], set()
    identity = None
    common = None
    for path in packages:
        manifest = verify(path)
        package = manifest['package']
        current = (package['id'], package['version'])
        fields = {key: value for key, value in package.items() if key not in ('platform', 'installedBytes')}
        require(identity is None or current == identity, 'Each release must contain one extension version')
        require(common is None or fields == common, 'Platform package metadata differs')
        identity, common = current, fields
        require(package['platform'] not in seen, 'Duplicate platform package')
        seen.add(package['platform'])
        require('+' not in package['version'], 'Release versions must not use build metadata')
        tag = f"{package['id']}-v{package['version']}"
        expected_name = f"{package['id']}-{package['version']}-{package['platform']}.recast-extension"
        require(path.name == expected_name, 'Unexpected package filename')
        offers.append({'package': package, 'sha256': digest(path), 'downloadBytes': path.stat().st_size, 'url': ASSETS + tag + '/' + path.name})
    require(not require_all or seen == PLATFORMS, 'All five platform packages are required before drafting a release')
    index = directory / 'extension-release.json'
    index.write_text(json.dumps({'schemaVersion': 1, 'packages': offers}, indent=2) + '\n')
    checksums = directory / 'SHA256SUMS'
    checksums.write_text(''.join(f"{offer['sha256']}  {Path(offer['url']).name}\n" for offer in offers) + f'{digest(index)}  {index.name}\n')
    notes = directory / 'release-notes.md'
    notes.write_text(f"{common['description']}\n\nSource: https://github.com/Boof2015/recast/commit/{source_commit}\n\nInstall the package for your platform in **Recast → Settings → Extensions → Install from file**.\n\nThis draft is for testing. Publish manually only after verifying the packages on all supported platforms.\n")
    return {'tag': tag, 'title': f"{common['name']} {common['version']}", 'assets': [str(path) for path in packages] + [str(index), str(checksums)], 'notes': str(notes)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    pack = sub.add_parser('build')
    pack.add_argument('--metadata', type=Path, required=True)
    pack.add_argument('--payload', type=Path, required=True)
    pack.add_argument('--output', type=Path, required=True)
    check = sub.add_parser('verify')
    check.add_argument('package', type=Path)
    args = parser.parse_args()
    if args.command == 'build':
        print(build(json.loads(args.metadata.read_text()), args.payload, args.output))
    else:
        verify(args.package)
        print('Package verified.')


if __name__ == '__main__':
    main()
