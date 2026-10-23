#!/usr/bin/env python3
"""Build the optional, pinned LibreOffice provider for this platform."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE.parent / 'shared'))
import package

# The same checksum/download and license rules apply to both official packs.
def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value

common = module('text_build', HERE.parent / 'text/build.py')
notices = module('worker_notices', HERE.parent / 'text/notices.py')


def copy_engine(source, destination, mac=False):
    source = source.resolve()
    # Resolve upstream aliases into ordinary package files. Reject links outside
    # the distribution. urelibs is a compatibility alias for SDK helpers; runtime
    # fundamentalrc points directly at Frameworks, so avoid duplicating it.
    for base, dirs, files in os.walk(source, followlinks=False):
        for name in dirs + files:
            path = Path(base) / name
            if path.is_symlink() and not path.resolve(strict=True).is_relative_to(source):
                raise ValueError(f'External engine symlink: {path}')
    def ignore(directory, names):
        return ['urelibs'] if mac and Path(directory).relative_to(source).as_posix() == 'Contents/MacOS' else []
    shutil.copytree(source, destination, symlinks=False, ignore=ignore)


def stage_engine(asset, system, destination):
    with tempfile.TemporaryDirectory(prefix='office-unpack-') as temporary:
        work = Path(temporary)
        if system.startswith('macos'):
            mount = work / 'volume'
            mount.mkdir()
            subprocess.run(['hdiutil', 'attach', '-readonly', '-nobrowse', '-mountpoint', str(mount), str(asset)], check=True)
            try:
                copy_engine(mount / 'LibreOffice.app', destination / 'LibreOffice.app', mac=True)
            finally:
                subprocess.run(['hdiutil', 'detach', str(mount)], check=True)
            return 'office/LibreOffice.app/Contents/MacOS/soffice', destination / 'LibreOffice.app/Contents/Resources/fonts/truetype'
        if system.startswith('windows'):
            expanded = work / 'expanded'
            command = ['msiexec.exe', '/a', str(asset), '/qn', 'TARGETDIR=' + str(expanded), '/l*v', str(work / 'extract.log')]
            result = subprocess.run(command, timeout=600)
            if result.returncode not in (0, 3010):
                raise ValueError(f'MSI extraction failed ({result.returncode}): {(work / "extract.log").read_text(errors="replace")[-6000:]}')
            binaries = list(expanded.rglob('program/soffice.bin'))
            if len(binaries) != 1:
                raise ValueError('Cannot identify the extracted Office installation')
            copy_engine(binaries[0].parents[1], destination)
        else:
            with tarfile.open(asset) as archive:
                archive.extractall(work / 'deb', filter='data')
            packages = sorted((work / 'deb').rglob('*.deb'))
            if not packages:
                raise ValueError('No Office engine Debian packages found')
            for deb in packages:
                subprocess.run(['dpkg-deb', '-x', str(deb), str(work / 'expanded')], check=True)
            binaries = list((work / 'expanded/opt').glob('libreoffice*/program/soffice.bin'))
            if len(binaries) != 1:
                raise ValueError('Cannot identify the extracted Office installation')
            copy_engine(binaries[0].parents[1], destination)
        return 'office/program/soffice.bin', destination / 'share/fonts/truetype'


def audit(payload, system):
    executable = payload / json.loads((payload / 'engine.json').read_text())['executable']
    if system.startswith('macos'):
        import re
        binaries = []
        seen = set()
        for binary in payload.rglob('*'):
            if not binary.is_file():
                continue
            with binary.open('rb') as stream:
                header = stream.read(8)
                magic = header[:4]
            if magic not in (b'\xcf\xfa\xed\xfe', b'\xce\xfa\xed\xfe', b'\xca\xfe\xba\xbe'):
                continue
            if magic == b'\xca\xfe\xba\xbe' and not 1 <= int.from_bytes(header[4:8], 'big') <= 16:
                continue  # Java class files share the universal Mach-O magic.
            digest = package.digest(binary)
            if digest not in seen:
                seen.add(digest)
                binaries.append(binary)
        for binary in binaries:
            arch = subprocess.check_output(['lipo', '-archs', str(binary)], text=True).strip()
            expected = 'arm64' if system.endswith('arm64') else 'x86_64'
            if expected not in arch.split():
                raise ValueError(f'Incorrect architecture: {binary}: {arch}')
            load = subprocess.check_output(['otool', '-l', str(binary)], text=True)
            minima = re.findall(r'\bminos (\d+(?:\.\d+)*)', load) + re.findall(r'cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+(?:\.\d+)*)', load)
            if not minima or any(tuple(map(int, v.split('.'))) > (13, 0) for v in minima):
                raise ValueError(f'macOS 13 target exceeded: {binary}: {minima}')
            links = subprocess.check_output(['otool', '-L', str(binary)], text=True).splitlines()[1:]
            own_ids = subprocess.check_output(['otool', '-D', str(binary)], text=True).splitlines()[1:]
            links = [line for line in links if line.strip().split(' (', 1)[0] not in own_ids]
            if any(not line.strip().startswith(('/usr/lib/', '/System/Library/', '@rpath/', '@loader_path/', '@executable_path/')) for line in links):
                raise ValueError(f'External runtime dependency: {binary}')
    elif system.startswith('linux'):
        env = dict(os.environ, LD_LIBRARY_PATH=str(executable.parent))
        links = subprocess.run(['ldd', str(executable)], env=env, text=True, capture_output=True, check=True).stdout
        if 'not found' in links:
            raise ValueError(f'Missing Office runtime libraries:\n{links}')


def build(output, stage_only=False):
    lock = json.loads((HERE / 'dependencies.lock.json').read_text(encoding='utf-8'))
    system = common.host()
    root = REPO / '.backend-build/office-extension' / system
    root.mkdir(parents=True, exist_ok=True)
    downloads = REPO / '.backend-build/extension-downloads'
    downloads.mkdir(parents=True, exist_ok=True)
    subprocess.run(['cargo', 'build', '--locked', '--release', '-j', '2', '--manifest-path', str(HERE / 'worker/Cargo.toml')], env=dict(os.environ, MACOSX_DEPLOYMENT_TARGET='13.0'), check=True)
    notices.write_notices(HERE)
    payload = root / 'payload'
    engine_stamp = root / 'engine-stamp.json'
    identity = {'engine': lock['libreoffice'], 'fonts': lock['fonts'], 'stagingVersion': 1}
    if not engine_stamp.exists() or json.loads(engine_stamp.read_text()) != identity or not (payload / 'engine.json').exists():
        shutil.rmtree(payload, ignore_errors=True)
        (payload / 'licenses').mkdir(parents=True)
        asset = common.fetch(lock['libreoffice']['platforms'][system], downloads)
        executable, fonts = stage_engine(asset, system, payload / 'office')
        fonts.mkdir(parents=True, exist_ok=True)
        for font in lock['fonts']:
            target = fonts / Path(font['path']).name if font['path'].startswith('fonts/') else payload / font['path']
            shutil.copyfile(common.fetch(font, downloads), target)
        (payload / 'engine.json').write_text(json.dumps({'executable': executable, 'version': lock['libreoffice']['version']}) + '\n', encoding='utf-8')
        engine_stamp.write_text(json.dumps(identity), encoding='utf-8')
    suffix = '.exe' if system.startswith('windows') else ''
    worker = 'recast-office-worker' + suffix
    (payload / 'bin').mkdir(exist_ok=True)
    shutil.copyfile(HERE / 'worker/target/release' / worker, payload / 'bin' / worker)
    (payload / 'bin' / worker).chmod(0o755)
    shutil.copyfile(HERE / 'worker/target/license-notices.txt', payload / 'licenses/worker-dependencies.txt')
    (payload / 'licenses/SOURCES.json').write_text(json.dumps(lock, indent=2) + '\n', encoding='utf-8')
    (payload / 'licenses/LibreOffice.txt').write_text('LibreOffice is distributed under MPL 2.0 and other licenses. Complete upstream notices are retained inside office/. Source for LibreOffice ' + lock['libreoffice']['version'] + ': https://download.documentfoundation.org/libreoffice/src/' + lock['libreoffice']['version'] + '/\n', encoding='utf-8')
    descriptor = {'protocolVersion':1, 'executable':'bin/' + worker,
        'inputs':[{'id':'docx','label':'DOCX','kind':'documents','extensions':['docx']}],
        'targets':[{'id':'pdf','label':'PDF','description':'Keep document layout','extension':'pdf','inputs':['docx'],'settings':[]}]}
    (payload / 'provider.json').write_text(json.dumps(descriptor, indent=2) + '\n', encoding='utf-8')
    audit(payload, system)
    if stage_only:
        print(payload, flush=True)
        return payload
    metadata = {'id':'recast.office','name':'Office','version':lock['version'],'description':'Convert DOCX documents to PDF using LibreOffice.','publisher':'Recast','platform':system,'minimumAppVersion':'0.1.0'}
    path = package.build(metadata, payload, output)
    print(path, flush=True)
    return path

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=REPO / 'extension-artifacts')
    parser.add_argument('--stage-only', action='store_true')
    args = parser.parse_args()
    build(args.output.resolve(), args.stage_only)
