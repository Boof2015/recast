//! Versioned, out-of-process conversion providers. The host owns publication.
use crate::image_backend::{check_cancel, publish, ConversionError, OutputFile};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Setting {
    pub id: String,
    pub label: String,
    pub choices: Vec<String>,
    pub default: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub extensions: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub label: String,
    pub description: String,
    pub extension: String,
    pub inputs: Vec<String>,
    pub settings: Vec<Setting>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub protocol_version: u32,
    pub executable: String,
    pub inputs: Vec<Input>,
    pub targets: Vec<Target>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub key: String,
    pub target: String,
    pub settings: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Destination {
    pub id: String,
    pub label: String,
    pub description: String,
    pub category: String,
    pub provider_key: String,
    pub provider_target: String,
    pub extension: String,
    pub settings: Vec<Setting>,
}
// A shared PDF group may use multiple document providers. Settings still
// apply uniformly to every file handled by a given provider/target.
#[derive(Default)]
pub struct GroupSelections {
    outputs: BTreeMap<String, String>,
    settings: BTreeMap<(String, String), String>,
}
impl GroupSelections {
    pub fn check(
        &mut self,
        kind: &str,
        target: &Target,
        selection: &Selection,
    ) -> Result<(), String> {
        let selection_id = serde_json::to_string(selection).map_err(|e| e.to_string())?;
        let output_id = if kind == "documents" && target.extension == "pdf" {
            "shared-pdf".to_string()
        } else {
            selection_id.clone()
        };
        if self
            .outputs
            .insert(kind.into(), output_id.clone())
            .is_some_and(|old| old != output_id)
            || self
                .settings
                .insert((kind.into(), selection.key.clone()), selection_id.clone())
                .is_some_and(|old| old != selection_id)
        {
            return Err("Choose one output and settings for the whole group.".into());
        }
        Ok(())
    }
}
#[derive(Debug)]
pub struct Provider {
    pub key: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub root: PathBuf,
    pub descriptor: Descriptor,
    pub _lease: crate::extensions::Lease,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Response {
    protocol_version: u32,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
}
#[derive(Deserialize)]
struct Progress {
    fraction: f64,
    phase: String,
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"-".contains(&c))
}
impl Descriptor {
    pub fn validate(&self) -> Result<(), String> {
        if self.protocol_version != 1
            || !crate::extensions::portable_path(&self.executable)
            || self.inputs.is_empty()
            || self.inputs.len() > 32
            || self.targets.is_empty()
            || self.targets.len() > 32
        {
            return Err("Unsupported conversion provider descriptor.".into());
        }
        let mut inputs = HashSet::new();
        for input in &self.inputs {
            if !id(&input.id)
                || !inputs.insert(input.id.as_str())
                || !text(&input.label, 80)
                || !["images", "audio", "video", "documents"].contains(&input.kind.as_str())
                || input.extensions.is_empty()
                || input.extensions.len() > 32
                || input.extensions.iter().any(|s| !id(s))
            {
                return Err("Invalid provider input declaration.".into());
            }
        }
        let mut targets = HashSet::new();
        for target in &self.targets {
            let mut settings = HashSet::new();
            if !id(&target.id)
                || !targets.insert(&target.id)
                || !id(&target.extension)
                || !text(&target.label, 80)
                || !text(&target.description, 200)
                || target.inputs.is_empty()
                || target.inputs.iter().any(|s| !inputs.contains(s.as_str()))
                || target.settings.len() > 8
            {
                return Err("Invalid provider output declaration.".into());
            }
            for setting in &target.settings {
                if !id(&setting.id)
                    || !settings.insert(&setting.id)
                    || !text(&setting.label, 80)
                    || setting.choices.is_empty()
                    || setting.choices.len() > 32
                    || !setting.choices.contains(&setting.default)
                    || setting.choices.iter().any(|s| !text(s, 80))
                    || setting.choices.iter().collect::<HashSet<_>>().len() != setting.choices.len()
                {
                    return Err("Invalid provider setting declaration.".into());
                }
            }
        }
        Ok(())
    }
}
impl Provider {
    pub fn matches(&self, path: &Path, kind: Option<&str>) -> bool {
        let extension = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        self.descriptor.inputs.iter().any(|input| {
            kind.is_none_or(|kind| input.kind == kind) && input.extensions.contains(&extension)
        })
    }
    pub fn target(&self, selection: &Selection) -> Result<&Target, String> {
        if selection.key != self.key {
            return Err("The selected extension version is unavailable.".into());
        }
        let target = self
            .descriptor
            .targets
            .iter()
            .find(|target| target.id == selection.target)
            .ok_or("This extension does not support the selected output.")?;
        if selection.settings.len() != target.settings.len()
            || target.settings.iter().any(|s| {
                !selection
                    .settings
                    .get(&s.id)
                    .is_some_and(|value| s.choices.contains(value))
            })
        {
            return Err("Choose valid extension settings.".into());
        }
        Ok(target)
    }
    pub fn destination(&self, target: &Target) -> Destination {
        Destination {
            id: format!("extension:{}:{}", self.id, target.id),
            label: target.label.clone(),
            description: target.description.clone(),
            category: self.name.clone(),
            provider_key: self.key.clone(),
            provider_target: target.id.clone(),
            extension: target.extension.clone(),
            settings: target.settings.clone(),
        }
    }
    pub fn inspect(
        &self,
        source: &Path,
        cancel: &AtomicBool,
    ) -> Result<(Input, Vec<Destination>), String> {
        let response = self
            .call(
                serde_json::json!({"action":"inspect", "source":source}),
                cancel,
                |_, _| {},
            )
            .map_err(message)?;
        let input = self
            .descriptor
            .inputs
            .iter()
            .find(|input| Some(&input.id) == response.input.as_ref())
            .ok_or("The extension returned an undeclared input format.")?
            .clone();
        let mut seen = HashSet::new();
        let mut destinations = Vec::new();
        for id in response.targets {
            let target = self
                .descriptor
                .targets
                .iter()
                .find(|target| target.id == id && target.inputs.contains(&input.id))
                .ok_or("The extension returned an undeclared conversion.")?;
            if !seen.insert(id) {
                return Err("The extension returned duplicate conversions.".into());
            }
            destinations.push(self.destination(target));
        }
        Ok((input, destinations))
    }
    fn call(
        &self,
        mut request: serde_json::Value,
        cancel: &AtomicBool,
        report: impl Fn(f64, &str),
    ) -> Result<Response, ConversionError> {
        check_cancel(cancel)?;
        let workspace = tempfile::Builder::new()
            .prefix("recast-provider-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let request_path = workspace.path().join("request.json");
        request["protocolVersion"] = 1.into();
        request["workspace"] = serde_json::to_value(workspace.path()).map_err(|e| e.to_string())?;
        fs::write(
            &request_path,
            serde_json::to_vec(&request).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let diagnostics = workspace.path().join("diagnostics.log");
        let mut command = Command::new(self.root.join(&self.descriptor.executable));
        command
            .arg("--request")
            .arg(&request_path)
            .current_dir(workspace.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&diagnostics).map_err(|e| e.to_string())?);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{} could not start: {e}", self.name))?;
        let mut process = ProcessGuard::new(&mut child)?;
        let started = Instant::now();
        let timeout = if request["action"] == "inspect" {
            30
        } else {
            600
        };
        let mut stopping = None;
        let mut last_progress = String::new();
        let status = loop {
            if check_cancel(cancel).is_err() || started.elapsed() > Duration::from_secs(timeout) {
                if stopping.is_none() {
                    let _ = fs::write(workspace.path().join("cancel"), b"");
                    stopping = Some(Instant::now());
                }
                if stopping.is_some_and(|at: Instant| at.elapsed() > Duration::from_secs(2)) {
                    process.kill();
                }
            }
            if fs::metadata(&diagnostics).is_ok_and(|m| m.len() > 1024 * 1024) {
                process.kill();
                return Err("The extension produced too much diagnostic output."
                    .to_string()
                    .into());
            }
            if let Ok(bytes) = read_bounded(&workspace.path().join("progress.json"), 4096) {
                if let Ok(progress) = serde_json::from_slice::<Progress>(&bytes) {
                    if progress.fraction.is_finite()
                        && (0.0..=1.0).contains(&progress.fraction)
                        && text(&progress.phase, 100)
                    {
                        let identity = format!("{}:{}", progress.fraction, progress.phase);
                        if last_progress != identity {
                            report(progress.fraction, &progress.phase);
                            last_progress = identity;
                        }
                    }
                }
            }
            match process.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(35)),
                Err(e) => return Err(format!("The extension stopped unexpectedly: {e}").into()),
            }
        };
        check_cancel(cancel)?;
        if stopping.is_some() {
            return Err("This document took too long to convert.".to_string().into());
        }
        let bytes = read_bounded(&workspace.path().join("response.json"), 64 * 1024)?;
        let response: Response = serde_json::from_slice(&bytes)
            .map_err(|_| "The extension returned an invalid response.".to_string())?;
        if response.protocol_version != 1
            || response.warnings.len() > 16
            || response.warnings.iter().any(|s| !text(s, 1000))
        {
            return Err("The extension returned an invalid response."
                .to_string()
                .into());
        }
        if let Some(error) = &response.error {
            return Err(error.chars().take(1000).collect::<String>().into());
        }
        if !status.success() {
            return Err("The extension could not finish this conversion."
                .to_string()
                .into());
        }
        Ok(response)
    }
    pub fn convert(
        &self,
        source: &Path,
        folder: Option<&Path>,
        selection: &Selection,
        cancel: &AtomicBool,
        report: impl Fn(f64, &str),
    ) -> Result<OutputFile, ConversionError> {
        let target = self.target(selection)?;
        let inspected = self.inspect(source, cancel);
        check_cancel(cancel)?;
        let (_, offered) = inspected?;
        if !offered
            .iter()
            .any(|d| d.provider_target == selection.target)
        {
            return Err("This file cannot use the selected extension output."
                .to_string()
                .into());
        }
        let folder = folder
            .or_else(|| source.parent())
            .ok_or_else(|| "Choose an output folder.".to_string())?;
        let temporary = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{}", target.extension))
            .tempfile_in(folder)
            .map_err(|_| "The output folder is not writable.".to_string())?;
        let response = self.call(serde_json::json!({"action":"convert", "source":source, "target":target.id, "settings":selection.settings, "output":temporary.path()}), cancel, report)?;
        check_cancel(cancel)?;
        let metadata = fs::symlink_metadata(temporary.path()).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_file() || metadata.len() == 0 {
            return Err("The extension did not produce a valid output file."
                .to_string()
                .into());
        }
        // Reopen the path: a provider may have replaced the originally empty file.
        let mut validated = tempfile::Builder::new()
            .prefix(".recast-")
            .tempfile_in(folder)
            .map_err(|e| e.to_string())?;
        std::io::copy(
            &mut File::open(temporary.path()).map_err(|e| e.to_string())?,
            validated.as_file_mut(),
        )
        .map_err(|e| e.to_string())?;
        validated
            .as_file_mut()
            .sync_all()
            .map_err(|e| e.to_string())?;
        check_cancel(cancel)?;
        let bytes = validated
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        let path = publish(
            validated,
            source.file_stem().unwrap_or_default(),
            folder,
            &target.extension,
        )?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
            warnings: response.warnings,
            details: Some(format!("{} {}", self.name, self.version)),
        })
    }
}
fn message(error: ConversionError) -> String {
    match error {
        ConversionError::Failed(s) => s,
        ConversionError::Cancelled => "Cancelled.".into(),
        ConversionError::Clipping => "Unsupported extension response.".into(),
    }
}
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Expected a regular extension file.".into());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Extension response exceeds its limit.".into());
    }
    Ok(bytes)
}
struct ProcessGuard<'a> {
    child: &'a mut std::process::Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}
