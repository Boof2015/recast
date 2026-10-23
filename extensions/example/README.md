# A small Recast extension

This working example converts **UTF-8 `.txt` → `.html`**. A **Theme** dropdown chooses a light or dark page. Text stays literal, including indentation, Unicode, and HTML-like markup. The page has no scripts or external resources. Inputs are limited to 1 MiB; binary data and unsupported control characters are rejected.

The worker is a small Rust executable with no external conversion engine. Python and Rust are needed to build it, but neither is needed on the machine where the extension is installed.

## Build and try it

From the Recast repository root, with Python 3.12+ and Rust installed:

```sh
python3 extensions/example/build.py
```

The script prints the `.recast-extension` path under `.backend-build/example-extension/packages/`. It builds for the current OS and architecture, collects dependency notices, stages the provider, and verifies the archive. The first build may download Rust dependencies. No engines are downloaded.

Use **Settings → Extensions → Install from file** in Recast, select that package, and install it. Add [sample.txt](sample.txt) to a batch. **HTML** appears under Documents; choose it, select a theme, and convert. Open the resulting HTML in a browser. Existing HTML files are preserved with a numbered output name. The example can coexist with Text & Markdown.

Building again never overwrites an existing package. Use a fresh output folder, for example:

```sh
python3 extensions/example/build.py --output .backend-build/example-second-build
```

To test the actual package, substitute the path printed by the build:

```sh
python3 extensions/example/check.py <package-path>
```

This checks inspection, both themes, literal text/HTML escaping, Unicode, defaults, invalid inputs/settings, cancellation, and source preservation in temporary directories. It does not install into your Recast settings.

The app also has an opt-in integration test that installs into temporary app data, discovers the provider, converts through Recast, and checks collision handling and removal. After preparing the app's bundled backends, run this on macOS/Linux:

```sh
RECAST_TEST_EXAMPLE_EXTENSION="/absolute/package-path" cargo test --locked \
  --manifest-path src-tauri/Cargo.toml packaged_example_provider -- --ignored
```

In PowerShell, set `$env:RECAST_TEST_EXAMPLE_EXTENSION = 'C:\absolute\package-path'`, then run the same Cargo command on one line without the environment assignment or backslash.

## How it adds a conversion

1. [provider.json](provider.json) declares input extensions, the Documents group, HTML output, and the Theme choices. Recast builds the format chooser and controls from this descriptor.
2. [worker/src/main.rs](worker/src/main.rs) receives `--request <absolute-json-path>`. On `inspect`, it checks the content and reports available targets. The filename extension only nominates a candidate; it does not prove that the file is supported.
3. On `convert`, the worker validates the target and settings, renders the page, checks that the source has not changed, writes only the temporary output supplied by Recast, and verifies that output. It writes progress to `progress.json` and checks the workspace's `cancel` file between stages.
4. The worker writes `response.json` and exits. Recast publishes the output under a collision-safe name, reports results, and cleans up temporary files. One input produces one output.

The package metadata in [build.py](build.py) identifies the extension and platform. The archive also includes the executable, descriptor, and dependency licenses. See the complete [provider protocol](../PROVIDERS.md) and [package format](../PACKAGE_FORMAT.md) for limits, timeouts, responses, version handling, and trust boundaries.

## Adapt it

Change the package ID/name/publisher in `build.py`, the worker behavior, and `provider.json` together. Use your own package ID rather than `example.text-html` or `recast.*`; increment the package version when distributing changes. Keep stable input, target, and setting IDs. Include licenses for everything you distribute.

Build natively for each platform you intend to support. Windows uses an `.exe` entry point; the recipe adjusts the descriptor. macOS builds target macOS 13. Linux binaries inherit the build machine's libc baseline, so build on a suitably old distribution and verify on the systems you support. A platform label alone does not establish compatibility. The example is not included in official extension release workflows.

Providers can use other languages, but must ship a directly executable entry point and any required runtime; Recast does not invoke a shell or assume Python/Node is installed. Runtime paths must be relative to the installed payload, not your checkout. For a slower engine, report progress, poll cancellation during work, and validate the completed conversion before reporting success.

Extensions run native code with the user's permissions. Package checksums detect changed bytes; they do not sandbox the executable or verify a local publisher's identity. Install packages from authors you trust. Local installation needs no GitHub release; the built-in online catalog currently lists only published official Recast extensions.
