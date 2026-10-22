# Conversion provider protocol 1

The host discovers a `provider.json` in an installed package's payload. That file and the executable must be in the package's verified inventory. Packages without a provider descriptor can be stored but do not add formats. The highest installed SemVer for each package ID is used for new inspections; existing batches retain their exact package digest, settings, and executable through retries.

```json
{
  "protocolVersion": 1,
  "executable": "bin/example-worker",
  "inputs": [
    {"id": "markdown", "label": "Markdown", "kind": "documents", "extensions": ["md", "markdown"]}
  ],
  "targets": [
    {
      "id": "pdf", "label": "PDF", "description": "Ready to share",
      "extension": "pdf", "inputs": ["markdown"],
      "settings": [{"id": "page-size", "label": "Page size", "choices": ["A4", "Letter"], "default": "A4"}]
    }
  ]
}
```

Input kinds are `images`, `audio`, `video`, and `documents`. Extensions nominate candidates; the worker must inspect the actual source. Each inspection may offer a subset of declared targets. Recast intersects those offers across the group, so one incompatible file blocks that destination. Providers can add conversions for an existing kind. Core conversions keep their own implementation and controls. Protocol 1 supports select controls; arbitrary HTML and scripts cannot be injected into the UI.

## Requests and responses

Recast launches `<payload>/<executable> --request <absolute-request-path>` with its current directory set to a private temporary workspace. Arguments never go through a shell. The UTF-8 JSON request is:

```json
{
  "protocolVersion": 1,
  "action": "convert",
  "source": "/absolute/input.md",
  "workspace": "/private/temporary/workspace",
  "target": "pdf",
  "settings": {"page-size": "A4"},
  "output": "/chosen/folder/.recast-temporary.pdf"
}
```

For `inspect`, `target`, `settings`, and `output` are omitted. Write `response.json` in `workspace`, then exit:

```json
{"protocolVersion":1,"input":"markdown","targets":["pdf"]}
```

A successful conversion writes the requested output and returns `{"protocolVersion":1}` with exit status 0. Optional `warnings` is an array of short strings. Failures return `{"protocolVersion":1,"error":"Useful explanation."}` and a nonzero exit status. Response files must be regular files, at most 64 KiB. Inspect responses cannot invent undeclared inputs or targets.

For progress, replace `progress.json` with `{"fraction":0.65,"phase":"Rendering PDF"}`. Fractions are 0–1; phases are short plain text. Poll for `workspace/cancel` during work. Recast gives workers two seconds to stop cooperatively before terminating their process group (Unix) or job object (Windows). Inspection is limited to 30 seconds, conversion to ten minutes, and diagnostic output to 1 MiB.

The worker must validate its completed output and preserve its source. It writes only to the supplied temporary output; Recast then checks the file, syncs it, and publishes it using a collision-safe name. Recast removes temporary files after failure or cancellation. Do not produce multiple outputs, select a final output name, or overwrite a source. Providers are native programs with the user's permissions, not a security sandbox. Install only packages you trust.

## Lifecycle

Installation/removal emits `recast:extensions-changed`; open batches re-inspect their files when idle. Conversion validates the selection again before starting. A package used by a batch cannot be removed until the batch is cleared. Updating installs a new immutable version without changing an existing retry. Window close and application quit cancel inspections as well as conversions and wait for worker cleanup.

See [Text & Markdown](text/README.md) for a working adapter and cross-platform package checks.
