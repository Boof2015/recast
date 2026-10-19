"""Generate synthetic MP3 input fixtures with standalone LAME, not Recast.

Usage: python3 generate_mp3.py /path/to/lame
Initial fixtures use LAME 4.0. CI only reads the checked-in files.
"""
from pathlib import Path
import math
import struct
import subprocess
import sys
import tempfile
import wave

ROOT = Path(__file__).resolve().parent


def main():
    lame = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix="recast-mp3-fixtures-") as temporary:
        source = Path(temporary) / "source.wav"
        with wave.open(str(source), "wb") as output:
            output.setparams((2, 2, 44100, 0, "NONE", "not compressed"))
            output.writeframes(b"".join(
                struct.pack("<hh", int(9000 * math.sin(i * 431 * math.tau / 44100)),
                            int(7000 * math.sin(i * 997 * math.tau / 44100)))
                for i in range(44117)
            ))
        for name, options in [
            ("lame-vbr.mp3", ["-V", "2", "--tt", "Recast synthetic tone", "--ta", "Recast fixture"]),
            ("lame-crc.mp3", ["-b", "192", "-p"]),
            ("lame-no-xing.mp3", ["-b", "128", "-t"]),
        ]:
            subprocess.run([lame, "--silent", "--noreplaygain", *options, str(source), str(ROOT / name)], check=True)


if __name__ == "__main__":
    main()
