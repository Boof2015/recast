"""Independent AAC fixtures made from synthetic tones with macOS CoreAudio.

CI uses the checked-in files, never afconvert. No external recordings are used.
"""
import math
from pathlib import Path
import struct
import subprocess
import tempfile
import wave

ROOT = Path(__file__).resolve().parent


def main():
    with tempfile.TemporaryDirectory(prefix='recast-aac-fixtures-') as temporary:
        source = Path(temporary) / 'tones.wav'
        with wave.open(str(source), 'wb') as output:
            output.setparams((2, 2, 48000, 0, 'NONE', 'not compressed'))
            output.writeframes(b''.join(
                struct.pack('<h', int(amplitude * math.sin(i * frequency * math.tau / 48000)))
                for i in range(48117)
                for amplitude, frequency in [(9000, 431), (7000, 997)]))
        for name, container, codec, bitrate in [
            ('apple-aac-lc.m4a', 'm4af', 'aac', 192000),
            ('apple-aac-he.m4a', 'm4af', 'aach', 64000),
            ('apple-aac-hev2.m4a', 'm4af', 'aacp', 48000),
            ('apple-aac-adts.aac', 'adts', 'aac', 128000),
        ]:
            subprocess.run(['/usr/bin/afconvert', '-f', container, '-d', codec,
                            '-b', str(bitrate), str(source), str(ROOT / name)], check=True)


if __name__ == '__main__':
    main()
