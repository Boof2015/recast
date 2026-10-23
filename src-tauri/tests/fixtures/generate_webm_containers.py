"""Independent VP8/Vorbis fixture; CI uses the checked-in synthetic media.
Run: python3 generate_webm_containers.py /path/to/ffmpeg
Generated with Homebrew FFmpeg 9.0.1, separately from Recast's worker.
"""
from pathlib import Path
import subprocess
import sys
root = Path(__file__).resolve().parent
subprocess.run([sys.argv[1], '-v', 'error', '-y', '-threads', '2', '-filter_threads', '1',
                '-i', str(root / 'apple-h264.mp4'), '-i', str(root / 'xiph-vorbis.ogg'),
                '-map', '0:v', '-map', '1:a', '-c:v', 'libvpx', '-threads:v', '2',
                '-b:v', '200k', '-c:a', 'copy',
                str(root / 'video-vp8-vorbis.webm')], check=True)
