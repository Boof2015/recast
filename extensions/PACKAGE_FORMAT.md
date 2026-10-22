# Extension package schema 1

A `.recast-extension` is a ZIP archive with `manifest.json` at its root and all installed files under `payload/`. Use stored or Deflate compression. Archive symlinks, directory entries, special files, duplicate paths, path traversal, Windows device/stream paths, encrypted entries and undeclared files are rejected. Paths are portable ASCII, case-insensitively unique, with at most 240 characters. This initial restriction can be revised if a verified engine requires more.

```json
{
  "schemaVersion": 1,
  "package": {
    "id": "recast.text",
    "name": "Text & Markdown",
    "version": "0.1.0",
    "description": "Markdown to PDF and DOCX.",
    "publisher": "Recast",
    "platform": "macos-arm64",
    "minimumAppVersion": "0.1.0",
    "installedBytes": 123
  },
  "files": [
    {
      "path": "bin/pandoc",
      "bytes": 123,
      "sha256": "<64 lowercase hex characters>",
      "executable": true
    }
  ]
}
```

The example is illustrative, not an installable engine. Executable conversion packages also include `payload/provider.json`; see [the provider protocol](PROVIDERS.md). The installer validates that descriptor and its declared executable before making the package available. The builder computes file sizes, checksums and executable flags, and requires license files under `licenses/`. Manifest fields are strict; unknown fields are rejected. The maximum archive is 1 GiB, installed payload 4 GiB, manifest 8 MiB and inventory 30,000 files. Unix permissions are normalized to 0755 for executables and 0644 otherwise; privileges and ownership are not restored from ZIP metadata.

Platform names: `macos-arm64`, `macos-x86_64`, `windows-x86_64`, `linux-arm64`, `linux-x86_64`. Versions use SemVer. `minimumAppVersion` checks the host app version, not an executable-provider ABI. Engine recipes must separately establish OS compatibility; official packs must support Recast's declared baseline, including macOS 13. The measured upstream Pandoc ARM Mac binary requires a newer macOS and cannot be shipped unchanged.

## Build locally

Create a metadata JSON file containing the `package` fields above except `installedBytes`. Stage binaries, templates, fonts and licenses in a payload directory. Do not put the output directory inside that payload.

```sh
python3 extensions/shared/package.py build \
  --metadata .backend-build/text/metadata.json \
  --payload .backend-build/text/payload \
  --output .backend-build/extension-packages
python3 extensions/shared/package.py verify \
  .backend-build/extension-packages/recast.text-0.1.0-macos-arm64.recast-extension
```

File names are `<id>-<version>-<platform>.recast-extension`. No commands from the package or metadata execute during building or verification. Packages are installed by digest, preserving separate immutable versions; an installation receipt records local or official download provenance.

## Release metadata

Each release has a tag `<id>-v<version>` and an `extension-release.json` asset:

```json
{
  "schemaVersion": 1,
  "packages": [
    {
      "package": { "...": "same package summary as manifest.json" },
      "sha256": "<archive SHA-256>",
      "downloadBytes": 456,
      "url": "https://github.com/Boof2015/recast-extensions/releases/download/recast.text-v0.1.0/recast.text-0.1.0-macos-arm64.recast-extension"
    }
  ]
}
```

Release tooling generates this asset and `SHA256SUMS` after validating all five platform packages. The app requires metadata URLs and package URLs to belong to that exact published release. It picks the highest compatible version per extension, verifies the downloaded bytes, then checks the embedded summary against the release summary before installation.
