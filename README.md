# Recast

A local file converter, built with Tauri 2, React, TypeScript, and Rust.

Recast converts **still PNG, JPEG, WebP, BMP, TIFF, AVIF, and GIF**, **animated GIF ↔ animated WebP**, and **PCM WAV ↔ FLAC**, including same-format conversion. Images and audio can share a batch with independent group settings. Everything runs locally through bundled converters.

## Images

Settings follow the output: JPEG has quality, WebP and AVIF have quality/lossless, PNG and TIFF use lossless compression, and BMP writes uncompressed 24-bit files. JPEG and BMP fill transparency with a shared background color, white by default, selected through Recast's built-in picker. Still-image outputs support resizing; BMP omits metadata after color normalization, while the other applicable targets offer retention of supported metadata. AVIF supports up to 12-bit color. GIF offers palette size and dithering, preserves cut-out transparency, and rounds animation timing to 10 ms.

TIFF accepts classic TIFF and BigTIFF, either byte order, with unsigned samples up to 16 bits. Output is ordinary single-page TIFF with Deflate compression and preserved transparency. Resize and Keep metadata are its only settings; XMP/IPTC, color profiles, and common TIFF tags can be retained, but camera EXIF is not fully carried over. Multipage/layered TIFF, raw camera containers, floating-point samples, and unsupported compression receive an explicit explanation.

Animated GIF/WebP inputs only offer destinations that retain animation. Animated PNG/AVIF, multi-image BMPs, and BMP wrappers containing embedded PNG/JPEG are rejected. Explicit frame selection and video-to-animation are later work.

## Audio

The first audio increment accepts PCM WAV (8/16/24/32-bit integer and 32/64-bit float) and FLAC (16/24/32-bit). WAV output supports those PCM representations; **FLAC output currently supports 16-bit and 24-bit PCM**. Float, 8-bit, and 32-bit sources require an explicit compatible sample format for FLAC; Recast never silently reduces them to 24-bit.

- **Keep source** preserves each file's sample rate and sample format where the destination supports them. Explicit selections apply to the whole audio group.
- One to eight channels are retained; files with more than two channels need a recognized channel layout. Channel mixing is not implemented.
- **Dithering: Auto** applies unshaped triangular dither once at final integer quantization when reducing precision or resampling. Unchanged integer samples and exact widening bypass it. **Off** is also available.
- **Allow clipping** permits integer output when decoded or resampled peaks exceed its range. Without consent, affected files fail with an **Allow clipping & retry** action that preserves completed outputs. Recast does not normalize or limit the signal; compatible float WAV retains out-of-range values.
- **Keep metadata** carries supported ordinary text tags. Artwork, loudness-analysis tags, and production metadata are omitted. Output details report sample properties and whether dithering was applied.

ALAC, MP3, AAC/M4A, Opus, Ogg Vorbis, and the planned video formats remain on the roadmap. This increment establishes the shared audio workflow; it does not complete audio format coverage.

## Batches

Batches preserve originals and existing outputs, produce one output per successful input, report per-file results, support cancellation, and retry unfinished files. Destinations must work for the entire group.

## Development

The macOS app and bundled workers target macOS 13+ using source-built codecs. The Apple Silicon build is checked on macOS 26; macOS 13 itself remains unverified.

Build prerequisites are Node.js 24+, Rust (CI pins 1.98.1), Python 3.12+, CMake, Make, a C/C++ compiler, pkg-config, and the [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/). NASM enables the image backend's x86 JPEG optimizations. On macOS, install Xcode command-line tools and ensure the modern Python is on PATH as `python3`. On Windows, build both converters in MSYS2 UCRT64 with `/usr/bin/python`, then use MSVC Rust for the app; the [platform workflow](.github/workflows/platform-checks.yml) lists the build packages.

```sh
npm ci
npm run backend:prepare
npm run desktop
```

`backend:prepare` builds both the image worker and FFmpeg/ffprobe from pinned, checksum-verified sources. To rebuild only one, use `npm run backend:image` or `npm run backend:audio`. Windows equivalents are `/usr/bin/python scripts/prepare-image-backend.py` and `/usr/bin/python scripts/prepare-audio-backend.py` in MSYS2 UCRT64.

Choose or drop supported images, WAV, or FLAC files, select an output for each group, adjust settings, and convert. Outputs go beside their sources by default; a different folder can be selected. Existing names, including the original during same-format conversion, get a numbered alternative. JPEG outputs use `.jpg`; TIFF outputs use `.tiff` (both `.tif` and `.tiff` inputs are recognized by content). Click a completed output to reveal it in its folder.

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
python3 -m unittest discover -s scripts/tests -v
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri -- build --bundles app -- --locked
```

Native tests use the real bundled converters and require `backend:prepare`. `npm run package:check -- --package <package path> --output artifacts` checks executable hashes, architecture, runtime dependencies, and source/license notices, then repeats the tests against packaged resources. The macOS app is generated at `src-tauri/target/release/bundle/macos/Recast.app`. It needs no Homebrew installation at runtime. Other platforms use their configured bundle targets instead of `--bundles app`.

Source versions and hashes are recorded in [image-backend.lock.json](scripts/image-backend.lock.json) and [audio-backend.lock.json](scripts/audio-backend.lock.json). The minimal FFmpeg build enables WAV/FLAC and PCM processing without network access or external codec libraries. Generated resources carry build manifests and license notices and are ignored by Git.

The platform workflow covers Windows x86_64, Linux x86_64/ARM64, and both Mac architectures. The previous image/batch increment passed all five targets; the new audio increment needs its own run. Older macOS runtime checks, native desktop/accessibility checks, and distribution signing remain separate release work.
