#!/usr/bin/env python3
"""Build the tiny example provider for the current machine; no conversion engines."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(HERE.parent / 'shared'))
import package


def host():
    system = {'Darwin': 'macos', 'Windows': 'windows', 'Linux': 'linux'}.get(platform.system())
    arch = {'arm64': 'arm64', 'aarch64': 'arm64', 'amd64': 'x86_64', 'x86_64': 'x86_64'}.get(platform.machine().lower())
    name = f'{system}-{arch}'
    if name not in package.PLATFORMS:
        raise ValueError(f'Unsupported platform: {name}')
    return name


def build(output):
    system = host()
    subprocess.run(['cargo', 'build', '--locked', '--release', '--manifest-path', str(HERE / 'worker/Cargo.toml')],
                   env=dict(os.environ, MACOSX_DEPLOYMENT_TARGET='13.0'), check=True)
    # Reuse the dependency-notice collector without building the Text extension.
    spec = importlib.util.spec_from_file_location('notices', HERE.parent / 'text/notices.py')
    notices = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(notices)
    notices.write_notices(HERE)
    with tempfile.TemporaryDirectory(prefix='recast-example-') as temporary:
        payload = Path(temporary) / 'payload'
        (payload / 'bin').mkdir(parents=True)
        (payload / 'licenses').mkdir()
        binary = 'recast-example-worker' + ('.exe' if system.startswith('windows') else '')
        shutil.copyfile(HERE / 'worker/target/release' / binary, payload / 'bin' / binary)
        (payload / 'bin' / binary).chmod(0o755)
        shutil.copyfile(HERE / 'worker/target/license-notices.txt', payload / 'licenses/rust-dependencies.txt')
        descriptor = json.loads((HERE / 'provider.json').read_text(encoding='utf-8'))
        descriptor['executable'] = 'bin/' + binary
        (payload / 'provider.json').write_text(json.dumps(descriptor, indent=2) + '\n', encoding='utf-8')
        metadata = {
            'id': 'example.text-html', 'name': 'Text to HTML example', 'version': '0.1.0',
            'description': 'Convert UTF-8 plain text to a self-contained HTML page.',
            'publisher': 'Recast example', 'platform': system, 'minimumAppVersion': '0.1.0',
        }
        return package.build(metadata, payload, output)


if __name__ == '__main__':
    if sys.version_info < (3, 12):
        raise SystemExit('Python 3.12+ is required. Use a current Python interpreter (for example python3.14).')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=REPO / '.backend-build/example-extension/packages')
    args = parser.parse_args()
    print(build(args.output))
