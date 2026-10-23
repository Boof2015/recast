"""Generate independent synthetic audio fixtures using macOS CoreAudio.

Run on macOS with Apple Lossless/AAC codecs available. CI reads the checked-in
files; it does not run this generator or need Apple's codecs.
"""

from pathlib import Path
import subprocess
import tempfile
import wave


ROOT = Path(__file__).resolve().parent


def main():
    with tempfile.TemporaryDirectory(prefix="recast-audio-fixtures-") as temporary:
        for bits in (16, 24, 32):
            source = Path(temporary) / f"source-{bits}.wav"
            samples = b"".join(
                ((i * 104729 * 7919) & 0xFFFFFFFF).to_bytes(4, "little")[: bits // 8]
                for i in range(5003 * 2)
            )
            with wave.open(str(source), "wb") as output:
                output.setnchannels(2)
                output.setsampwidth(bits // 8)
                output.setframerate(48000)
                output.writeframes(samples)
            subprocess.run(
                ["/usr/bin/afconvert", "-f", "m4af", "-d", "alac", str(source),
                 str(ROOT / f"apple-{bits}.m4a")], check=True,
            )
            if bits == 16:
                subprocess.run(
                    ["/usr/bin/afconvert", "-f", "m4af", "-d", "aac", "-b", "128000",
                     str(source), str(ROOT / "unsupported-aac.m4a")], check=True,
                )


if __name__ == "__main__":
    main()
