#!/usr/bin/env python3
"""Collect notices from the exact downloaded Rust dependency sources."""
import json
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent


def write_notices(directory=HERE):
    host = next(line.split(': ', 1)[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--locked', '--offline', '--format-version', '1', '--filter-platform', host, '--manifest-path', str(directory / 'worker/Cargo.toml')], text=True))
    resolved = {node['id'] for node in metadata['resolve']['nodes']}
    wasmtime = next((Path(package['manifest_path']).parent / 'LICENSE' for package in metadata['packages'] if package['name'] == 'wasmtime'), None)
    sections = []
    missing = []
    for package in sorted(metadata['packages'], key=lambda package: (package['name'], package['version'])):
        if package['source'] is None or package['id'] not in resolved:
            continue
        root = Path(package['manifest_path']).parent
        paths = sorted({path for pattern in ['LICENSE*', 'COPYING*', 'NOTICE*', 'license*', 'licenses/**/*'] for path in root.glob(pattern) if path.is_file() and path.stat().st_size < 1024 * 1024})
        sections.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nLicense: {package['license']}\nRepository: {package.get('repository') or ''}\n")
        if package.get('license_file'):
            extra = root / package['license_file']
            if extra.is_file() and extra not in paths:
                paths.append(extra)
        if not paths and wasmtime and package['license'] == 'Apache-2.0 WITH LLVM-exception':
            sections.append(wasmtime.read_text(encoding='utf-8'))
            continue
        fallback = directory / 'licenses' / f"{package['name']}-{package['version']}.txt"
        if not paths and fallback.is_file():
            sections.append(fallback.read_text(encoding='utf-8'))
            continue
        if not paths:
            missing.append((package['name'], package['license']))
        for path in paths:
            sections.append(f'\n--- {path.relative_to(root)} ---\n' + path.read_text(encoding='utf-8'))
    if missing:
        raise ValueError(f'Dependency source packages missing license texts: {missing}')
    target = directory / 'worker/target/license-notices.txt'
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text('\n'.join(sections), encoding='utf-8')
    print(target)

if __name__ == '__main__':
    write_notices()
