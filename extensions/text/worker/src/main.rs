mod engine;
use engine::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_TEXT: u64 = 16 * 1024 * 1024;
const MAX_ASSETS: u64 = 128 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    action: String,
    source: PathBuf,
    workspace: PathBuf,
    #[serde(default)]
    output: Option<PathBuf>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    settings: BTreeMap<String, String>,
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let file = File::open(path).map_err(|_| {
        format!(
            "Could not read {}.",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("Choose individual document files.".into());
    }
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err("This document or one of its images is too large.".into());
    }
    Ok(bytes)
}
fn source(path: &Path) -> Result<(String, String, Vec<u8>)> {
    let bytes = read(path, MAX_TEXT)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "Save this text file as UTF-8 before converting it.".to_string())?;
    if text
        .chars()
        .any(|c| c.is_control() && !['\r', '\n', '\t', '\u{c}'].contains(&c))
    {
        return Err("This file contains binary data, rather than ordinary text.".into());
    }
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let input = match extension.as_str() {
        "md" | "markdown" => "markdown",
        "txt" => "text",
        _ => return Err("Choose a Markdown or plain text file.".into()),
    };
    Ok((
        input.into(),
        text.trim_start_matches('\u{feff}')
            .replace("\r\n", "\n")
            .replace('\r', "\n"),
        bytes,
    ))
}
fn cancelled(work: &Path) -> Result<()> {
    if work.join("cancel").exists() {
        Err("Cancelled.".into())
    } else {
        Ok(())
    }
}
fn progress(work: &Path, fraction: f64, phase: &str) -> Result<()> {
    cancelled(work)?;
    let bytes = serde_json::to_vec(&json!({"fraction":fraction,"phase":phase})).map_err(error)?;
    fs::write(work.join("progress.json"), bytes).map_err(error)
}
struct Interrupt {
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Interrupt {
    fn new(engine: wasmtime::Engine, work: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let thread = thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if work.join("cancel").exists() {
                    engine.increment_epoch();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Interrupt {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct AssetContext<'a> {
    source: &'a Path,
    work: &'a Path,
    snapshots: Vec<(PathBuf, Vec<u8>)>,
    bytes: u64,
    text: String,
}
impl AssetContext<'_> {
    fn walk(&mut self, node: &mut Value) -> Result<()> {
        match node.get("t").and_then(Value::as_str).unwrap_or("") {
            "RawInline" | "RawBlock" => {
                let raw = node["c"][1].as_str().unwrap_or("").trim();
                if raw.starts_with("<!--") && raw.ends_with("-->") {
                    *node = if node["t"] == "RawBlock" {
                        json!({"t":"Plain","c":[]})
                    } else {
                        json!({"t":"Str","c":""})
                    };
                } else {
                    return Err("Raw HTML, TeX, and Typst content is not supported yet. Use ordinary Markdown formatting.".into());
                }
            }
            "Str" | "MetaString" => {
                if let Some(text) = node["c"].as_str() {
                    self.text.push_str(text);
                }
            }
            "Code" | "CodeBlock" => {
                if let Some(text) = node["c"][1].as_str() {
                    self.text.push_str(text);
                }
            }
            "Image" => {
                let reference = node["c"][2][0].as_str().ok_or("Invalid Markdown image.")?;
                let reference = percent_encoding::percent_decode_str(reference)
                    .decode_utf8()
                    .map_err(error)?;
                if reference.contains("://")
                    || reference.starts_with("data:")
                    || reference.starts_with("//")
                {
                    return Err("Remote images are not downloaded. Save the image locally and use its relative path.".into());
                }
                let relative = Path::new(reference.as_ref());
                if relative.is_absolute() || reference.contains('\\') || reference.contains(':') {
                    return Err("Use a relative path for this document's images.".into());
                }
                let path = self
                    .source
                    .parent()
                    .ok_or("The source folder is unavailable.")?
                    .join(relative);
                let bytes = read(&path, 64 * 1024 * 1024)?;
                self.bytes += bytes.len() as u64;
                if self.bytes > MAX_ASSETS {
                    return Err("This document's images exceed 128 MiB.".into());
                }
                let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                    "png"
                } else if bytes.starts_with(b"\xff\xd8\xff") {
                    "jpg"
                } else {
                    return Err("Document images currently need to be PNG or JPEG.".into());
                };
                // Refuse animated PNG rather than silently exporting one frame.
                if extension == "png" {
                    let mut offset = 8usize;
                    while offset + 12 <= bytes.len() {
                        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
                            as usize;
                        if &bytes[offset + 4..offset + 8] == b"acTL" {
                            return Err(
                                "Animated images are not supported in documents yet.".into()
                            );
                        }
                        if &bytes[offset + 4..offset + 8] == b"IDAT" {
                            break;
                        }
                        offset = offset.checked_add(size + 12).ok_or("Invalid PNG image.")?;
                    }
                }
                let staged = format!("asset-{}.{}", self.snapshots.len(), extension);
                fs::write(self.work.join(&staged), &bytes).map_err(error)?;
                self.snapshots.push((path, bytes));
                node["c"][2][0] = staged.into();
            }
            _ => {}
        }
        match node {
            Value::Array(items) => {
                if items.len() == 2
                    && items[0]
                        .as_str()
                        .is_some_and(|key| key.starts_with("typst:"))
                    && items[1].is_string()
                {
                    return Err("Custom Typst attributes are not supported.".into());
                }
                for item in items {
                    self.walk(item)?;
                }
            }
            Value::Object(items) => {
                for (key, value) in items {
                    if key.starts_with("typst:") {
                        return Err("Custom Typst attributes are not supported.".into());
                    }
                    self.walk(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn check_fonts(root: &Path, text: &str) -> Result<()> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root.join("fonts")).map_err(error)? {
        let path = entry.map_err(error)?.path();
        if [Some("ttf"), Some("otf")].contains(&path.extension().and_then(|s| s.to_str())) {
            files.push(fs::read(path).map_err(error)?);
        }
    }
    let fonts: Vec<_> = files
        .iter()
        .map(|bytes| ttf_parser::Face::parse(bytes, 0).map_err(error))
        .collect::<Result<_>>()?;
    for c in text.chars().filter(|c| {
        !c.is_whitespace() && !matches!(*c, '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}')
    }) {
        if !fonts.iter().any(|font| font.glyph_index(c).is_some()) {
            return Err(format!(
                "The bundled fonts cannot render '{c}' (U+{:04X}) yet.",
                c as u32
            ));
        }
    }
    Ok(())
}
fn run_typst(root: &Path, work: &Path) -> Result<()> {
    let diagnostics = work.join("typst.log");
    let mut command = Command::new(root.join(if cfg!(windows) {
        "bin/typst.exe"
    } else {
        "bin/typst"
    }));
    command
        .args(["compile", "--ignore-system-fonts", "--font-path"])
        .arg(root.join("fonts"))
        .arg("--root")
        .arg(work)
        .args(["--diagnostic-format", "short"])
        .arg(work.join("document.typ"))
        .arg(work.join("output.pdf"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(&diagnostics).map_err(error)?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("The PDF renderer could not start: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if cancelled(work).is_err() || started.elapsed() > Duration::from_secs(300) {
            let _ = child.kill();
            let _ = child.wait();
            cancelled(work)?;
            return Err("This document took too long to render.".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(35)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error(e));
            }
        }
    };
    let diagnostics = String::from_utf8_lossy(&read(&diagnostics, 1024 * 1024)?).into_owned();
    if !status.success() || !diagnostics.trim().is_empty() {
        return Err(diagnostics
            .lines()
            .next()
            .unwrap_or("This document could not be rendered.")
            .chars()
            .take(600)
            .collect());
    }
    let bytes = read(&work.join("output.pdf"), 256 * 1024 * 1024)?;
    if !bytes.starts_with(b"%PDF-")
        || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|w| w == b"%%EOF")
    {
        return Err("The PDF renderer produced an incomplete document.".into());
    }
    Ok(())
}
fn docx_page_size(path: &Path, paper: &str) -> Result<()> {
    use quick_xml::{
        events::{BytesStart, Event},
        Reader, Writer,
    };
    let mut source = zip::ZipArchive::new(File::open(path).map_err(error)?).map_err(error)?;
    let temporary = path.with_extension("staged.docx");
    let mut output = zip::ZipWriter::new(File::create(&temporary).map_err(error)?);
    let mut changed = false;
    for i in 0..source.len() {
        let mut entry = source.by_index(i).map_err(error)?;
        let name = entry.name().map_err(error)?.into_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(error)?;
        if name == "word/document.xml" {
            let mut reader = Reader::from_reader(bytes.as_slice());
            let mut writer = Writer::new(Vec::new());
            loop {
                match reader.read_event().map_err(error)? {
                    Event::Empty(tag) if tag.name().as_ref() == b"w:pgSz" => {
                        let mut tag = BytesStart::new("w:pgSz");
                        let (width, height) = if paper == "Letter" {
                            ("12240", "15840")
                        } else {
                            ("11906", "16838")
                        };
                        tag.push_attribute(("w:w", width));
                        tag.push_attribute(("w:h", height));
                        writer.write_event(Event::Empty(tag)).map_err(error)?;
                        changed = true;
                    }
                    Event::End(tag) if tag.name().as_ref() == b"w:sectPr" && !changed => {
                        let mut page = BytesStart::new("w:pgSz");
                        let (width, height) = if paper == "Letter" {
                            ("12240", "15840")
                        } else {
                            ("11906", "16838")
                        };
                        page.push_attribute(("w:w", width));
                        page.push_attribute(("w:h", height));
                        writer.write_event(Event::Empty(page)).map_err(error)?;
                        writer.write_event(Event::End(tag)).map_err(error)?;
                        changed = true;
                    }
                    Event::Eof => break,
                    event => writer.write_event(event).map_err(error)?,
                }
            }
            bytes = writer.into_inner();
        }
        output
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(error)?;
        output.write_all(&bytes).map_err(error)?;
    }
    output.finish().map_err(error)?;
    if !changed {
        return Err("The Word document has no valid page layout.".into());
    }
    drop(source);
    fs::copy(&temporary, path).map_err(error)?;
    fs::remove_file(temporary).map_err(error)?;
    Ok(())
}
fn convert(request: &Request, root: &Path, input: &str, text: &str, original: &[u8]) -> Result<()> {
    let target = request.target.as_deref().ok_or("Choose PDF or DOCX.")?;
    if !["pdf", "docx"].contains(&target) {
        return Err("Choose PDF or DOCX.".into());
    }
    if request.settings.len() != 1 {
        return Err("Choose a page size.".into());
    }
    let paper = request
        .settings
        .get("page-size")
        .filter(|s| ["A4", "Letter"].contains(&s.as_str()))
        .ok_or("Choose A4 or Letter paper.")?;
    let work = &request.workspace;
    progress(work, 0.05, "Reading document")?;
    fs::write(work.join("stdin"), if input == "text" { "" } else { text }).map_err(error)?;
    let mut pandoc = engine::Pandoc::new(root, work)?;
    let _interrupt = Interrupt::new(pandoc.engine(), work.clone());
    pandoc.convert(
        json!({"from":"markdown", "to":"json", "standalone":true}),
        work,
    )?;
    let mut document: Value =
        serde_json::from_slice(&read(&work.join("stdout"), 64 * 1024 * 1024)?).map_err(error)?;
    if input == "text" {
        document["meta"] = json!({});
        document["blocks"] = Value::Array(
            text.split("\n\n")
                .map(|paragraph| {
                    let mut inlines = Vec::new();
                    for (index, line) in paragraph.split('\n').enumerate() {
                        if index > 0 {
                            inlines.push(json!({"t":"LineBreak"}));
                        }
                        inlines.push(json!({"t":"Str","c":line}));
                    }
                    json!({"t":"Para","c":inlines})
                })
                .collect(),
        );
    }
    if let Some(meta) = document["meta"].as_object_mut() {
        for key in [
            "header-includes",
            "include-before",
            "include-after",
            "bibliography",
            "csl",
            "template",
        ] {
            if meta.contains_key(key) {
                return Err(format!("Document metadata '{key}' is not supported yet."));
            }
        }
        meta.retain(|key, _| {
            ["title", "subtitle", "author", "date", "abstract"].contains(&key.as_str())
        });
    }
    let mut context = AssetContext {
        source: &request.source,
        work,
        snapshots: Vec::new(),
        bytes: 0,
        text: String::new(),
    };
    context.walk(&mut document)?;
    if target == "pdf" {
        check_fonts(root, &context.text)?;
    }
    progress(work, 0.3, "Formatting document")?;
    fs::write(
        work.join("stdin"),
        serde_json::to_vec(&document).map_err(error)?,
    )
    .map_err(error)?;
    if target == "pdf" {
        fs::copy(
            root.join("templates/document.typst"),
            work.join("recast.typst"),
        )
        .map_err(error)?;
        pandoc.convert(json!({"from":"json", "to":"typst", "standalone":true, "template":"recast.typst", "output-file":"document.typ", "variables":{"papersize":if paper == "Letter" {"us-letter"} else {"a4"},"fontsize":"11pt"}, "fail-if-warnings":true}), work)?;
        progress(work, 0.65, "Rendering PDF")?;
        run_typst(root, work)?;
    } else {
        pandoc.convert(json!({"from":"json", "to":"docx", "standalone":true, "output-file":"output.docx", "fail-if-warnings":true}), work)?;
        docx_page_size(&work.join("output.docx"), paper)?;
    }
    progress(work, 0.95, "Checking output")?;
    if read(&request.source, MAX_TEXT)? != original
        || context
            .snapshots
            .iter()
            .any(|(path, bytes)| read(path, 64 * 1024 * 1024).as_ref() != Ok(bytes))
    {
        return Err(
            "The document or one of its images changed during conversion. Try again.".into(),
        );
    }
    cancelled(work)?;
    let output = request
        .output
        .as_deref()
        .ok_or("Missing output destination.")?;
    fs::copy(work.join(format!("output.{target}")), output).map_err(error)?;
    File::options()
        .write(true)
        .open(output)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    Ok(())
}
fn handle(request: &Request, root: &Path) -> Result<Value> {
    if request.protocol_version != 1 {
        return Err("Unsupported provider protocol.".into());
    }
    cancelled(&request.workspace)?;
    let (input, text, original) = source(&request.source)?;
    match request.action.as_str() {
        "inspect" => Ok(json!({"protocolVersion":1,"input":input,"targets":["pdf","docx"]})),
        "convert" => {
            convert(request, root, &input, &text, &original)?;
            Ok(json!({"protocolVersion":1}))
        }
        _ => Err("Unsupported provider request.".into()),
    }
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "--compile") && args.len() == 4 {
        if let Err(error) = engine::compile(Path::new(&args[2]), Path::new(&args[3])) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() != 3 || args[1] != "--request" {
        eprintln!("Expected --request FILE.");
        std::process::exit(1);
    }
    let request: Request = match read(Path::new(&args[2]), 64 * 1024)
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(error))
    {
        Ok(request) => request,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let root = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent()?.parent().map(Path::to_path_buf));
    let response = root
        .ok_or_else(|| "The extension folder is unavailable.".to_string())
        .and_then(|root| handle(&request, &root));
    let failed = response.is_err();
    let response = response.unwrap_or_else(|error| json!({"protocolVersion":1,"error":error}));
    if let Err(error) = fs::write(
        request.workspace.join("response.json"),
        serde_json::to_vec(&response).unwrap(),
    ) {
        eprintln!("{error}");
        std::process::exit(1);
    }
    if failed {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn walk(node: &mut Value) -> Result<()> {
        AssetContext {
            source: Path::new("unused.md"),
            work: Path::new("unused"),
            snapshots: Vec::new(),
            bytes: 0,
            text: String::new(),
        }
        .walk(node)
    }
    #[test]
    fn rejects_embedded_code_but_keeps_literal_text_and_comments() {
        let mut raw = json!({"t":"RawBlock","c":["typst","#read(\"secret\")"]});
        assert!(walk(&mut raw).is_err());
        let mut literal = json!({"t":"CodeBlock","c":[["",[],[]],"typst: ordinary code"]});
        walk(&mut literal).unwrap();
        assert_eq!(literal["c"][1], "typst: ordinary code");
        let mut attribute = json!({"t":"Div","c":[["",[],[["typst:fill","red"]]],[]]});
        assert!(walk(&mut attribute).is_err());
        let mut comment = json!({"t":"RawBlock","c":["html","<!-- internal note -->"]});
        walk(&mut comment).unwrap();
        assert_eq!(comment, json!({"t":"Plain","c":[]}));
    }
}
