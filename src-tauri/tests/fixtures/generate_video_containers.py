"""Extra video fixtures from synthetic Apple clips and tones; CI uses checked-in files.

Run after generate_video.swift: python3 generate_video_containers.py /path/to/ffmpeg
Generated initially with Homebrew FFmpeg 9.0.1, separate from Recast's build.
"""
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent

def run(*args):
    subprocess.run([sys.argv[1], '-v', 'error', '-y', '-threads', '2',
                    '-filter_threads', '1', *map(str, args)], check=True)

with tempfile.TemporaryDirectory() as temporary:
    subtitle = Path(temporary) / 'captions.srt'
    subtitle.write_text('1\n00:00:00,250 --> 00:00:01,250\nSynthetic caption\n', encoding='utf-8')
    for name, codec in [('video-captions.mp4', 'mov_text'), ('video-srt.mkv', 'srt')]:
        run('-i', ROOT / 'apple-h264.mp4', '-i', subtitle, '-map', '0:v', '-map', '1:s',
            '-c:v', 'copy', '-c:s', codec, '-metadata:s:s:0', 'language=eng',
            '-disposition:s:0', 'default', ROOT / name)
    run('-i', ROOT / 'apple-h264.mp4', '-i', ROOT / 'apple-aac-lc.m4a', '-map', '0:v', '-map', '1:a',
        '-c:v', 'libvpx-vp9', '-threads:v', '2', '-b:v', '0', '-crf', '25', '-pix_fmt', 'yuv420p',
        '-c:a', 'libopus', '-b:a', '96000', ROOT / 'video-vp9-opus.webm')
