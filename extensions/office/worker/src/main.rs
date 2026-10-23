mod doc;
mod documents;
mod docx;
mod odt;
mod rtf;
#[cfg(test)]
mod tests;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
const MAX_SOURCE: u64 = 256 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    action: String,
    source: PathBuf,
    workspace: PathBuf,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    settings: BTreeMap<String, String>,
    #[serde(default)]
    output: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Engine {
    executable: String,
    version: String,
}
fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|_| {
        format!(
            "Could not read {}.",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("Choose an individual document file.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err("This document exceeds the supported size limit.".into());
    }
    Ok(bytes)
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
    fs::write(
        work.join("progress.json"),
        serde_json::to_vec(&json!({"fraction":fraction,"phase":phase})).map_err(error)?,
    )
    .map_err(error)
}
fn profile(work: &Path) -> Result<PathBuf> {
    let profile = work.join("profile");
    fs::create_dir_all(profile.join("user")).map_err(error)?;
    // This profile is private to a single conversion. It cannot attach to an
    // installed LibreOffice process or read/change the user's suite settings.
    fs::write(profile.join("user/registrymodifications.xcu"), br#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting">
<prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop>
<prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop>
<!-- LibreOffice treats native equations as active content. Input inspection
rejects actual embedded objects; keep native equation rendering enabled. -->
<prop oor:name="DisableActiveContent" oor:op="fuse"><value>false</value></prop>
<prop oor:name="DisablePythonRuntime" oor:op="fuse"><value>true</value></prop>
<prop oor:name="DisableOLEAutomation" oor:op="fuse"><value>true</value></prop>
<prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop>
</item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>0</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Misc"><prop oor:name="FirstRun" oor:op="fuse"><value>false</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Save/Document"><prop oor:name="WarnAlienFormat" oor:op="fuse"><value>false</value></prop></item>
</oor:items>"#).map_err(error)?;
    Ok(profile)
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn run_office(
    command: &mut Command,
    work: &Path,
    log: &Path,
    timeout: Duration,
) -> Result<ExitStatus> {
    // LibreOffice's launcher restarts soffice.bin after exit 81 when a new
    // profile needs initialization (desktop/exithelper.h). We launch the engine
    // directly so cancellation can kill/reap it; handle that one normal restart
    // here, keeping the same profile, arguments, log and overall deadline.
    let started = Instant::now();
    let mut attempts = Vec::new();
    loop {
        let check = || -> Result<()> {
            cancelled(work)?;
            if started.elapsed() >= timeout {
                return Err("This document took too long to render.".into());
            }
            if fs::metadata(log).is_ok_and(|m| m.len() > 1024 * 1024) {
                return Err("The Office engine produced too much diagnostic output.".into());
            }
            Ok(())
        };
        check()?;
        let mut process = Process(
            command
                .spawn()
                .map_err(|e| format!("The Office engine could not start: {e}"))?,
        );
        let status = loop {
            check()?;
            if let Some(status) = process.0.try_wait().map_err(error)? {
                break status;
            }
            thread::sleep(Duration::from_millis(35));
        };
        attempts.push(json!({"exitCode":status.code(),"status":status.to_string()}));
        fs::write(
            work.join("office-process.json"),
            serde_json::to_vec(&json!({"attempts":attempts})).map_err(error)?,
        )
        .map_err(error)?;
        // Never retry crashes, ordinary conversion failures or a restart loop.
        if status.code() != Some(81) || attempts.len() >= 2 {
            return Ok(status);
        }
    }
}
fn render(root: &Path, engine: &Engine, work: &Path, kind: &str) -> Result<()> {
    let executable = root.join(&engine.executable);
    if !executable.is_file() {
        return Err("The Office engine is missing. Reinstall the extension.".into());
    }
    let profile = profile(work)?;
    let profile = url::Url::from_directory_path(profile)
        .map_err(|_| "The Office profile path is unavailable.".to_string())?;
    let out = work.join("rendered");
    fs::create_dir_all(&out).map_err(error)?;
    let log = work.join("office.log");
    let file = File::create(&log).map_err(error)?;
    let filter = "pdf:writer_pdf_Export:".to_string()
        + &json!({
            "UseLosslessCompression":{"type":"boolean","value":"true"},
            "ReduceImageResolution":{"type":"boolean","value":"false"},
            "SelectPdfVersion":{"type":"long","value":"17"},
            "ExportBookmarks":{"type":"boolean","value":"true"},
            "ExportNotes":{"type":"boolean","value":"false"},
            "IsAddStream":{"type":"boolean","value":"false"},
            "ExportFormFields":{"type":"boolean","value":"false"}
        })
        .to_string();
    let mut command = Command::new(&executable);
    command
        .arg(format!("-env:UserInstallation={profile}"))
        .args([
            "--headless",
            "--nologo",
            "--nodefault",
            "--norestore",
            "--nofirststartwizard",
            "--convert-to",
        ])
        .arg(filter)
        .arg("--outdir")
        .arg(&out)
        .arg(work.join(format!("document.{kind}")))
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(error)?)
        .stderr(file)
        .env("SAL_DISABLE_CRASHREPORT", "1")
        .env("XDG_CACHE_HOME", work.join("cache"));
    #[cfg(target_os = "linux")]
    {
        command
            .env("SAL_USE_VCLPLUGIN", "svp")
            .env("LD_LIBRARY_PATH", executable.parent().unwrap());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let status = run_office(&mut command, work, &log, Duration::from_secs(300))?;
    cancelled(work)?;
    let diagnostic = String::from_utf8_lossy(&read(&log, 1024 * 1024)?).to_string();
    if !status.success()
        || !out.join("document.pdf").is_file()
        || diagnostic
            .lines()
            .any(|line| line.to_ascii_lowercase().contains("error:"))
    {
        return Err("LibreOffice could not render this document. It may be damaged, protected, or use unsupported content.".into());
    }
    Ok(())
}
fn validate_pdf(path: &Path) -> Result<()> {
    let bytes = read(path, 512 * 1024 * 1024)?;
    if !bytes.starts_with(b"%PDF-")
        || !bytes[bytes.len().saturating_sub(1024)..]
            .windows(5)
            .any(|v| v == b"%%EOF")
    {
        return Err("The Office engine produced an incomplete PDF.".into());
    }
    let pdf = lopdf::Document::load_mem(&bytes)
        .map_err(|_| "The Office engine produced an unreadable PDF.".to_string())?;
    if pdf.is_encrypted() || pdf.xref_recovery.is_some() {
        return Err("The Office engine produced an invalid PDF.".into());
    }
    let pages = pdf.get_pages();
    if pages.is_empty() || pages.len() > 100_000 {
        return Err("The PDF has an invalid page count.".into());
    }
    for (_, id) in pages {
        pdf.get_dictionary(id).map_err(error)?;
        let content = pdf
            .get_page_content_with_limit(id, 128 * 1024 * 1024)
            .map_err(|_| "The PDF contains a damaged page.".to_string())?;
        lopdf::content::Content::decode(&content)
            .map_err(|_| "The PDF contains damaged page instructions.".to_string())?;
    }
    Ok(())
}
fn handle(request: &Request, root: &Path) -> Result<Value> {
    if request.protocol_version != 1 {
        return Err("Unsupported provider protocol.".into());
    }
    cancelled(&request.workspace)?;
    let original = read(&request.source, MAX_SOURCE)?;
    let (kind, document) = documents::inspect(&original)?;
    if request.action == "inspect" {
        return Ok(json!({"protocolVersion":1,"input":kind,"targets":["pdf"]}));
    }
    if request.action != "convert"
        || request.target.as_deref() != Some("pdf")
        || !request.settings.is_empty()
    {
        return Err("Choose PDF with the document's original layout.".into());
    }
    let output = request.output.as_deref().ok_or("Missing output path.")?;
    let engine: Engine =
        serde_json::from_slice(&read(&root.join("engine.json"), 4096)?).map_err(error)?;
    if engine.version.is_empty()
        || Path::new(&engine.executable).is_absolute()
        || engine.executable.split('/').any(|p| p == "..")
    {
        return Err("The Office engine configuration is invalid.".into());
    }
    let work = &request.workspace;
    progress(work, 0.1, "Preparing document")?;
    fs::write(work.join(format!("document.{kind}")), &original).map_err(error)?;
    progress(work, 0.25, "Rendering PDF")?;
    render(root, &engine, work, kind)?;
    progress(work, 0.9, "Checking PDF")?;
    let rendered = work.join("rendered/document.pdf");
    validate_pdf(&rendered)?;
    if Sha256::digest(read(&request.source, MAX_SOURCE)?) != Sha256::digest(&original) {
        return Err("The document changed during conversion. Try again.".into());
    }
    cancelled(work)?;
    fs::copy(&rendered, output).map_err(error)?;
    File::options()
        .write(true)
        .open(output)
        .map_err(error)?
        .sync_all()
        .map_err(error)?;
    let warnings: Vec<&str> = if document.comments {
        vec!["Document comments are not included in the PDF."]
    } else {
        Vec::new()
    };
    Ok(json!({"protocolVersion":1,"warnings":warnings}))
}
fn main() {
    let args: Vec<_> = std::env::args_os().collect();
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
    let result = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent()?.parent().map(Path::to_path_buf))
        .ok_or("The extension directory is unavailable.".to_string())
        .and_then(|root| handle(&request, &root));
    let failed = result.is_err();
    let response = result.unwrap_or_else(|error| json!({"protocolVersion":1,"error":error}));
    if fs::write(
        request.workspace.join("response.json"),
        serde_json::to_vec(&response).unwrap(),
    )
    .is_err()
        || failed
    {
        std::process::exit(1);
    }
}
