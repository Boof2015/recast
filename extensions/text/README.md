# Text & Markdown

Optional offline conversion of UTF-8 Markdown and plain text to PDF and DOCX. The only output setting is A4 or Letter paper. One source produces one file through Recast's normal batch, cancellation, retry, and safe naming workflow.

## Build and check

Run from the repository root with Rust 1.98.1 and Python 3.12+:

```sh
cargo fetch --locked --manifest-path extensions/text/worker/Cargo.toml
python3 extensions/text/build.py --output extension-artifacts
python3 -m pip install pypdf==6.21.2
python3 extensions/text/check.py --package extension-artifacts --output .backend-build/text-checks
```

Install the resulting `.recast-extension` through Settings → Extensions → Install from file. Building downloads only the pinned assets in `dependencies.lock.json`; installed conversion never downloads document content, fonts, or engines. Check outputs and reports are local artifacts, not committed binaries.

The **Extension packages** workflow builds and checks Apple Silicon, Intel Mac, Windows x64, Linux x64, and Linux ARM64. Successful main-branch builds feed the existing **Draft extension release** workflow. That uploads a draft to `Boof2015/recast-extensions`, leaves it unpublished and does not mark it latest. Publish only after reviewing the platform checks and testing installation. Rebuilding the same published/draft version does not replace its assets; increment the extension version for changed packages.

## Engine and layout

Pandoc 3.12.1's official WASM artifact runs in Wasmtime 50, with Typst 0.15.1 rendering PDF. The builder precompiles WASM for the explicit baseline target, so user conversions do not compile an engine or assume the build machine's optional CPU instructions. The worker and compiled module must stay together. This avoids the newer macOS requirement of upstream native Pandoc binaries; Mac workers are built for macOS 13 and audited for deployment target, architecture, and system-only dynamic dependencies. An actual macOS 13 machine remains a separate runtime check.

Fonts are pinned Noto Sans, Mono, CJK, Arabic, Devanagari, Symbols, Emoji, and STIX Two Math. PDF rendering ignores system fonts and rejects missing glyphs. DOCX contains editable content and uses the reader application's fonts. `document.typst` adapts Pandoc's standard template with the bundled font families; its upstream copyright and dual GPL/BSD license are included with the Pandoc notices. Pandoc itself is GPL-2.0-or-later; its source is at `https://github.com/jgm/pandoc/tree/3.12.1` and its WASM build instructions are in that source tree. The package records exact binary/font source URLs and digests in `licenses/SOURCES.json`, and includes upstream engine/font licenses plus locked Rust dependency notices. The Rust adapter and build recipes live alongside this file.

Markdown is parsed to Pandoc's AST, checked, then written as DOCX or Typst. Only relative local PNG/JPEG images are staged into the private workspace. WASI sees that workspace alone; document-supplied raw code, custom Typst attributes, external templates, citation resources, and remote images are rejected. Typst is restricted to the staged root. Supported front matter is title, subtitle, author, date, and abstract. Plain text bypasses Markdown interpretation.

Limits: 16 MiB UTF-8 text, 64 MiB per image, 128 MiB images total. Animated images, unsupported raw markup, missing assets, unsupported PDF glyphs, and renderer warnings fail the file. Sources and referenced images are checked again before publication so changes during conversion do not silently publish an inconsistent document. DOCX input, HTML input, PDF input, and Office layout preservation are outside this pack.

`check.py` independently reads PDF with pypdf and DOCX with ZIP/XML, checks layout dimensions, text, image/footnote relationships, multipage output, literal text, and error/cancellation behavior. The host also has an opt-in actual-package test:

```sh
RECAST_TEST_TEXT_EXTENSION=/absolute/path/package.recast-extension \
  cargo test --locked --manifest-path src-tauri/Cargo.toml \
  packaged_text_provider -- --ignored
```
