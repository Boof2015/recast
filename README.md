# Recast

A local file converter, built with Tauri 2, React, TypeScript, and Rust.

Recast converts **still PNG, JPEG, WebP, BMP, TIFF, AVIF, and GIF**, **animated GIF ↔ animated WebP**, and **PCM WAV, FLAC, ALAC, MP3, and AAC-LC**, including same-format conversion. Images and audio can share a batch with independent group settings. Everything runs locally through bundled converters.

## Images

Settings follow the output: JPEG has quality, WebP and AVIF have quality/lossless, PNG and TIFF use lossless compression, and BMP writes uncompressed 24-bit files. JPEG and BMP fill transparency with a shared background color, white by default, selected through Recast's built-in picker. Still-image outputs support resizing; BMP omits metadata after color normalization, while the other applicable targets offer retention of supported metadata. AVIF supports up to 12-bit color. GIF offers palette size and dithering, preserves cut-out transparency, and rounds animation timing to 10 ms.

TIFF accepts classic TIFF and BigTIFF, either byte order, with unsigned samples up to 16 bits. Output is ordinary single-page TIFF with Deflate compression and preserved transparency. Resize and Keep metadata are its only settings; XMP/IPTC, color profiles, and common TIFF tags can be retained, but camera EXIF is not fully carried over. Multipage/layered TIFF, raw camera containers, floating-point samples, and unsupported compression receive an explicit explanation.

Animated GIF/WebP inputs only offer destinations that retain animation. Animated PNG/AVIF, multi-image BMPs, and BMP wrappers containing embedded PNG/JPEG are rejected. Explicit frame selection and video-to-animation are later work.

## Audio

Audio input accepts PCM WAV (8/16/24/32-bit integer and 32/64-bit float), FLAC (16/24/32-bit), ALAC in M4A/MP4 containers (16/24-bit), MP3, and AAC-LC in M4A/MP4 or raw ADTS files. WAV output supports those PCM representations; **FLAC and ALAC output currently support 16-bit and 24-bit PCM**. Float, 8-bit, and 32-bit PCM/lossless sources require an explicit compatible sample format for these destinations; Recast never silently reduces them to 24-bit.

ALAC writes Apple Lossless audio in `.m4a` files. This build rejects 20/32-bit ALAC, unsupported channel layouts, and containers with multiple audio streams or video; it does not silently reduce precision, remap channels, or discard tracks. ALAC output preserves source rates up to 384 kHz; higher rates require an explicit compatible rate or WAV. M4A describes the container: ALAC and AAC files sharing that extension are identified by their actual codec.

MP3 uses bundled LAME, with **variable bitrate quality** (V2 by default) or **constant bitrate**. It retains mono/stereo and supports 8, 11.025, 12, 16, 22.05, 24, 32, 44.1, and 48 kHz; unsupported source rates need an explicit compatible choice. Available constant bitrates depend on the selected rates across the group. Recast never silently changes the rate/bitrate or downmixes surround audio. PCM bit depth and dithering do not apply to MP3 encoding.

MP3 and AAC have no source PCM bit depth. When lossy inputs are present, the sample-format default is labelled **Auto**: lossy audio becomes **32-bit float WAV** or **24-bit PCM FLAC/ALAC**, while existing PCM/lossless sources retain their own formats where supported. Explicit choices remain available. Neither decoding MP3 into a lossless format nor increasing bitrate restores lost detail; MP3-to-MP3 conversion re-encodes and adds another lossy generation.

MP3 checks complete frame boundaries, consistent rate/channel count, and declared Xing frame counts before conversion. Export checks the full decoded result and exact gapless sample count. Files without delay/padding metadata retain all decoded samples; Recast does not guess a trim. Free-format MP3, changing stream parameters, concatenated stream headers, and malformed/incomplete frames are unsupported. Lossy encoding can create peaks above full scale; these are reported without normalization. Converting such peaks to integer PCM follows the existing clipping-consent policy.

AAC writes **AAC-LC in `.m4a` files**, using a target bitrate (192 kbps by default), Keep source or an explicit sample rate, and optional text metadata. Actual bitrate varies with the audio. It supports the thirteen standard rates from 7,350 to 96,000 Hz and standard mono through 7.1 layouts; incompatible rates, bitrates, and channel mappings require an explicit choice. PCM depth and dither do not apply to AAC encoding. M4A output uses sample-accurate edit durations and validates the fully decoded sample count before publication.

AAC-LC input includes independently encoded Apple M4A and raw ADTS `.aac` files. Raw AAC retains all decoded samples because it has no reliable gapless trim metadata. Truncated ADTS frames, changing stream parameters, multi-block/PCE ADTS, and containers with multiple audio tracks or video are rejected. **HE-AAC/HE-AACv2 remain unsupported for now**: independent checks found a timing offset in the bundled decoder, and these files receive a clear explanation. Other AAC profiles and LATM/LOAS are also outside this increment. Re-encoding lossy audio adds another lossy generation; decoding it to PCM/lossless does not restore lost detail.

- **Keep source** preserves each file's sample rate and sample format where the destination supports them. Explicit selections apply to the whole audio group.
- One to eight channels are retained; files with more than two channels need a recognized channel layout. Channel mixing is not implemented.
- **Dithering: Auto** applies unshaped triangular dither once at final integer quantization when reducing precision or resampling. Unchanged integer samples and exact widening bypass it. **Off** is also available.
- **Allow clipping** permits integer output when decoded or resampled peaks exceed its range. Without consent, affected files fail with an **Allow clipping & retry** action that preserves completed outputs. Recast does not normalize or limit the signal; compatible float WAV retains out-of-range values.
- **Keep metadata** carries supported ordinary text tags. Artwork, loudness-analysis tags, and production metadata are omitted. Output details report sample properties and whether dithering was applied.

Opus, Ogg Vorbis, and the planned video formats remain on the roadmap. Audio format coverage is not complete yet.

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

Choose or drop supported images, WAV, FLAC, ALAC, MP3, or AAC-LC files, select an output for each group, adjust settings, and convert. Outputs go beside their sources by default; a different folder can be selected. Existing names, including the original during same-format conversion, get a numbered alternative. JPEG outputs use `.jpg`; TIFF outputs use `.tiff` (both `.tif` and `.tiff` inputs are recognized by content). Click a completed output to reveal it in its folder.

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

Source versions and hashes are recorded in [image-backend.lock.json](scripts/image-backend.lock.json) and [audio-backend.lock.json](scripts/audio-backend.lock.json). The minimal FFmpeg build enables WAV/FLAC/ALAC/MP3/AAC and PCM processing without network access, with a statically linked, source-built LAME 4.0 encoder. A checked FFmpeg source correction accumulates trailing MP3 padding across final packets to preserve exact decoded lengths. Generated resources carry build manifests and license notices and are ignored by Git.

The platform workflow covers Windows x86_64, Linux x86_64/ARM64, and both Mac architectures. The image and WAV/FLAC/ALAC/MP3 workflow passes [all five targets at `5fe0055`](https://github.com/Boof2015/recast/actions/runs/39205740495), including 98 Rust tests before and after packaging and 21 build/audit tests on each target. The subsequent AAC-LC increment passes 108 Rust tests against the packaged Apple Silicon app, 21 Python checks, UI checks, and independent CoreAudio decoding; its five-platform CI run is pending. Older macOS runtime checks, native desktop/accessibility checks, and distribution signing remain separate release work.
