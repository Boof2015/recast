use crate::{
    audio_backend::AudioBackend,
    image_backend::{
        check_cancel, publish, ConversionError, ImageBackend, ImageOptions, OutputFile,
    },
    video_animation::{bmp, inspect, AnimationInfo},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::Stdio,
    sync::atomic::AtomicBool,
};

pub const TARGETS: [&str; 6] = ["png", "jpeg", "webp", "avif", "tiff", "bmp"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameSelection {
    // Keep integer source timestamps out of JavaScript's floating-point numbers.
    pub pts: String,
    pub time: f64,
    pub source: String,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramePreview {
    pub selection: FrameSelection,
    pub previous: Option<f64>,
    pub next: Option<f64>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameOptions {
    pub image: ImageOptions,
    pub frames: BTreeMap<String, FrameSelection>,
}
impl FrameOptions {
    pub fn validate(&self) -> Result<(), String> {
        self.image.validate()?;
        if !TARGETS.contains(&self.image.target.as_str()) {
            return Err("Choose a still image format.".into());
        }
        for selection in self.frames.values() {
            selection.timestamp()?;
        }
        Ok(())
    }
}
impl FrameSelection {
    fn timestamp(&self) -> Result<i64, String> {
        if !self.time.is_finite() || self.time < 0.0 {
            return Err("Choose a frame inside this video.".into());
        }
        self.pts
            .parse::<i64>()
            .ok()
            .filter(|pts| *pts < i64::MAX)
            .ok_or_else(|| "The selected frame has an invalid timestamp.".into())
    }
}

pub(crate) fn fingerprint(source: &Path) -> Result<String, String> {
    let meta = fs::metadata(source).map_err(|_| "The file is no longer available.")?;
    let time = meta
        .modified()
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    Ok(format!("{}:{}", meta.len(), time.as_nanos()))
}
pub(crate) fn unchanged(source: &Path, expected: &str) -> Result<(), String> {
    if fingerprint(source)? != expected {
        return Err("This file changed. Preview its frame again before converting.".into());
    }
    Ok(())
}
struct Clock {
    tick: f64,
    origin: f64,
}
impl Clock {
    fn read(probe: &Value, info: &AnimationInfo) -> Result<Self, String> {
        let tick = probe["streams"]
            .as_array()
            .and_then(|streams| {
                streams
                    .iter()
                    .find(|s| s["index"].as_u64() == Some(u64::from(info.stream_index)))
            })
            .and_then(|s| s["time_base"].as_str())
            .and_then(|s| s.split_once('/'))
            .and_then(|(n, d)| Some(n.parse::<f64>().ok()? / d.parse::<f64>().ok()?))
            .filter(|t| t.is_finite() && *t > 0.0)
            .ok_or("The video clock could not be read.")?;
        let origin = probe["format"]["start_time"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        if !origin.is_finite() {
            return Err("The video clock could not be read.".into());
        }
        Ok(Self { tick, origin })
    }
    fn time(&self, pts: i64) -> f64 {
        (pts as f64 * self.tick - self.origin).max(0.0)
    }
}

impl AudioBackend {
    /// Read decoded presentation timestamps around a seek, including neighbours.
    /// This works with reordered frames and VFR; average FPS is never a frame ID.
    fn frame_at(
        &self,
        source: &Path,
        time: f64,
        info: &AnimationInfo,
        clock: &Clock,
        work: &Path,
        cancel: &AtomicBool,
    ) -> Result<FramePreview, ConversionError> {
        if !time.is_finite() || time < 0.0 || time > info.duration {
            return Err("Choose a position inside this video.".to_string().into());
        }
        let identity = fingerprint(source)?;
        let mut margin = 1.0;
        loop {
            let start = (time - margin).max(0.0);
            let end = (time + margin).min(info.duration);
            let timestamps = work.join("timestamps.json");
            let mut command = self.command("ffprobe");
            command
                .args([
                    "-v",
                    "error",
                    "-threads",
                    "2",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-select_streams",
                    &info.stream_index.to_string(),
                    "-read_intervals",
                    // Absolute end: +duration is measured from the earlier keyframe
                    // that demuxer seeking actually reaches, not our requested start.
                    &format!(
                        "{:.6}%{:.6}",
                        start + clock.origin,
                        end + clock.origin + clock.tick
                    ),
                    "-show_frames",
                    "-show_entries",
                    "frame=best_effort_timestamp:frame_side_data=",
                    "-of",
                    "json",
                ])
                .arg(source)
                .stdout(File::create(&timestamps).map_err(|e| e.to_string())?);
            crate::video_backend::run(
                &mut command,
                &work.join("timestamps.log"),
                None,
                cancel,
                120,
                &mut |_| {},
            )?;
            if fs::metadata(&timestamps).map_err(|e| e.to_string())?.len() > 8_000_000 {
                return Err("This video's frame timing is too large to read safely."
                    .to_string()
                    .into());
            }
            let data: Value =
                serde_json::from_reader(File::open(timestamps).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            let frames = data["frames"]
                .as_array()
                .ok_or("No video frames were found.".to_string())?;
            let points: Vec<i64> = frames
                .iter()
                .map(|f| {
                    f["best_effort_timestamp"]
                        .as_i64()
                        .ok_or("A video frame has no usable timestamp.".to_string())
                })
                .collect::<Result<_, _>>()?;
            if points.windows(2).any(|p| p[0] >= p[1]) {
                return Err(
                    "This video's frame timestamps are not unique and increasing."
                        .to_string()
                        .into(),
                );
            }
            let index = points
                .iter()
                .rposition(|pts| clock.time(*pts) <= time + clock.tick * 0.001)
                .unwrap_or(0);
            // A sparse video may have seconds between pictures. Expand until
            // both neighbours are known, or the actual source boundary is reached.
            if !points.is_empty()
                && (index > 0 || start == 0.0)
                && (index + 1 < points.len() || end == info.duration)
            {
                unchanged(source, &identity)?;
                return Ok(FramePreview {
                    selection: FrameSelection {
                        pts: points[index].to_string(),
                        time: clock.time(points[index]),
                        source: identity,
                    },
                    previous: index
                        .checked_sub(1)
                        .map(|i| clock.time(points[i]))
                        .filter(|time| *time < clock.time(points[index])),
                    next: points.get(index + 1).map(|pts| clock.time(*pts)),
                });
            }
            if start == 0.0 && end == info.duration {
                return Err("No frame was found at this position.".to_string().into());
            }
            margin *= 4.0;
        }
    }

    fn render_selected_frame(
        &self,
        source: &Path,
        selection: &FrameSelection,
        picture: (&AnimationInfo, &Clock),
        size: (u32, u32),
        work: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, ConversionError> {
        let (info, clock) = picture;
        let pts = selection.timestamp()?;
        if selection.time > info.duration
            || (clock.time(pts) - selection.time).abs() > clock.tick * 0.01
        {
            return Err("The selected frame no longer matches this video."
                .to_string()
                .into());
        }
        let raw = work.join("frame.rgb");
        let mut command = self.command("ffmpeg");
        command
            .args([
                "-hide_banner",
                "-v",
                "error",
                "-nostdin",
                "-y",
                "-xerror",
                "-err_detect",
                "crccheck+explode",
                "-threads",
                "2",
                "-filter_threads",
                "1",
                "-protocol_whitelist",
                "file,pipe",
                "-copyts",
                "-noaccurate_seek",
                "-ss",
                &format!("{:.6}", (selection.time - 1.0).max(0.0)),
                "-i",
            ])
            .arg(source)
            .args([
                "-map",
                &format!("0:{}", info.stream_index),
                "-an",
                "-sn",
                "-dn",
                "-vf",
                &format!(
                    "trim=start_pts={pts}:end_pts={},scale={}:{}:flags=lanczos,setsar=1",
                    pts + 1,
                    size.0,
                    size.1
                ),
                "-frames:v",
                "1",
                "-fps_mode",
                "passthrough",
                "-threads:v",
                "1",
                "-pix_fmt",
                "rgb24",
                "-f",
                "rawvideo",
            ])
            .arg(&raw)
            .stdout(Stdio::null());
        crate::video_backend::run(
            &mut command,
            &work.join("frame.log"),
            None,
            cancel,
            120,
            &mut |_| {},
        )?;
        let bytes = fs::read(raw).map_err(|e| e.to_string())?;
        if bytes.len() != size.0 as usize * size.1 as usize * 3 {
            return Err("The selected frame could not be decoded. Preview it again."
                .to_string()
                .into());
        }
        Ok(bmp(&bytes, size.0, size.1))
    }

    pub fn preview_selected_frame(
        &self,
        source: &Path,
        time: f64,
        cancel: &AtomicBool,
    ) -> Result<(FramePreview, Vec<u8>), ConversionError> {
        check_cancel(cancel)?;
        let identity = fingerprint(source)?;
        let probe = self.probe_media(source, cancel)?;
        let info = inspect(&probe)?;
        let clock = Clock::read(&probe, &info)?;
        let work = tempfile::tempdir().map_err(|e| e.to_string())?;
        let frame = self.frame_at(source, time, &info, &clock, work.path(), cancel)?;
        let bytes = self.render_selected_frame(
            source,
            &frame.selection,
            (&info, &clock),
            info.dimensions(480),
            work.path(),
            cancel,
        )?;
        unchanged(source, &identity)?;
        Ok((frame, bytes))
    }

    pub fn convert_frame(
        &self,
        image: &ImageBackend,
        source: &Path,
        folder: Option<&Path>,
        options: &FrameOptions,
        cancel: &AtomicBool,
        mut report: impl FnMut(f64, &str),
    ) -> Result<OutputFile, ConversionError> {
        options.validate()?;
        check_cancel(cancel)?;
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        let parent = folder.unwrap_or_else(|| source.parent().unwrap());
        let identity = fingerprint(&source)?;
        let selected = options.frames.get(&source.to_string_lossy().into_owned());
        if let Some(selected) = selected {
            unchanged(&source, &selected.source)?;
        }
        let work = tempfile::Builder::new()
            .prefix("recast-frame-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let staged = work.path().join("source");
        report(0.02, "Reading selected frame");
        copy(
            &source,
            &mut File::create(&staged).map_err(|e| e.to_string())?,
            cancel,
        )?;
        unchanged(&source, &identity)?;
        let probe = self.probe_media(&staged, cancel)?;
        let info = inspect(&probe)?;
        let clock = Clock::read(&probe, &info)?;
        // Unvisited batch members use the first displayed frame. Visited ones
        // export the exact saved PTS, including when retried later.
        let selection = match selected {
            Some(selection) => selection.clone(),
            None => {
                self.frame_at(&staged, 0.0, &info, &clock, work.path(), cancel)?
                    .selection
            }
        };
        let bytes = self.render_selected_frame(
            &staged,
            &selection,
            (&info, &clock),
            (info.width, info.height),
            work.path(),
            cancel,
        )?;
        let frame = work.path().join("frame.bmp");
        fs::write(&frame, bytes).map_err(|e| e.to_string())?;
        let mut settings = options.image.clone();
        settings.metadata = false;
        report(0.65, "Saving image");
        let converted = image.convert(&frame, Some(work.path()), &settings, cancel)?;
        let extension = crate::image_format::ImageFormat::from_id(&settings.target)?.extension();
        let mut output = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{extension}"))
            .tempfile_in(parent)
            .map_err(|_| "Cannot write to this output folder.".to_string())?;
        copy(Path::new(&converted.path), output.as_file_mut(), cancel)?;
        output.as_file().sync_all().map_err(|e| e.to_string())?;
        check_cancel(cancel)?;
        let path = publish(
            output,
            source.file_stem().unwrap_or_default(),
            parent,
            extension,
        )?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes: converted.bytes,
            warnings: converted.warnings,
            details: Some(format!("Frame at {:.6} s", selection.time)),
        })
    }
}
fn copy(source: &Path, output: &mut File, cancel: &AtomicBool) -> Result<(), ConversionError> {
    let mut input = File::open(source).map_err(|e| e.to_string())?;
    let mut buffer = [0; 65536];
    loop {
        check_cancel(cancel)?;
        let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
    }
    Ok(())
}
