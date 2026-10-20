# Recast

A local file converter, built with Tauri 2, React, TypeScript, and Rust.

Recast converts **still PNG, JPEG, WebP, BMP, TIFF, AVIF, and GIF**, **animated GIF ↔ animated WebP**, and **PCM WAV, FLAC, ALAC, MP3, AAC-LC, Opus, and Ogg Vorbis**, including same-format conversion. **H.264 MP4 video output** is also available. Images, audio, and video can share a batch with independent group settings. Everything runs locally through bundled converters.

## Images

Settings follow the output: JPEG has quality, WebP and AVIF have quality/lossless, PNG and TIFF use lossless compression, and BMP writes uncompressed 24-bit files. JPEG and BMP fill transparency with a shared background color, white by default, selected through Recast's built-in picker. Still-image outputs support resizing; BMP omits metadata after color normalization, while the other applicable targets offer retention of supported metadata. AVIF supports up to 12-bit color. GIF offers palette size and dithering, preserves cut-out transparency, and rounds animation timing to 10 ms.

TIFF accepts classic TIFF and BigTIFF, either byte order, with unsigned samples up to 16 bits. Output is ordinary single-page TIFF with Deflate compression and preserved transparency. Resize and Keep metadata are its only settings; XMP/IPTC, color profiles, and common TIFF tags can be retained, but camera EXIF is not fully carried over. Multipage/layered TIFF, raw camera containers, floating-point samples, and unsupported compression receive an explicit explanation.

Animated GIF/WebP inputs only offer destinations that retain animation. Animated PNG/AVIF, multi-image BMPs, and BMP wrappers containing embedded PNG/JPEG are rejected. Explicit frame selection and video-to-animation are later work.

## Audio

Audio input accepts PCM WAV (8/16/24/32-bit integer and 32/64-bit float), FLAC (16/24/32-bit), ALAC in M4A/MP4 containers (16/24-bit), MP3, AAC-LC in M4A/MP4 or raw ADTS files, and Opus/Vorbis in Ogg containers. WAV output supports those PCM representations; **FLAC and ALAC output currently support 16-bit and 24-bit PCM**. Float, 8-bit, and 32-bit PCM/lossless sources require an explicit compatible sample format for these destinations; Recast never silently reduces them to 24-bit.

ALAC writes Apple Lossless audio in `.m4a` files. This build rejects 20/32-bit ALAC, unsupported channel layouts, and containers with multiple audio streams or video; it does not silently reduce precision, remap channels, or discard tracks. ALAC output preserves source rates up to 384 kHz; higher rates require an explicit compatible rate or WAV. M4A describes the container: ALAC and AAC files sharing that extension are identified by their actual codec.

MP3 uses bundled LAME, with **variable bitrate quality** (V2 by default) or **constant bitrate**. It retains mono/stereo and supports 8, 11.025, 12, 16, 22.05, 24, 32, 44.1, and 48 kHz; unsupported source rates need an explicit compatible choice. Available constant bitrates depend on the selected rates across the group. Recast never silently changes the rate/bitrate or downmixes surround audio. PCM bit depth and dithering do not apply to MP3 encoding.

MP3, AAC, Opus, and Vorbis have no source PCM bit depth. When lossy inputs are present, the sample-format default is labelled **Auto**: lossy audio becomes **32-bit float WAV** or **24-bit PCM FLAC/ALAC**, while existing PCM/lossless sources retain their own formats where supported. Explicit choices remain available. Neither decoding MP3 into a lossless format nor increasing bitrate restores lost detail; MP3-to-MP3 conversion re-encodes and adds another lossy generation.

MP3 checks complete frame boundaries, consistent rate/channel count, and declared Xing frame counts before conversion. Export checks the full decoded result and exact gapless sample count. Files without delay/padding metadata retain all decoded samples; Recast does not guess a trim. Free-format MP3, changing stream parameters, concatenated stream headers, and malformed/incomplete frames are unsupported. Lossy encoding can create peaks above full scale; these are reported without normalization. Converting such peaks to integer PCM follows the existing clipping-consent policy.

