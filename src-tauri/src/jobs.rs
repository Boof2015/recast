use crate::audio_backend::{AudioBackend, AudioOptions};
use crate::audio_extract::VideoAudioOptions;
use crate::image_backend::{ConversionError, ImageBackend, ImageOptions};
use crate::video_backend::VideoOptions;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{ipc::Channel, Manager, State, WebviewWindow};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversionRequest {
    pub paths: Vec<String>,
    pub output_folder: Option<String>,
    pub image_options: Option<ImageOptions>,
    pub image_frame_options: Option<crate::image_frame::ImageFrameOptions>,
    pub audio_options: Option<AudioOptions>,
    pub video_options: Option<VideoOptions>,
    pub video_audio_options: Option<VideoAudioOptions>,
    pub animation_options: Option<crate::video_animation::AnimationOptions>,
    pub video_frame_options: Option<crate::video_frame::FrameOptions>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileResult {
    pub id: String,
    pub name: String,
    pub status: FileStatus,
    pub output_path: Option<String>,
    pub output_bytes: Option<u64>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub details: Option<String>,
    pub progress: Option<f64>,
    pub phase: Option<String>,
    pub can_allow_clipping: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum BatchStatus {
    Running,
    Cancelling,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSnapshot {
    pub id: String,
    pub revision: u64,
    pub status: BatchStatus,
    pub files: Vec<FileResult>,
}

pub struct Job {
    pub snapshot: Mutex<BatchSnapshot>,
    request: ConversionRequest,
    cancel: AtomicBool,
}

impl Job {
    fn snapshot(&self) -> BatchSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
    fn update(&self, change: impl FnOnce(&mut BatchSnapshot)) -> BatchSnapshot {
        let mut snapshot = self
            .snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut snapshot);
        snapshot.revision += 1;
        snapshot.clone()
    }
    pub fn active(&self) -> bool {
        matches!(
            self.snapshot().status,
            BatchStatus::Running | BatchStatus::Cancelling
        )
    }
    fn request_cancel(&self) -> BatchSnapshot {
        self.update(|state| {
            if matches!(state.status, BatchStatus::Running | BatchStatus::Cancelling) {
                self.cancel.store(true, Ordering::Relaxed);
                state.status = BatchStatus::Cancelling;
            }
        })
    }
}

#[derive(Default)]
pub struct JobManager(Mutex<HashMap<String, Arc<Job>>>, AtomicBool);

impl JobManager {
    pub fn cancel_before_exit(&self) -> Vec<Arc<Job>> {
        let jobs = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Holding the same lock as prepare prevents a new run from starting
        // between collecting active jobs and beginning shutdown.
        self.1.store(true, Ordering::Relaxed);
        jobs.values()
            .filter(|job| job.active())
            .map(|job| {
                job.request_cancel();
                job.clone()
            })
            .collect()
    }

    pub fn cancel_before_close(&self, label: &str) -> Option<Arc<Job>> {
        let job = self.0.lock().ok()?.get(label)?.clone();
        if !job.active() {
            return None;
        }
        job.request_cancel();
        Some(job)
    }
    pub fn cancel_window(&self, label: &str) {
        if let Some(job) = self.0.lock().unwrap().remove(label) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn prepare(
        &self,
        label: &str,
        request: ConversionRequest,
        retry: bool,
    ) -> Result<Arc<Job>, String> {
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| "The batch state is unavailable.".to_string())?;
        if self.1.load(Ordering::Relaxed) {
            return Err("Recast is closing. Wait for the current conversions to stop.".into());
        }
        if jobs.get(label).is_some_and(|job| job.active()) {
            return Err("Wait for this conversion to finish or cancel it first.".into());
        }
        let (request, files) = if retry {
            let previous = jobs.get(label).ok_or("There is no batch to retry.")?;
            let mut files = previous.snapshot().files;
            if files
                .iter()
                .all(|file| file.status == FileStatus::Succeeded)
            {
                return Err("Every file has already converted.".into());
            }
            for file in &mut files {
                if file.status != FileStatus::Succeeded {
                    file.status = FileStatus::Pending;
                    file.error = None;
                    file.output_path = None;
                    file.output_bytes = None;
                    file.warnings.clear();
                    file.details = None;
                    file.progress = None;
                    file.phase = None;
                    file.can_allow_clipping = false;
                }
            }
            let mut saved = previous.request.clone();
            // Retry freezes all original settings except this explicit consent.
            // Successful outputs stay untouched when retrying clipped files.
            if request
                .audio_options
                .as_ref()
                .is_some_and(|options| options.allow_clipping)
            {
                if let Some(options) = &mut saved.audio_options {
                    options.allow_clipping = true;
                }
            }
            if request
                .video_audio_options
                .as_ref()
                .is_some_and(|options| options.audio.allow_clipping)
            {
                if let Some(options) = &mut saved.video_audio_options {
                    options.audio.allow_clipping = true;
                }
            }
            (saved, files)
        } else {
            if let Some(options) = &request.image_options {
                options.validate()?;
            }
            if let Some(options) = &request.audio_options {
                options.validate()?;
            }
            if let Some(options) = &request.video_options {
                options.validate()?;
            }
            if let Some(options) = &request.video_audio_options {
                options.audio.validate()?;
                if request.video_options.is_some() {
                    return Err("Choose one output for the video group.".into());
                }
            }
            if let Some(options) = &request.animation_options {
                options.validate()?;
                if request.video_options.is_some() || request.video_audio_options.is_some() {
                    return Err("Choose one output for the video group.".into());
                }
            }
            if let Some(options) = &request.image_frame_options {
                options.validate()?;
                if request.image_options.is_some() {
                    return Err("Choose one output for the image group.".into());
                }
            }
            if request.image_options.is_none()
                && request.image_frame_options.is_none()
                && request.audio_options.is_none()
                && request.video_options.is_none()
                && request.video_audio_options.is_none()
                && request.animation_options.is_none()
                && request.video_frame_options.is_none()
            {
                return Err("Choose an output format before converting.".into());
            }
            if let Some(options) = &request.video_frame_options {
                options.validate()?;
                if request.video_options.is_some()
                    || request.video_audio_options.is_some()
                    || request.animation_options.is_some()
                {
                    return Err("Choose one output for the video group.".into());
                }
            }
            if request.paths.is_empty() {
                return Err("Choose at least one file to convert.".into());
            }
            if let Some(folder) = &request.output_folder {
                if !Path::new(folder).is_absolute() || !Path::new(folder).is_dir() {
                    return Err("Choose an existing output folder.".into());
                }
            }
            let mut seen = HashSet::new();
            let mut files = Vec::new();
            for path in &request.paths {
                if !Path::new(path).is_absolute() {
                    return Err("Choose real files from your computer before converting.".into());
                }
                let canonical = Path::new(path)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(path));
                let id = canonical.to_string_lossy().into_owned();
                if !seen.insert(id.clone()) {
                    continue;
                }
                files.push(FileResult {
                    name: canonical
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    id,
                    status: FileStatus::Pending,
                    output_path: None,
                    output_bytes: None,
                    error: None,
                    warnings: vec![],
                    details: None,
                    progress: None,
                    phase: None,
                    can_allow_clipping: false,
                });
            }
            (request, files)
        };
        let job = Arc::new(Job {
            snapshot: Mutex::new(BatchSnapshot {
                id: uuid::Uuid::new_v4().to_string(),
                revision: 0,
                status: BatchStatus::Running,
                files,
            }),
            request,
            cancel: AtomicBool::new(false),
        });
        jobs.insert(label.into(), job.clone());
        Ok(job)
    }
}

pub fn run_job(
    job: &Job,
    backend: &ImageBackend,
    audio: &AudioBackend,
    report: impl Fn(BatchSnapshot),
) {
    for index in 0..job.snapshot().files.len() {
        if job.snapshot().files[index].status == FileStatus::Succeeded {
            continue;
        }
        if job.cancel.load(Ordering::Relaxed) {
            break;
        }
        let snapshot = job.update(|state| state.files[index].status = FileStatus::Running);
        let source = snapshot.files[index].id.clone();
        report(snapshot);
        let folder = job.request.output_folder.as_deref().map(Path::new);
        let result = crate::inputs::inspect_with_audio(Path::new(&source), audio, &job.cancel)
            .map_err(ConversionError::from)
            .and_then(|input| match input.kind.as_str() {
                "images" if job.request.image_frame_options.is_some() => {
                    let options = job.request.image_frame_options.as_ref().unwrap();
                    backend.convert_with_frame(
                        Path::new(&source),
                        folder,
                        &options.image,
                        options.frames.get(&source),
                        true,
                        &job.cancel,
                    )
                }
                "images" => backend.convert(
                    Path::new(&source),
                    folder,
                    job.request
                        .image_options
                        .as_ref()
                        .ok_or_else(|| "Choose an image output format.".to_string())?,
                    &job.cancel,
                ),
                "audio" => audio.convert(
                    Path::new(&source),
                    folder,
                    job.request
                        .audio_options
                        .as_ref()
                        .ok_or_else(|| "Choose an audio output format.".to_string())?,
                    &job.cancel,
                ),
                "video" if job.request.video_frame_options.is_some() => {
                    let options = job.request.video_frame_options.as_ref().unwrap();
                    input.require_frame_target(options)?;
                    audio.convert_frame(
                        backend,
                        Path::new(&source),
                        folder,
                        options,
                        &job.cancel,
                        |progress, phase| {
                            report(job.update(|state| {
                                state.files[index].progress = Some(progress);
                                state.files[index].phase = Some(phase.into());
                            }))
                        },
                    )
                }
                "video" if job.request.animation_options.is_some() => {
                    let options = job.request.animation_options.as_ref().unwrap();
                    let range = *options
                        .ranges
                        .get(&source)
                        .ok_or_else(|| "Choose a range for this video.".to_string())?;
                    audio.convert_animation(
                        backend,
                        Path::new(&source),
                        folder,
                        (options, range),
                        &job.cancel,
                        |progress, phase| {
                            report(job.update(|state| {
                                state.files[index].progress = Some(progress);
                                state.files[index].phase = Some(phase.into());
                            }))
                        },
                    )
                }
                "video" if job.request.video_audio_options.is_some() => {
                    let options = job.request.video_audio_options.as_ref().unwrap();
                    audio.extract_audio(
                        Path::new(&source),
                        folder,
                        &options.audio,
                        options.tracks.get(&source).copied(),
                        &job.cancel,
                    )
                }
                "video" => audio.convert_video(
                    Path::new(&source),
                    folder,
                    job.request
                        .video_options
                        .as_ref()
                        .ok_or_else(|| "Choose a video output format.".to_string())?,
                    &job.cancel,
                    |progress, phase| {
                        report(job.update(|state| {
                            state.files[index].progress = Some(progress);
                            state.files[index].phase = Some(phase.into());
                        }))
                    },
                ),
                _ => Err(ConversionError::Failed(
                    "This file type is not supported for conversion yet.".into(),
                )),
            });
        let snapshot = job.update(|state| {
            let file = &mut state.files[index];
            file.progress = None;
            file.phase = None;
            match result {
                Ok(output) => {
                    file.status = FileStatus::Succeeded;
                    file.output_path = Some(output.path);
                    file.output_bytes = Some(output.bytes);
                    file.warnings = output.warnings;
                    file.details = output.details;
                }
                Err(ConversionError::Cancelled) => file.status = FileStatus::Cancelled,
                Err(ConversionError::Clipping) => {
                    file.status = FileStatus::Failed;
                    file.can_allow_clipping = true;
                    file.error = Some("This conversion exceeds the integer output range. Allow clipping and retry, or choose floating-point WAV.".into());
                }
                Err(ConversionError::Failed(error)) => {
                    file.status = FileStatus::Failed;
                    file.error = Some(error);
                }
            }
        });
        report(snapshot);
    }
    report(job.update(|state| {
        let cancelled = job.cancel.load(Ordering::Relaxed);
        state.status = if cancelled {
            BatchStatus::Cancelled
        } else {
            BatchStatus::Completed
        };
        for file in &mut state.files {
            if matches!(file.status, FileStatus::Pending | FileStatus::Running) {
                file.status = FileStatus::Cancelled;
            }
        }
    }));
}

fn launch(
    job: Arc<Job>,
    backend: ImageBackend,
    audio: AudioBackend,
    channel: Channel<BatchSnapshot>,
) -> BatchSnapshot {
    let initial = job.snapshot();
    let _ = channel.send(initial.clone());
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_job(&job, &backend, &audio, |snapshot| {
                let _ = channel.send(snapshot);
            })
        }));
        if outcome.is_err() {
            let snapshot = job.update(|state| {
                state.status = BatchStatus::Completed;
                for file in &mut state.files {
                    if matches!(file.status, FileStatus::Pending | FileStatus::Running) {
                        file.status = FileStatus::Failed;
                        file.error = Some(
                            "The conversion stopped unexpectedly. You can retry this file.".into(),
                        );
                    }
                }
            });
            let _ = channel.send(snapshot);
        }
    });
    initial
}

