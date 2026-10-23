"""AV1 fixtures from synthetic pixels; CI uses the checked-in media.

Run: python3 generate_av1.py /path/to/ffmpeg
Uses independent FFmpeg 9.0.1 with SVT-AV1, not Recast's decoder.
"""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent


def run(*args):
    subprocess.run([sys.argv[1], '-v', 'error', '-y', '-threads', '2',
                    '-filter_threads', '1', *map(str, args)], check=True)


for name, source, depth, timing in [
    ('video-av1.mp4', 'apple-h264.mp4', 'yuv420p', ''),
    ('video-av1-vfr.mkv', 'apple-vfr-rotated.mov', 'yuv420p', ''),
    ('video-av1-24fps.webm', 'apple-h264.mp4', 'yuv420p', ',fps=24'),
    ('video-av1-10bit.mkv', 'apple-h264.mp4', 'yuv420p10le', ''),
]:
    run('-i', root / source, '-map', '0:v', '-vf', 'scale=128:96' + timing,
        '-c:v', 'libsvtav1', '-preset', '10', '-crf', '32',
        '-svtav1-params', 'lp=2', '-pix_fmt', depth, '-fps_mode', 'passthrough', root / name)

run('-i', root / 'video-av1.mp4', '-i', root / 'video-vp9-opus.webm',
    '-map', '0:v', '-map', '1:a', '-c', 'copy', root / 'video-av1-opus.webm')