AAC writes **AAC-LC in `.m4a` files**, using a target bitrate (192 kbps by default), Keep source or an explicit sample rate, and optional text metadata. Actual bitrate varies with the audio. It supports the thirteen standard rates from 7,350 to 96,000 Hz and standard mono through 7.1 layouts; incompatible rates, bitrates, and channel mappings require an explicit choice. PCM depth and dither do not apply to AAC encoding. M4A output uses sample-accurate edit durations and validates the fully decoded sample count before publication.

AAC-LC input includes independently encoded Apple M4A and raw ADTS `.aac` files. Raw AAC retains all decoded samples because it has no reliable gapless trim metadata. Truncated ADTS frames, changing stream parameters, multi-block/PCE ADTS, and containers with multiple audio tracks or video are rejected. **HE-AAC/HE-AACv2 remain unsupported for now**: independent checks found a timing offset in the bundled decoder, and these files receive a clear explanation. Other AAC profiles and LATM/LOAS are also outside this increment. Re-encoding lossy audio adds another lossy generation; decoding it to PCM/lossless does not restore lost detail.

Opus writes **Ogg `.opus` files**, with variable bitrate targeting 128 kbps by default. Output is always **48,000 Hz**, with resampling clearly shown in the controls; rate and PCM-depth settings remembered from other formats do not alter this path. Bitrate is for the complete stream, with choices from 16 to 512 kbps (up to 256 kbps for mono). Standard mono, stereo, 3.0, quad, 5.0, 5.1, 6.1, and 7.1 channel layouts are preserved without downmixing.

Opus input is identified by content in Ogg containers, including `.opus` and `.ogg` names. Header gain, pre-skip, and final padding are honored. Complete page checksums, sequence/continuation checks, and sample timing validation prevent damaged streams from being silently shortened; every export is fully decoded and its sample count checked. Chained/multiplexed Ogg streams and nonstandard/ambisonic mapping families are unsupported. Opus in WebM/MP4 belongs to later container work. Text metadata, lossy-input defaults, clipping consent, safe names, cancellation, and mixed batches use the existing workflow.

Vorbis writes **Ogg `.ogg` files** using bundled libvorbis. Controls are **quality −1 to 10** (5 by default), **Keep source** or an explicit sample rate, and **Keep metadata**. Higher quality generally produces larger files; variable bitrate depends on the audio, sample rate, and channel count. Supported output layouts match Opus, without automatic downmixing or remapping. Source rates above 200,000 Hz require an explicit compatible rate; the selectable rates span 7,350–192,000 Hz. PCM depth and dithering do not apply to Vorbis encoding.

Vorbis input is identified by content. The shared Ogg checks reject corrupt, truncated, chained, and multiplexed streams. Full source decoding is checked against the final sample position before resampling, and every export is decoded and checked for exact length. Cropped streams with unusual initial timing remain unsupported rather than guessing a trim. A checked FFmpeg correction preserves short files whose audio fits on one Ogg page; independent Xiph fixtures and decoding verify timing and channel content.

- **Keep source** preserves each file's sample rate and sample format where the destination supports them. Explicit selections apply to the whole audio group.
- One to eight channels are retained; files with more than two channels need a recognized channel layout. Channel mixing is not implemented.
- **Dithering: Auto** applies unshaped triangular dither once at final integer quantization when reducing precision or resampling. Unchanged integer samples and exact widening bypass it. **Off** is also available.
- **Allow clipping** permits integer output when decoded or resampled peaks exceed its range. Without consent, affected files fail with an **Allow clipping & retry** action that preserves completed outputs. Recast does not normalize or limit the signal; compatible float WAV retains out-of-range values.
- **Keep metadata** carries supported ordinary text tags. Artwork, loudness-analysis tags, and production metadata are omitted. Output details report sample properties and whether dithering was applied.

The planned ordinary audio formats are now implemented within the limits above. HE-AAC timing and the remaining release checks are still tracked.

## Video

The first video destination is **MP4 with H.264**. **Encoding** defaults to Automatic, which tries available hardware; Software explicitly uses the bundled x264 encoder. Other controls are **Quality** (85 by default), **Resolution** (Original, 1080p, 720p, 480p), **Frame rate** (Original, 24, 30, 60), and **Keep metadata**. Resolution fits inside the chosen landscape/portrait size without upscaling. Original retains each file's presentation timing, including variable frame rates; an explicit rate duplicates/drops frames to produce that rate. Rotation is applied to the pixels. Quality controls compression, not a promised file size.

