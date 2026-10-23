# Recast extensions

Extensions add optional conversion formats through **Settings → Extensions**. Installed providers declare their inputs, outputs and settings; Recast adds the corresponding choices to the batch UI. Conversion runs locally and works offline after installation. Image, audio and video support remains part of Recast itself.

| Extension | Conversions | Engine |
| --- | --- | --- |
| [Text & Markdown](text/README.md) | UTF-8 TXT / Markdown → PDF or DOCX | Pandoc and Typst |
| [Office](office/README.md) | DOCX / DOC / ODT / RTF → PDF | LibreOffice |
| [Authoring example](example/README.md) | UTF-8 TXT → HTML | Small standalone Rust worker |

## Write an extension

The [working example and guide](example/README.md) covers building a local package, declaring a format and dropdown, inspecting inputs, converting, reporting progress, cancellation, and verification. It needs no separate conversion engine. The [provider protocol](PROVIDERS.md) defines runtime behavior; the [package format](PACKAGE_FORMAT.md) defines the archive and inventory.

Use **Install from file** to test a `.recast-extension` locally. No release or catalog entry is required. Native providers run with the user's permissions; local publisher identity is self-declared. Install only trusted packages. Checksums verify bytes, not authorship.

Recast verifies inventories, checksums, platform compatibility and minimum app version. Installed versions are immutable. Batches pin their provider version through conversion and retry, and a package in use cannot be removed. Group destinations must be supported by every file. The host publishes one collision-safe output per successful input and preserves existing files.

## Official distribution

Source and build recipes live here in `recast`. Downloadable packages are release assets in [recast-extensions](https://github.com/Boof2015/recast-extensions/releases), separate from app releases. **Check for extensions** reads published official releases compatible with the current device; drafts and prereleases are excluded. There is no background polling or automatic engine download when opening a file.

Run **Extension packages** manually on `main` to build official packs. Commits and pull requests do not trigger extension packaging. The [draft release workflow](../.github/workflows/extension-release.yml) validates all five platform packages and creates a draft in `Boof2015/recast-extensions`; it can also be run manually with a successful build run ID. The example is not part of official release builds.

Cross-repository uploads use the `EXTENSIONS_RELEASE_TOKEN` secret in `recast`, restricted to Contents read/write in `recast-extensions`. The workflow never publishes, marks Latest, or overwrites releases/assets. Test draft packages via local installation, then publish manually. Generated binaries and packages belong in `.backend-build/`, not Git or the main app bundle. Dependency licenses accompany each package.