impl<'a> ProcessGuard<'a> {
    fn new(child: &'a mut std::process::Child) -> Result<Self, ConversionError> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.is_null()
                || unsafe {
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &limits as *const _ as *const _,
                        std::mem::size_of_val(&limits) as u32,
                    )
                } == 0
                || unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0
            {
                let _ = child.kill();
                let _ = child.wait();
                if !job.is_null() {
                    unsafe {
                        CloseHandle(job);
                    }
                }
                return Err("The extension process could not be managed safely."
                    .to_string()
                    .into());
            }
            Ok(Self { child, job })
        }
        #[cfg(not(windows))]
        {
            Ok(Self { child })
        }
    }
    fn kill(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
        let _ = self.child.kill();
    }
}
impl Drop for ProcessGuard<'_> {
    fn drop(&mut self) {
        self.kill();
        let _ = self.child.wait();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

pub fn selected<'a>(
    providers: &'a [Arc<Provider>],
    selection: &Selection,
) -> Result<&'a Provider, String> {
    providers
        .iter()
        .find(|p| p.key == selection.key)
        .map(Arc::as_ref)
        .ok_or_else(|| {
            "This extension version is no longer available. Choose an output again.".into()
        })
}

#[cfg(test)]
pub(crate) fn test_descriptor() -> Descriptor {
    serde_json::from_value(serde_json::json!({
        "protocolVersion":1,"executable":"bin/worker",
        "inputs":[{"id":"text","label":"TXT","kind":"documents","extensions":["txt"]}],
        "targets":[{"id":"pdf","label":"PDF","description":"Document","extension":"pdf","inputs":["text"],
          "settings":[{"id":"page-size","label":"Page size","choices":["A4","Letter"],"default":"A4"}]}]
    })).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_document_pdf_keeps_each_providers_group_settings_consistent() {
        let mut group = GroupSelections::default();
        let text = test_descriptor().targets.remove(0);
        let mut office = text.clone();
        office.settings.clear();
        let text_selection = Selection {
            key: "text".into(),
            target: "pdf".into(),
            settings: BTreeMap::from([("page-size".into(), "Letter".into())]),
        };
        let office_selection = Selection {
            key: "office".into(),
            target: "pdf".into(),
            settings: BTreeMap::new(),
        };
        group.check("documents", &text, &text_selection).unwrap();
        group
            .check("documents", &office, &office_selection)
            .unwrap();
        group.check("documents", &text, &text_selection).unwrap();
        let mut changed = text_selection.clone();
        changed.settings.insert("page-size".into(), "A4".into());
        assert!(group.check("documents", &text, &changed).is_err());
        let mut group = GroupSelections::default();
        group.check("documents", &text, &text_selection).unwrap();
        office.extension = "docx".into();
        assert!(group
            .check("documents", &office, &office_selection)
            .is_err());
        let mut group = GroupSelections::default();
        group.check("images", &text, &text_selection).unwrap();
        assert!(group.check("images", &text, &office_selection).is_err());
    }

    #[test]
    fn descriptors_reject_unsafe_execution_and_ambiguous_capabilities() {
        let valid = test_descriptor();
        valid.validate().unwrap();
        let mut bad = valid.clone();
        bad.executable = "../worker".into();
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.targets[0].inputs = vec!["unknown".into()];
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.targets[0].settings[0].choices = vec!["A4".into(), "A4".into()];
        assert!(bad.validate().is_err());
        bad = valid.clone();
        bad.targets[0].settings[0].default = "Other".into();
        assert!(bad.validate().is_err());
        bad = valid;
        bad.inputs.push(bad.inputs[0].clone());
        assert!(bad.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_stops_an_uncooperative_worker_and_descendant() {
        use std::os::unix::process::CommandExt;
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("child.pid");
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 60 & echo $! > \"$1\"; wait", "fixture"])
            .arg(&pid_path)
            .process_group(0)
            .spawn()
            .unwrap();
        let guard = ProcessGuard::new(&mut child).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !fs::read_to_string(&pid_path).is_ok_and(|s| s.trim().parse::<i32>().is_ok()) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let descendant: i32 = fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(guard);
        assert!(!child.wait().unwrap().success());
        // A killed orphan can briefly remain as a zombie on Linux. ps reports
        // that state without treating it as a still-running converter.
        let status = Command::new("ps")
            .args(["-o", "stat=", "-p", &descendant.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&status.stdout);
        assert!(
            state.trim().is_empty() || state.trim().starts_with('Z'),
            "{state}"
        );
    }
}
