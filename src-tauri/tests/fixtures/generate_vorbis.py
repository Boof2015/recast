"""Generate synthetic Vorbis input with Xiph's standalone encoder example.

Pass the pinned audio build's install directory. This bypasses FFmpeg's encoder
wrapper and Ogg muxer. CI consumes checked-in fixtures without regeneration.
"""
import io
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import wave

ROOT = Path(__file__).resolve().parent


def build_example(stage, work, name):
    executable = work / name
    subprocess.run(['cc', '-O2', '-I', str(stage / 'include'),
                    str(stage.parent / 'vorbis/examples' / f'{name}.c'),
                    str(stage / 'lib/libvorbisenc.a'), str(stage / 'lib/libvorbisfile.a'),
                    str(stage / 'lib/libvorbis.a'), str(stage / 'lib/libogg.a'),
                    '-lm', '-o', str(executable)], check=True)
    return executable


def main():
    with tempfile.TemporaryDirectory(prefix='recast-vorbis-fixtures-') as temporary:
        encoder = build_example(Path(sys.argv[1]), Path(temporary), 'encoder_example')
        for name, frames in [('xiph-vorbis.ogg', 44117), ('xiph-vorbis-short.ogg', 117)]:
            output = io.BytesIO()
            with wave.open(output, 'wb') as wav:
                wav.setparams((2, 2, 44100, 0, 'NONE', 'not compressed'))
                wav.writeframes(b''.join(struct.pack('<h', int(7000 * math.sin(i*f*math.tau/44100)))
                                        for i in range(frames) for f in [431, 997]))
            encoded = subprocess.run([encoder], input=output.getvalue(), capture_output=True, check=True)
            (ROOT / name).write_bytes(encoded.stdout)


if __name__ == '__main__':
    main()
