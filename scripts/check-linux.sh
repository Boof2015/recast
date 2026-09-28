#!/usr/bin/env bash
set -euo pipefail
# Used by scripts/Dockerfile.linux-check. /source is a read-only repository mount.
# Only this disposable /work copy is built; native host resources stay intact.
tar -C /source --exclude=./.git --exclude=./node_modules --exclude=./dist \
  --exclude=./src-tauri/target --exclude=./src-tauri/resources/image-backend \
  --exclude=./.backend-build --exclude=./.agents --exclude=./.codex -cf - . | tar -C /work -xf -
mkdir -p .backend-build/downloads
if [ -d /source/.backend-build/downloads ]; then
  cp /source/.backend-build/downloads/*.tar.gz .backend-build/downloads/
fi
npm ci
rustup component add rustfmt clippy
python3 scripts/prepare-image-backend.py
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --bundles deb -- --locked
python3 scripts/check-package.py --package src-tauri/target/release/bundle/deb --output /artifacts
