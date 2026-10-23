//! A deliberately small provider: UTF-8 text to a self-contained HTML page.
//! Recast owns discovery, the settings UI, job scheduling and final filenames.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    action: String,
    source: PathBuf,
    workspace: PathBuf,
    target: Option<String>,
    #[serde(default)]
    settings: BTreeMap<String, String>,
    output: Option<PathBuf>,
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|e| format!("Cannot read file: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Expected a regular file.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("File exceeds this example’s 1 MiB limit.".into());
    }
    Ok(bytes)
}

fn text(bytes: &[u8]) -> Result<&str> {
    let value = std::str::from_utf8(bytes).map_err(|_| "Use a UTF-8 text file.")?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err("This file contains binary or unsupported control characters.".into());
    }
    Ok(value.trim_start_matches('\u{feff}'))
}

fn escaped(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn render(title: &str, content: &str, theme: &str) -> String {
    let (background, foreground, scheme) = match theme {
        "Dark" => ("#1b1b1b", "#eeeeee", "dark"),
        _ => ("#fafafa", "#222222", "light"),
    };
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">\n\
         <title>{}</title>\n<style>\n\
         :root {{ color-scheme: {scheme}; background: {background}; color: {foreground}; }}\n\
         body {{ margin: 0 auto; padding: 2rem; max-width: 80ch; }}\n\
         pre {{ white-space: pre-wrap; overflow-wrap: anywhere; font: 1rem/1.6 monospace; }}\n\
         code {{ font: inherit; }}\n</style></head>\n<body><pre><code>{}</code></pre></body></html>\n",
        escaped(title),
        escaped(content),
    )
}

fn cancelled(request: &Request) -> Result<()> {
    if request.workspace.join("cancel").exists() {
        Err("Cancelled.".into())
    } else {
        Ok(())
    }
}

fn progress(request: &Request, fraction: f64, phase: &str) -> Result<()> {
    cancelled(request)?;
    // The host tolerates a partially read progress file and checks again.
    fs::write(
        request.workspace.join("progress.json"),
        json!({"fraction": fraction, "phase": phase}).to_string(),
    )
    .map_err(|e| format!("Cannot report progress: {e}"))
}

fn run(request: &Request) -> Result<Value> {
    if request.protocol_version != 1 {
        return Err("Unsupported provider protocol.".into());
    }
    if !request.source.is_absolute() || !request.workspace.is_absolute() {
        return Err("Source and workspace paths must be absolute.".into());
    }
    cancelled(request)?;
    let bytes = read_bounded(&request.source, MAX_TEXT_BYTES)?;
    let content = text(&bytes)?;
    match request.action.as_str() {
        "inspect" => Ok(json!({"protocolVersion": 1, "input": "text", "targets": ["html"]})),
        "convert" => {
            if request.target.as_deref() != Some("html") {
                return Err("This example only outputs HTML.".into());
            }
            if request.settings.keys().any(|key| key != "theme") {
                return Err("Unknown conversion setting.".into());
            }
            let theme = request
                .settings
                .get("theme")
                .map(String::as_str)
                .unwrap_or("Light");
            if !matches!(theme, "Light" | "Dark") {
                return Err("Choose the Light or Dark theme.".into());
            }
            let output = request.output.as_ref().ok_or("Missing output path.")?;
            if !output.is_absolute() {
                return Err("Output path must be absolute.".into());
            }
            // Recast supplies an empty temporary file. Never write the source.
            let source_path = request.source.canonicalize().map_err(|e| e.to_string())?;
            let output_path = output.canonicalize().map_err(|e| e.to_string())?;
            if source_path == output_path || !output.is_file() {
                return Err("Output must be a separate temporary file.".into());
            }
            progress(request, 0.2, "Rendering text")?;
            let title = request
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            let html = render(&title, content, theme);
            if read_bounded(&request.source, MAX_TEXT_BYTES)? != bytes {
                return Err("The source changed during conversion. Try again.".into());
            }
            progress(request, 0.7, "Writing HTML")?;
            let mut file = File::create(output).map_err(|e| e.to_string())?;
            file.write_all(html.as_bytes()).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            // Verify what was written before telling Recast to publish it.
            if fs::read(output).map_err(|e| e.to_string())? != html.as_bytes() {
                return Err("The HTML output could not be verified.".into());
            }
            progress(request, 1.0, "HTML ready")?;
            Ok(json!({"protocolVersion": 1}))
        }
        _ => Err("Unsupported provider action.".into()),
    }
}

fn main() {
    let result = (|| -> Result<bool> {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--request")) {
            return Err("Usage: recast-example-worker --request <absolute-request.json>".into());
        }
        let path = PathBuf::from(args.next().ok_or("Missing request path.")?);
        if args.next().is_some() || !path.is_absolute() {
            return Err("Expected one absolute request path.".into());
        }
        let request: Request = serde_json::from_slice(&read_bounded(&path, 64 * 1024)?)
            .map_err(|e| format!("Invalid request: {e}"))?;
        if !request.workspace.is_absolute() || !request.workspace.is_dir() {
            return Err("Missing absolute workspace directory.".into());
        }
        let response = run(&request);
        let failed = response.is_err();
        let value = response.unwrap_or_else(|error| json!({"protocolVersion": 1, "error": error}));
        fs::write(request.workspace.join("response.json"), value.to_string())
            .map_err(|e| format!("Cannot write response: {e}"))?;
        Ok(failed)
    })();
    match result {
        Ok(false) => (),
        Ok(true) => std::process::exit(1),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