Supported inputs are single-video-track, progressive, 8-bit SDR files in MP4/MOV, MKV/WebM, or AVI, using the enabled H.264, HEVC, VP8/VP9, MPEG-4, or MJPEG decoders. They must have square pixels, even dimensions up to 8192, right-angle rotation, a finite duration up to 24 hours, and rates up to 240 fps. HDR, higher bit depth/alpha (including ordinary ProRes), interlacing, unusual geometry, chapters, additional video/attachment/data tracks, and unsupported subtitles receive an explanation. This increment does not silently flatten or discard those features. Full frame validation currently has a ten-million-frame limit.

Silent video remains silent. All supported audio tracks (up to eight) are retained: **AAC-LC packets are copied**, while supported PCM, FLAC, 16/24-bit ALAC, MP3, Opus, and Vorbis tracks are encoded to AAC-LC. Source sample rates and recognized channel layouts must fit AAC; there is no silent resampling or downmixing. HE-AAC remains blocked. Existing MP4 text subtitles are copied, with language and track dispositions retained even when ordinary metadata is off. Other subtitle formats need a later preservation path.

The source and completed output are fully decoded and their frame timing, visible duration, audio timing, dimensions, and track counts checked before publication. Video reports preparation, source checking, encoding, and output checking progress. Cancellation stops the worker and removes temporary output; retries keep successful files. Originals and existing outputs are preserved.

Hardware encoding paths are Apple VideoToolbox on macOS, NVIDIA NVENC / Intel Quick Sync / AMD AMF on Windows, and NVIDIA NVENC / VA-API on Linux (including Intel/AMD render devices). Automatic proves availability with a short encode and software decode, then validates the complete actual output before publishing it. A hardware failure or invalid output retries once with software; cancellation never retries. Progress and file details identify the encoder used, including fallback. Linux tries available render nodes; drivers and device permissions are required. The NVIDIA headers require drivers 550.54.14+ on Linux or 551.76+ on Windows. Machines without supported hardware still work through software.

Quality is mapped separately for each encoder; equal slider values do not promise equal quality or file size across hardware. Intel Mac VideoToolbox uses a bitrate derived from dimensions, frame rate, and quality because its constant-quality mode is unavailable. This increment accelerates **encoding**. Source decoding, filters, and complete source/output validation still run in software, so total speed depends on those steps too.

Target-size encoding for upload limits follows; broader container/remux support, video-to-audio, and range/frame selection remain planned. Text/document scope will be discussed after video.

## Batches

Batches preserve originals and existing outputs, produce one output per successful input, report per-file results, support cancellation, and retry unfinished files. Destinations must work for the entire group.

## Development

The macOS app and bundled workers target macOS 13+ using source-built codecs. The Apple Silicon build is checked on macOS 26; macOS 13 itself remains unverified.