#[tauri::command]
pub async fn start_conversion(
    window: WebviewWindow,
    manager: State<'_, JobManager>,
    backend: State<'_, ImageBackend>,
    audio: State<'_, AudioBackend>,
    request: ConversionRequest,
    on_progress: Channel<BatchSnapshot>,
) -> Result<BatchSnapshot, String> {
    let backend = backend.inner().clone();
    let check = backend.clone();
    let audio = audio.inner().clone();
    let audio_check = audio.clone();
    let preflight = request.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if preflight.image_options.is_some()
            || preflight.image_frame_options.is_some()
            || preflight.animation_options.is_some()
            || preflight.video_frame_options.is_some()
        {
            check.verify()?;
        }
        if preflight.audio_options.is_some()
            || preflight.video_options.is_some()
            || preflight.video_audio_options.is_some()
            || preflight.animation_options.is_some()
            || preflight.video_frame_options.is_some()
        {
            audio_check.verify()?;
        }
        for path in &preflight.paths {
            let input = crate::inputs::inspect_with_audio(
                Path::new(path),
                &audio_check,
                &AtomicBool::new(false),
            )?;
            let valid = match input.kind.as_str() {
                "images" if preflight.image_frame_options.is_some() => {
                    let options = preflight.image_frame_options.as_ref().unwrap();
                    options.validate()?;
                    crate::inputs::require_target(
                        Path::new(path),
                        &format!("frame-{}", options.image.target),
                    )
                    .map(|_| ())
                }
                "images" => crate::inputs::require_target(
                    Path::new(path),
                    &preflight
                        .image_options
                        .as_ref()
                        .ok_or("Choose an image output format.")?
                        .target,
                )
                .map(|_| ()),
                "audio" => input.require_audio_target(
                    preflight
                        .audio_options
                        .as_ref()
                        .ok_or("Choose an audio output format.")?,
                ),
                "video" if preflight.video_frame_options.is_some() => {
                    input.require_frame_target(preflight.video_frame_options.as_ref().unwrap())
                }
                "video" if preflight.animation_options.is_some() => {
                    input.require_animation_target(preflight.animation_options.as_ref().unwrap())
                }
                "video" if preflight.video_audio_options.is_some() => input
                    .require_video_audio_target(preflight.video_audio_options.as_ref().unwrap()),
                "video" => input.require_video_target(
                    preflight
                        .video_options
                        .as_ref()
                        .ok_or("Choose a video output format.")?,
                ),
                _ => Err("This file type is not supported for conversion yet.".into()),
            };
            valid.map_err(|message| format!("{}: {message}", input.name))?;
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "The converters could not be checked.".to_string())??;
    if window
        .app_handle()
        .get_webview_window(window.label())
        .is_none()
    {
        return Err("This batch window was closed.".into());
    }
    let job = manager.prepare(window.label(), request, false)?;
    Ok(launch(job, backend, audio, on_progress))
}

