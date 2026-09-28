# Recast

A local file converter, built with Tauri 2, React, TypeScript, and Rust.

The first real conversion is connected: **still PNG and JPEG → WebP**, with quality, lossless, resize, and metadata options. Batches preserve original files and existing outputs, report per-file results, support cancellation, and retry unfinished files. The approved design framework remains in place. More image formats, audio, and video are later milestones.

## Development

The macOS app and bundled worker now target macOS 13+ using source-built codecs. The Apple Silicon build is checked on macOS 26; macOS 13 itself remains unverified. See [BACKEND.md](BACKEND.md) for build prerequisites and [PLATFORM_CHECKS.md](PLATFORM_CHECKS.md) for platform status. After installing those prerequisites:

```sh
npm ci
npm run backend:prepare
npm run desktop
```

Choose or drop PNG/JPEG files, select WebP, adjust the group settings, and convert. Outputs go beside their sources by default; a different folder can be selected. Click a completed output to reveal it in Finder.

For the development-only sample UI:

```sh
npm run desktop:design
```

Sample files cannot be converted. Adding real files replaces the samples and uses the actual backend's capabilities. For a browser-only design preview, run `npm run dev` and open `http://127.0.0.1:1420/?design=mixed`.

## Build and check

```sh
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --bundles app
```

Native tests use the real bundled converter and require `backend:prepare`. `npm run package:check -- --package <package path> --output artifacts` repeats those tests against resources extracted from a package. The macOS app is generated at `src-tauri/target/release/bundle/macos/Recast.app`. It carries its converter and needs no Homebrew installation at runtime.

See [DESIGN.md](DESIGN.md) for the accepted design and [BACKEND.md](BACKEND.md) for implementation and verification details. The platform workflow covers Windows, Linux, and both Mac architectures. Native runner results, older macOS runtime checks, accessibility, and distribution signing are tracked separately in the platform record.
