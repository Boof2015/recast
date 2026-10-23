# Office extension

The optional **Office** package uses LibreOffice 26.8.1 to convert **DOCX → PDF** locally. It retains the source document's page sizes, orientation, margins, headers and footers. There is no replacement paper-size control. Images are exported without deliberate resolution reduction or lossy recompression. PDF hyperlinks and bookmarks are retained where supported by LibreOffice; review comments are omitted with a result warning.

This is the first route of one shared Office backend. DOC/ODT/RTF, presentations, spreadsheets and editable-format conversions are planned; they are not enabled by this package yet. PDF input is not supported.

## Installation and use

Install the package for your platform through **Recast → Settings → Extensions → Install from file**, or download a published official package from that page. No system LibreOffice installation is needed. Each conversion uses a private, temporary profile and ends its worker process afterwards; it does not attach to a user's open LibreOffice session.

DOCX and Markdown/TXT can share a PDF batch when both extensions are installed. The text extension's paper-size control applies to the text inputs; DOCX keeps its existing page layout. Each source produces its own PDF, and Recast preserves originals and existing output files.

LibreOffice's rendering can differ from Microsoft Word, especially with uncommon features or missing fonts. The package retains LibreOffice's fonts and adds pinned CJK, Devanagari, symbol and emoji fallback fonts. Installed fonts can also affect layout. Test important documents in the intended PDF reader.

## Supported boundaries

- Standard, unencrypted DOCX up to 256 MiB; at most 10,000 ZIP parts and 512 MiB expanded content. XML parts are limited to 32 MiB.
- Tracked changes must be accepted or rejected first. Macro-enabled files, embedded Office/OLE objects, external image/template relationships, linked-content fields and imported document chunks receive an explanation before conversion.
- Ordinary hyperlinks are allowed. Macros, Python scripting and OLE automation are disabled in the private profile.
- A render has a five-minute limit. Cancellation terminates and reaps the engine; the host also owns a process group or Windows job object for forced cleanup.
- ZIP parts, XML, internal relationships and source changes are checked. Generated PDFs are parsed and their page-content streams decoded before publication. These checks do not guarantee pixel-identical reproduction of every Word feature.

## Building

Use Python 3.12+, Rust 1.98.1 and native platform tooling. macOS requires `hdiutil`; Windows uses `msiexec /a` to extract the pinned MSI without installing the suite; Linux uses `dpkg-deb`. Linux runtime libraries are listed in the manual extension workflow.

```sh
cargo test --locked --manifest-path extensions/office/worker/Cargo.toml
python3 extensions/office/build.py --output extension-artifacts
python3 -m pip install pypdf==6.21.2
python3 extensions/office/check.py --package extension-artifacts --output office-checks
```

The builder verifies upstream checksums, retains license notices, stages regular files, checks the platform runtime and produces a `.recast-extension` archive. On macOS, the redundant `urelibs` SDK alias is omitted; runtime `fundamentalrc` uses `Frameworks` directly. Other required aliases become ordinary files, as required by the extension format. `--stage-only` prepares a local payload for development without creating an archive.

Run **Extension packages** manually and select **office** to build all five platforms. The workflow also checks real document exports independently with `pypdf`. A successful main-branch run feeds the existing unpublished draft-release workflow in `recast-extensions`. The workflow remains manual; ordinary commits do not build extensions. Publishing remains manual after native platform review.

`fixtures.py` generates independent OOXML with mixed A5/Letter orientations, headers, footers, tables, an embedded image, a link, footnote, equation and multilingual text. Checks cover output content/geometry, malformed and unsupported documents, source preservation, and cancellation. For host installation/publication checks, the opt-in Rust test uses `RECAST_TEST_OFFICE_EXTENSION`, `RECAST_TEST_TEXT_EXTENSION`, and `RECAST_TEST_DOCX`.

The vendored `licenses/alloc-stdlib-0.2.4.txt` is the BSD license from that crate's recorded source revision: https://github.com/dropbox/rust-alloc-no-stdlib/blob/ae42d22078b98549e987d2f03d12df7b984fde47/LICENSE. The crate archive omits this repository-root file, so the notices collector includes the version-specific copy. Other worker notices come from the exact Cargo lockfile.