#[tauri::command]
pub fn retry_conversion(
    window: WebviewWindow,
    manager: State<'_, JobManager>,
    backend: State<'_, ImageBackend>,
    audio: State<'_, AudioBackend>,
    allow_clipping: Option<bool>,
    on_progress: Channel<BatchSnapshot>,
) -> Result<BatchSnapshot, String> {
    let mut request = manager
        .0
        .lock()
        .map_err(|_| "The batch state is unavailable.")?
        .get(window.label())
        .ok_or("There is no batch to retry.")?
        .request
        .clone();
    if allow_clipping == Some(true) {
        if let Some(options) = &mut request.audio_options {
            options.allow_clipping = true;
        }
        if let Some(options) = &mut request.video_audio_options {
            options.audio.allow_clipping = true;
        }
    }
    let job = manager.prepare(window.label(), request, true)?;
    Ok(launch(
        job,
        backend.inner().clone(),
        audio.inner().clone(),
        on_progress,
    ))
}

#[tauri::command]
pub fn cancel_conversion(
    window: WebviewWindow,
    manager: State<'_, JobManager>,
) -> Result<BatchSnapshot, String> {
    let jobs = manager
        .0
        .lock()
        .map_err(|_| "The batch state is unavailable.")?;
    let job = jobs
        .get(window.label())
        .ok_or("There is no active conversion.")?;
    Ok(job.request_cancel())
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;

#[tauri::command]
pub async fn backend_status(
    backend: State<'_, ImageBackend>,
    audio: State<'_, AudioBackend>,
) -> Result<(), String> {
    let backend = backend.inner().clone();
    let audio = audio.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        backend.verify()?;
        audio.verify()
    })
    .await
    .map_err(|_| "The converters could not be checked.".to_string())?
}

#[tauri::command]
pub fn reveal_output(
    window: WebviewWindow,
    manager: State<'_, JobManager>,
    path: String,
) -> Result<(), String> {
    let jobs = manager
        .0
        .lock()
        .map_err(|_| "The batch state is unavailable.")?;
    let job = jobs
        .get(window.label())
        .ok_or("No converted files are available.")?;
    if !job.snapshot().files.iter().any(|file| {
        file.status == FileStatus::Succeeded && file.output_path.as_ref() == Some(&path)
    }) {
        return Err("This file is not an output of this batch.".into());
    }
    if !Path::new(&path).is_file() {
        return Err("The converted file has moved or was removed.".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(format!("/select,{path}"));
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(
            Path::new(&path)
                .parent()
                .ok_or("The output folder is unavailable.")?,
        );
        command
    };
    command
        .spawn()
        .map_err(|_| "The output folder could not be opened.".to_string())?;
    Ok(())
}
