#!/usr/bin/env python3
"""Validate all platform packages, then create a draft (never publish)."""
import argparse
import json
import subprocess
from package import release, require

REPO = 'Boof2015/recast-extensions'


def create_draft(directory, source_commit, run=subprocess.run):
    info = release(directory, source_commit)
    result = run(['gh', 'api', '--paginate', '--slurp', f'repos/{REPO}/releases?per_page=100'], check=True, capture_output=True, text=True)
    existing = [item for page in json.loads(result.stdout) for item in page]
    require(not any(item['tag_name'] == info['tag'] for item in existing), 'That release already exists. Existing drafts and published packages are never overwritten.')
    run(['gh', 'release', 'create', info['tag'], '--repo', REPO, '--draft', '--latest=false', '--target', 'main', '--title', info['title'], '--notes-file', info['notes'], *info['assets']], check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True)
    parser.add_argument('--source-commit', required=True)
    args = parser.parse_args()
    create_draft(args.directory, args.source_commit)
