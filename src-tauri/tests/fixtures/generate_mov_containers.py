"""MOV-copy fixtures from synthetic Apple pixels; CI uses checked-in files.

Run: python3 generate_mov_containers.py /path/to/ffmpeg
Generated with independent Homebrew FFmpeg 9.0.1, not Recast's worker.
"""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent
for name, codec, options in [
    ('video-hevc.mov', 'libx265', ['-pix_fmt', 'yuv420p', '-tag:v', 'hvc1',
                                 '-x265-params', 'pools=1:frame-threads=1:log-level=error']),
    ('video-mpeg4.avi', 'mpeg4', ['-pix_fmt', 'yuv420p', '-q:v', '3']),
    ('video-mjpeg.avi', 'mjpeg', ['-pix_fmt', 'yuvj420p', '-q:v', '3']),
]:
    subprocess.run([sys.argv[1], '-v', 'error', '-y', '-threads', '2',
                    '-filter_threads', '1', '-i', str(root / 'apple-h264.mp4'),
                    '-map', '0:v', '-c:v', codec, '-threads:v', '2', *options,
                    str(root / name)], check=True)
