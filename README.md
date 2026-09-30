# Recast

A local file converter, built with Tauri 2, React, TypeScript, and Rust.

Recast converts **still PNG, JPEG, WebP, BMP, and single-page TIFF in all directions**, including same-format conversion. Settings follow the output: JPEG has quality, WebP has quality/lossless, PNG and TIFF use lossless compression, and BMP writes uncompressed 24-bit files. JPEG and BMP fill transparency with a shared background color, white by default, selected through Recast's built-in picker. All five support resizing; BMP omits metadata after color normalization, while the other targets offer retention of supported metadata.

TIFF accepts classic TIFF and BigTIFF, either byte order, with unsigned samples up to 16 bits. Output is ordinary single-page TIFF with Deflate compression and preserved transparency. Resize and Keep metadata are its only settings; XMP/IPTC, color profiles, and common TIFF tags can be retained, but camera EXIF is not fully carried over. Multipage/layered TIFF, raw camera containers, floating-point samples, and unsupported compression receive an explicit explanation.

Batches preserve originals and existing outputs, report per-file results, support cancellation, and retry unfinished files. Animated PNG/WebP, multi-image BMPs, and BMP wrappers containing embedded PNG/JPEG are rejected. More image formats, audio, and video are later milestones.

## Development

The macOS app and bundled worker now target macOS 13+ using source-built codecs. The Apple Silicon build is checked on macOS 26; macOS 13 itself remains unverified. See [BACKEND.md](BACKEND.md) for build prerequisites and [PLATFORM_CHECKS.md](PLATFORM_CHECKS.md) for platform status. After installing those prerequisites:

```sh
npm ci
npm run backend:prepare
npm run desktop
```

Choose or drop PNG/JPEG/WebP/BMP/TIFF/AVIF/GIF files, select an output, adjust the group settings, and convert. Animated GIF/WebP inputs offer animation-preserving GIF/WebP destinations; TIFF inputs must have one page. Outputs go beside their sources by default; a different folder can be selected. Existing names, including the original during same-format conversion, get a numbered alternative. JPEG outputs use `.jpg`; TIFF outputs use `.tiff` (both `.tif` and `.tiff` inputs are recognized by content). Click a completed output to reveal it in its folder.

Use **File → New Window** (⌘N on macOS, Ctrl+N elsewhere) for an independent batch. **File → Add Files…** (⌘O / Ctrl+O) adds to the focused window. Closing a running window cancels that batch and waits for cleanup; quitting Recast cancels all active batches and waits for cleanup. Completed outputs stay on disk. Adding an existing input again, cancelling a picker, or selecting the same setting or output folder leaves completed results available.

New windows open slightly down and right from the current window, wrapping into view at screen edges.

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
