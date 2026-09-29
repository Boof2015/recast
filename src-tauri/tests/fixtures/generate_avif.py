#!/usr/bin/env python3
"""Rebuild synthetic AVIF fixtures using Recast's pinned AVIF-enabled worker.

Inputs are the existing synthetic PNG/TIFF fixtures, not photographs or third-party
assets. RGB/alpha samples are checked against those inputs in avif_tests.rs.
"""
from pathlib import Path
import subprocess
import argparse
import tempfile

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = Path(__file__).resolve().parent
WORKER = ROOT / 'src-tauri/resources/image-backend/magick'

def encode(source, name, *args):
    subprocess.run([str(WORKER), str(FIXTURES / source), '-define', 'heic:speed=6', *args, str(FIXTURES / name)], check=True)

lossless = ['-quality', '100', '-define', 'heic:lossless=true', '-define', 'heic:chroma=444', '-define', 'heic:cicp=1/13/0/1']
encode('rgba.png', 'rgba.avif', *lossless)
encode('rgb.png', 'rgb.avif', *lossless)
encode('rgba16.tiff', 'rgba12.avif', '-depth', '12', *lossless)
encode('gray16.tiff', 'gray10.avif', '-depth', '10', *lossless, '-define', 'heic:cicp=1/13/6/1')
encode('rotated.jpg', 'rotated.avif', *lossless)
encode('linear-rgb.png', 'profiled.avif', *lossless)
encode('rgb.png', 'hdr.avif', '-depth', '10', '-define', 'heic:cicp=9/16/9/1', '-quality', '85')
subprocess.run([str(WORKER), '-delay', '10', str(FIXTURES / 'rgb.png'), str(FIXTURES / 'rgba.png'), '-define', 'heic:speed=6', '-quality', '85', str(FIXTURES / 'animated.avif')], check=True)

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--codec-prefix', type=Path, help='Pinned codec install prefix; also rebuild the direct-libheif grid/collection fixtures')
args = parser.parse_args()
if args.codec_prefix:
    prefix = args.codec_prefix.resolve()
    with tempfile.TemporaryDirectory(prefix='recast-avif-fixtures-') as temporary:
        executable = Path(temporary) / 'generate-grid'
        subprocess.run(['c++', '-std=c++17', '-DLIBHEIF_STATIC_BUILD', '-I'+str(prefix / 'include'), str(FIXTURES / 'generate_avif_grid.cpp'), str(prefix / 'lib/libheif.a'), str(prefix / 'lib/libaom.a'), '-pthread', '-o', str(executable)], check=True)
        subprocess.run([str(executable), str(FIXTURES / 'grid.avif'), str(FIXTURES / 'collection.avif'), str(FIXTURES / 'alpha12.avif')], check=True)
