use crate::{
    audio_backend::AudioBackend,
    image_backend::{ConversionError, ImageBackend},
    video_animation::{AnimationOptions, SourceRange},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tauri::{ipc::Response, State, WebviewWindow};

#[derive(Default)]
pub struct PreviewManager {
    windows: Mutex<HashMap<String, Arc<PreviewWindow>>>,
    closing: AtomicBool,
}
#[derive(Default)]
pub struct PreviewWindow {
    current: Mutex<Option<(String, Arc<AtomicBool>)>>,
    gate: Mutex<()>,
    active: AtomicUsize,
    closed: AtomicBool,
}
impl PreviewWindow {
    pub fn busy(&self) -> bool {
        self.active.load(Ordering::SeqCst) > 0
    }
    fn cancel(&self, id: Option<&str>) {
        if let Some((current, cancel)) = &*self.current.lock().unwrap() {
            if id.is_none_or(|id| id == current) {
                cancel.store(true, Ordering::SeqCst);
            }
        }
    }
}
struct Ticket {
    window: Arc<PreviewWindow>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.window.active.fetch_sub(1, Ordering::SeqCst);
    }
}
impl PreviewManager {
    fn begin(&self, label: &str, id: String) -> Result<Ticket, String> {
        let mut windows = self.windows.lock().map_err(|_| "Preview is unavailable.")?;
        if self.closing.load(Ordering::SeqCst) {
            return Err("Recast is closing.".into());
        }
        let window = windows.entry(label.into()).or_default().clone();
        if window.closed.load(Ordering::SeqCst) {
            return Err("This window is closing.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some((_, previous)) = window.current.lock().unwrap().replace((id, cancel.clone())) {
            previous.store(true, Ordering::SeqCst);
        }
        window.active.fetch_add(1, Ordering::SeqCst);
        Ok(Ticket { window, cancel })
    }
    pub fn close(&self, label: &str) -> Option<Arc<PreviewWindow>> {
        let windows = self.windows.lock().unwrap();
        let window = windows.get(label)?.clone();
        window.closed.store(true, Ordering::SeqCst);
        window.cancel(None);
        Some(window)
    }
    pub fn remove(&self, label: &str) {
        if let Some(window) = self.windows.lock().unwrap().remove(label) {
            window.closed.store(true, Ordering::SeqCst);
            window.cancel(None);
        }
    }
    pub fn close_all(&self) -> Vec<Arc<PreviewWindow>> {
        self.closing.store(true, Ordering::SeqCst);
        self.windows
            .lock()
            .unwrap()
            .values()
            .map(|window| {
                window.closed.store(true, Ordering::SeqCst);
                window.cancel(None);
                window.clone()
            })
            .collect()
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewRequest {
    request_id: String,
    path: String,
    time: f64,
    animation: Option<(AnimationOptions, SourceRange)>,
}

#[tauri::command]
pub async fn preview_video(
    window: WebviewWindow,
    manager: State<'_, PreviewManager>,
    audio: State<'_, AudioBackend>,
    image: State<'_, ImageBackend>,
    request: PreviewRequest,
) -> Result<Response, String> {
    let PreviewRequest {
        request_id,
        path,
        time,
        animation,
    } = request;
    let ticket = manager.begin(window.label(), request_id)?;
    let audio = audio.inner().clone();
    let image = image.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Only the newest request may use a worker; superseded scrubs are
        // cancelled and reaped before the next worker starts.
        let _gate = ticket
            .window
            .gate
            .lock()
            .map_err(|_| "Preview is unavailable.".to_string())?;
        crate::image_backend::check_cancel(&ticket.cancel).map_err(message)?;
        if !Path::new(&path).is_absolute() || !Path::new(&path).is_file() {
            return Err("The video is no longer available.".into());
        }
        let bytes = if let Some((mut options, range)) = animation {
            options.size = options.size.min(320);
            let work = tempfile::Builder::new()
                .prefix("recast-preview-")
                .tempdir()
                .map_err(|e| e.to_string())?;
            let output = audio
                .render_gif(
                    &image,
                    Path::new(&path),
                    work.path(),
                    (&options, range),
                    &ticket.cancel,
                    &mut |_, _| {},
                )
                .map_err(message)?;
            if std::fs::metadata(&output).map_err(|e| e.to_string())?.len() > 16_000_000 {
                return Err("Shorten the range for a motion preview.".into());
            }
            std::fs::read(output).map_err(|e| e.to_string())?
        } else {
            audio
                .preview_frame(Path::new(&path), time, &ticket.cancel)
                .map_err(message)?
        };
        crate::image_backend::check_cancel(&ticket.cancel).map_err(message)?;
        Ok(Response::new(bytes))
    })
    .await
    .map_err(|_| "The video preview could not finish.".to_string())?
}
fn message(error: ConversionError) -> String {
    match error {
        ConversionError::Failed(s) => s,
        ConversionError::Cancelled => "Preview cancelled.".into(),
        ConversionError::Clipping => "Preview failed.".into(),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameRequest {
    request_id: String,
    path: String,
    time: f64,
}

#[tauri::command]
pub async fn preview_video_frame(
    window: WebviewWindow,
    manager: State<'_, PreviewManager>,
    audio: State<'_, AudioBackend>,
    request: FrameRequest,
) -> Result<Response, String> {
    let ticket = manager.begin(window.label(), request.request_id)?;
    let audio = audio.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _gate = ticket
            .window
            .gate
            .lock()
            .map_err(|_| "Preview is unavailable.")?;
        crate::image_backend::check_cancel(&ticket.cancel).map_err(message)?;
        let source = Path::new(&request.path);
        if !source.is_absolute() || !source.is_file() {
            return Err("The video is no longer available.".into());
        }
        let (frame, bytes) = audio
            .preview_selected_frame(source, request.time, &ticket.cancel)
            .map_err(message)?;
        // One atomic binary response: little-endian JSON length, JSON metadata,
        // then BMP. Avoid a base64 copy and a race between image and timestamp.
        let json = serde_json::to_vec(&frame).map_err(|e| e.to_string())?;
        let mut response = Vec::with_capacity(4 + json.len() + bytes.len());
        response.extend((json.len() as u32).to_le_bytes());
        response.extend(json);
        response.extend(bytes);
        crate::image_backend::check_cancel(&ticket.cancel).map_err(message)?;
        Ok(Response::new(response))
    })
    .await
    .map_err(|_| "The frame preview could not finish.".to_string())?
}
#[tauri::command]
pub fn cancel_video_preview(
    window: WebviewWindow,
    manager: State<'_, PreviewManager>,
    request_id: String,
) {
    if let Some(window) = manager.windows.lock().unwrap().get(window.label()) {
        window.cancel(Some(&request_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_request_wins_but_old_cleanup_cannot_cancel_a_new_preview() {
        let manager = PreviewManager::default();
        let old = manager.begin("one", "old".into()).unwrap();
        let new = manager.begin("one", "new".into()).unwrap();
        assert!(old.cancel.load(Ordering::SeqCst));
        new.window.cancel(Some("old"));
        assert!(!new.cancel.load(Ordering::SeqCst));
        let other = manager.begin("two", "other".into()).unwrap();
        let window = manager.close("one").unwrap();
        assert!(window.busy());
        assert!(new.cancel.load(Ordering::SeqCst));
        assert!(!other.cancel.load(Ordering::SeqCst));
        assert!(manager.begin("one", "late".into()).is_err());
        drop(old);
        drop(new);
        assert!(!window.busy());
        let windows = manager.close_all();
        assert!(other.cancel.load(Ordering::SeqCst));
        drop(other);
        assert!(windows.iter().all(|w| !w.busy()));
        assert!(manager.begin("three", "no".into()).is_err());
    }
}