Build prerequisites are Node.js 24+, Rust (CI pins 1.98.1), Python 3.12+, CMake, Make, a C/C++ compiler, pkg-config, and the [Tauri system prerequisites](https://v2.tauri.app/start/prerequisites/). NASM enables x86 optimizations in the image codecs and x264. On macOS, install Xcode command-line tools and ensure the modern Python is on PATH as `python3`. On Windows, build both converters in MSYS2 UCRT64 with `/usr/bin/python`, then use MSVC Rust for the app; the [platform workflow](.github/workflows/platform-checks.yml) lists the build packages.

```sh
npm ci
npm run backend:prepare
npm run desktop
```

`backend:prepare` builds both the image worker and FFmpeg/ffprobe from pinned, checksum-verified sources. To rebuild only one, use `npm run backend:image` or `npm run backend:audio`. Windows equivalents are `/usr/bin/python scripts/prepare-image-backend.py` and `/usr/bin/python scripts/prepare-audio-backend.py` in MSYS2 UCRT64.

Choose or drop supported images, audio, or video files, select an output for each group, adjust settings, and convert. Outputs go beside their sources by default; a different folder can be selected. Existing names, including the original during same-format conversion, get a numbered alternative. JPEG outputs use `.jpg`; TIFF outputs use `.tiff` (both `.tif` and `.tiff` inputs are recognized by content). Click a completed output to reveal it in its folder.

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

Source versions and hashes are recorded in [image-backend.lock.json](scripts/image-backend.lock.json) and [audio-backend.lock.json](scripts/audio-backend.lock.json). The minimal FFmpeg build enables the supported audio/video codecs and containers without network access, with statically linked, source-built LAME 4.0, libopus 1.6.1, libogg 1.3.6, libvorbis 1.3.7, and x264 at pinned commit `b35605a`. The separate FFmpeg worker is now built with GPL support for x264; its GPL notice, x264 license, exact source URLs/hashes, configurations, and build recipe accompany the bundle. Checked FFmpeg source corrections accumulate trailing MP3 padding across final packets and account for Vorbis initial overlap when all audio fits on one Ogg page. Generated resources carry build manifests and license notices and are ignored by Git.

The platform workflow covers Windows x86_64, Linux x86_64/ARM64, and both Mac architectures. The image and WAV/FLAC/ALAC/MP3/AAC-LC workflow passes [all five targets at `b26b350`](https://github.com/Boof2015/recast/actions/runs/39210227739), including 108 Rust tests before and after packaging and 21 build/audit tests on each target. Local UI checks and independent CoreAudio decoding also pass. Older macOS runtime checks, native desktop/accessibility checks, and distribution signing remain separate release work.

Opus passes all five targets at `3eca4fb` in [run 39321173154](https://github.com/Boof2015/recast/actions/runs/39321173154): 117 Rust tests before and after packaging, 22 Python checks, formatting/Clippy, release builds, and package audits per target. No platform fixes were needed. Local UI checks and direct-libopus verification also pass.

Vorbis passes local Apple Silicon checks: 124 Rust tests, including a full repeat against the packaged resources, 23 Python checks, formatting/Clippy, frontend/release builds, package audits, both-theme browser UI checks, and independent Xiph decoding. The rebuilt app is 34.28 MiB. In [run 39330480422 at `ad2e9d5`](https://github.com/Boof2015/recast/actions/runs/39330480422), Windows and both Macs pass all 124 tests before and after packaging and all 23 Python checks. Both Linux jobs failed during FFmpeg configuration because libvorbis's static-link metadata omitted the system math library. The build now supplies that dependency; an isolated Linux ARM64 check reproduces the original failure and passes the corrected worker build, dependency audit, and real Vorbis conversions. Vorbis passes all five targets at `74aba12` in [run 39352703442](https://github.com/Boof2015/recast/actions/runs/39352703442): 124 Rust tests before and after packaging, 23 Python checks, formatting/Clippy, release builds, and package audits per target. Both Linux jobs pass with the libm correction.

MP4 passes local Apple Silicon checks: **133 Rust tests against the packaged resources**, **24 Python checks**, formatting/Clippy, frontend/release builds, package audits, and both-theme browser UI checks. Independent Apple decoding verifies output frame count, presentation timestamps, visible duration, dimensions, and rotated picture content. The app is 46.24 MiB; evidence is in `.backend-build/artifacts/mp4-macos-arm64/`. MP4 also passes all five CI targets at `687ba8c` ([run 39364919888](https://github.com/Boof2015/recast/actions/runs/39364919888)): 133 Rust tests before and after packaging, 24 Python checks, formatting/Clippy, release builds, and package audits per target.


Hardware encoding is locally verified on Apple Silicon: **137 Rust tests against the packaged workers**, **26 Python checks**, formatting/Clippy, frontend/release/package checks, and browser interaction checks. Required-hardware tests exercise VideoToolbox with CFR/VFR, rotation, subtitles, audio conversion, quality extremes, and explicit frame rate; independent Apple decoding verifies timing, visible duration, and picture orientation. The app is 46.40 MiB. A separate Linux ARM64 build verifies both compiled hardware paths, system-only dynamic dependencies, clean failures without GPU devices, and intact software conversion. Evidence is in `.backend-build/artifacts/hardware-macos-arm64/` and `hardware-linux-arm64/`. A Windows cross-compile also links all three hardware modules with only system DLL imports. Native Windows/Intel Mac packaging and representative NVIDIA/Intel/AMD hardware operation remain unverified for this increment; CI does not replace testing real GPU devices. No hardware-decoding support or target-size mode is claimed yet.
