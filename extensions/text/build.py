#!/usr/bin/env python3
"""Build a pinned, offline Text & Markdown package for the current platform."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.request
import zipfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE.parent / 'shared'))
import package


def host():
    system = platform.system().lower()
    if system == 'darwin':
        system = 'macos'
    if system.startswith(('msys', 'mingw')):
        system = 'windows'
    arch = 'arm64' if platform.machine().lower() in ('arm64', 'aarch64') else 'x86_64'
    value = f'{system}-{arch}'
    if value not in package.PLATFORMS:
        raise ValueError(f'Unsupported platform: {value}')
    return value


def fetch(asset, downloads):
    filename = asset['sha256'] + '-' + asset['url'].rsplit('/', 1)[-1]
    path = downloads / filename
    if path.is_file() and package.digest(path) == asset['sha256']:
        return path
    path.unlink(missing_ok=True)
    for attempt in range(3):
        temporary = path.with_suffix('.partial')
        try:
            with urllib.request.urlopen(asset['url'], timeout=60) as source, temporary.open('wb') as output:
                shutil.copyfileobj(source, output)
            if package.digest(temporary) != asset['sha256']:
                raise ValueError(f"Checksum mismatch: {asset['url']}")
            temporary.replace(path)
            return path
        except (OSError, ValueError):
            temporary.unlink(missing_ok=True)
            if attempt == 2:
                raise
            time.sleep(1)
    raise AssertionError('unreachable')


def archive_files(path):
    if zipfile.is_zipfile(path):
        with zipfile.ZipFile(path) as archive:
            for info in archive.infolist():
                if not info.is_dir():
                    yield info.filename, archive.read(info)
    else:
        with tarfile.open(path) as archive:
            for info in archive:
                if info.isfile():
                    yield info.name, archive.extractfile(info).read()


def audit(payload, system):
    binaries = [payload / 'bin' / name for name in ('recast-text-worker', 'typst')]
    if system.startswith('macos'):
        expected = 'arm64' if system.endswith('arm64') else 'x86_64'
        for binary in binaries:
            architecture = subprocess.check_output(['lipo', '-archs', str(binary)], text=True).strip()
            if architecture != expected:
                raise ValueError(f'Wrong architecture: {binary}: {architecture}')
            import re
            load = subprocess.check_output(['otool', '-l', str(binary)], text=True)
            minima = re.findall(r'\bminos (\d+(?:\.\d+)*)', load)
            if not minima or any(tuple(map(int, version.split('.'))) > (13, 0) for version in minima):
                raise ValueError(f'macOS 13 deployment target exceeded: {binary}: {minima}')
            links = subprocess.check_output(['otool', '-L', str(binary)], text=True).splitlines()[1:]
            if any(not line.strip().startswith(('/usr/lib/', '/System/Library/')) for line in links):
                raise ValueError(f'Non-system runtime dependency: {binary}')


def build(output):
    lock = json.loads((HERE / 'dependencies.lock.json').read_text(encoding='utf-8'))
    system = host()
    root = REPO / '.backend-build' / 'text-extension' / system
    downloads = REPO / '.backend-build' / 'extension-downloads'
    downloads.mkdir(parents=True, exist_ok=True)
    root.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, MACOSX_DEPLOYMENT_TARGET='13.0')
    subprocess.run(['cargo', 'build', '--locked', '--release', '-j', '2', '--manifest-path', str(HERE / 'worker/Cargo.toml')], env=env, check=True)
    subprocess.run([sys.executable, str(HERE / 'notices.py')], check=True)
    suffix = '.exe' if system.startswith('windows') else ''
    worker = HERE / 'worker/target/release' / ('recast-text-worker' + suffix)
    with tempfile.TemporaryDirectory(prefix='stage-', dir=root) as staging:
        payload = Path(staging)
        for directory in ('bin', 'lib', 'fonts', 'templates', 'licenses'):
            (payload / directory).mkdir()
        shutil.copyfile(worker, payload / 'bin' / worker.name)
        (payload / 'bin' / worker.name).chmod(0o755)
        pandoc = fetch(lock['pandoc'], downloads)
        wasm = root / 'pandoc.wasm'
        for name, data in archive_files(pandoc):
            if name.endswith('/pandoc.wasm'):
                wasm.write_bytes(data)
            elif name.endswith(('/COPYING.md', '/COPYRIGHT')):
                (payload / 'licenses' / ('pandoc-' + Path(name).name)).write_bytes(data)
        compiled = root / 'pandoc.cwasm'
        stamp = root / 'compile-stamp.json'
        identity = {'worker': package.digest(worker), 'wasm': package.digest(wasm)}
        if not compiled.exists() or not stamp.exists() or json.loads(stamp.read_text()) != identity:
            subprocess.run([str(worker), '--compile', str(wasm), str(compiled)], check=True, timeout=900)
            stamp.write_text(json.dumps(identity))
        shutil.copyfile(compiled, payload / 'lib/pandoc.cwasm')
        shutil.copyfile(compiled.with_suffix('.sha256'), payload / 'lib/pandoc.sha256')
        for name, data in archive_files(fetch(lock['typst']['platforms'][system], downloads)):
            basename = Path(name).name
            if basename == 'typst' + suffix:
                (payload / 'bin' / basename).write_bytes(data)
                (payload / 'bin' / basename).chmod(0o755)
            elif basename in ('LICENSE', 'NOTICE'):
                (payload / 'licenses' / ('typst-' + basename)).write_bytes(data)
        for asset in lock['fonts']:
            shutil.copyfile(fetch(asset, downloads), payload / asset['path'])
        shutil.copyfile(HERE / 'document.typst', payload / 'templates/document.typst')
        # Rust and transitive dependency licenses are gathered from Cargo's
        # exact lockfile by the release build before packaging.
        notices = HERE / 'worker/target/license-notices.txt'
        if not notices.is_file():
            raise ValueError('Generate worker license notices with notices.py before packaging')
        shutil.copyfile(notices, payload / 'licenses/worker-dependencies.txt')
        (payload / 'licenses/SOURCES.json').write_text(json.dumps(lock, indent=2) + '\n', encoding='utf-8')
        descriptor = {'protocolVersion': 1, 'executable': 'bin/' + worker.name,
            'inputs': [{'id':'markdown','label':'Markdown','kind':'documents','extensions':['md','markdown']}, {'id':'text','label':'TXT','kind':'documents','extensions':['txt']}],
            'targets': [{'id':target,'label':label,'description':description,'extension':target,'inputs':['markdown','text'],
                'settings':[{'id':'page-size','label':'Page size','choices':['A4','Letter'],'default':'A4'}]}
                for target,label,description in [('pdf','PDF','Ready to share'),('docx','DOCX','Editable Word document')]]}
        (payload / 'provider.json').write_text(json.dumps(descriptor, indent=2) + '\n', encoding='utf-8')
        audit(payload, system)
        metadata = {'id':'recast.text','name':'Text & Markdown','version':lock['version'], 'description':'Convert Markdown and plain text to PDF or DOCX.', 'publisher':'Recast','platform':system,'minimumAppVersion':'0.1.0'}
        path = package.build(metadata, payload, output)
        print(path, flush=True)
        return path

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=REPO / 'extension-artifacts')
    args = parser.parse_args()
    build(args.output.resolve())
