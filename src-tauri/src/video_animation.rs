use crate::{
    audio_backend::AudioBackend,
    image_backend::{check_cancel, publish, ConversionError, ImageBackend, OutputFile},
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRange {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimationOptions {
    pub size: u32,
    pub frame_rate: u32,
    pub colors: u16,
    pub dither: bool,
    pub ranges: BTreeMap<String, SourceRange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub stream_index: u32,
}

pub fn inspect(probe: &Value) -> Result<AnimationInfo, String> {
    let mut picture = probe.clone();
    let streams = picture["streams"]
        .as_array_mut()
        .ok_or("No video track was found.")?;
    streams.retain(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1);
    if streams.len() != 1 {
        return Err("GIF needs a single video track.".into());
    }
    let stream_index = streams[0]["index"]
        .as_u64()
        .ok_or("The video track has no index.")? as u32;
    // GIF deliberately omits audio, subtitles and chapters. Their codecs do not
    // constrain this destination, but picture preservation rules still apply.
    picture["chapters"] = serde_json::json!([]);
    let info = crate::video_backend::parse(picture)?.ok_or("No video track was found.")?;
    if let Some(issue) = &info.issue {
        return Err(issue.clone());
    }
    let duration = crate::video_backend::video_end(&info)
        .unwrap_or(info.duration)
        .min(info.duration);
    if !duration.is_finite() || duration < 0.01 {
        return Err("This video is too short for GIF timing.".into());
    }
    let (width, height) = if info.rotation.abs() % 180 == 90 {
        (info.height, info.width)
    } else {
        (info.width, info.height)
    };
    Ok(AnimationInfo {
        duration,
        width,
        height,
        stream_index,
    })
}

impl AnimationOptions {
    pub fn validate(&self) -> Result<(), String> {
        if ![320, 480, 720, 960].contains(&self.size)
            || ![10, 15, 20, 25, 30].contains(&self.frame_rate)
            || ![32, 64, 128, 256].contains(&self.colors)
        {
            return Err("Choose supported GIF size, frame rate, and colors.".into());
        }
        Ok(())
    }
    pub fn resolve(
        &self,
        info: &AnimationInfo,
        range: SourceRange,
    ) -> Result<(u32, u32, usize), String> {
        self.validate()?;
        range.validate(info.duration)?;
        let (width, height) = info.dimensions(self.size);
        let frames = ((range.end - range.start) * f64::from(self.frame_rate) - 1e-8)
            .ceil()
            .max(1.0) as usize;
        // Animated GIF delays below 20 ms are stretched by common viewers.
        let frames = frames.min((((range.end - range.start) * 100.0).round() as usize / 2).max(1));
        if frames > crate::animation::MAX_FRAMES
            || u64::from(width) * u64::from(height) * frames as u64 > 100_000_000
        {
            return Err(
                "This GIF would be too large. Shorten the range, or lower size or frame rate."
                    .into(),
            );
        }
        Ok((width, height, frames))
    }
}
impl SourceRange {
    pub fn validate(self, duration: f64) -> Result<(), String> {
        if !self.start.is_finite()
            || !self.end.is_finite()
            || self.start < 0.0
            || self.end - self.start < 0.01 - 1e-8
            || self.end > duration + 0.001
        {
            return Err("Choose a range inside this video, at least 0.01 seconds long.".into());
        }
        Ok(())
    }
}
impl AnimationInfo {
    pub fn dimensions(&self, size: u32) -> (u32, u32) {
        let factor = (f64::from(size) / f64::from(self.width.max(self.height))).min(1.0);
        (
            (f64::from(self.width) * factor).round().max(1.0) as u32,
            (f64::from(self.height) * factor).round().max(1.0) as u32,
        )
    }
}

// The image worker's policy accepts BMP, not raw pixels. A small top-down 24-bit
// BMP bridge avoids enabling extra coders/delegates or platform media codecs.
fn bmp(rgb: &[u8], width: u32, height: u32) -> Vec<u8> {
    let stride = (width * 3).div_ceil(4) * 4;
    let size = 54 + stride * height;
    let mut bytes = Vec::with_capacity(size as usize);
    bytes.extend(b"BM");
    bytes.extend(size.to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(54u32.to_le_bytes());
    bytes.extend(40u32.to_le_bytes());
    bytes.extend((width as i32).to_le_bytes());
    bytes.extend((-(height as i32)).to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(24u16.to_le_bytes());
    bytes.extend([0; 24]);
    for row in rgb.chunks_exact(width as usize * 3) {
        for pixel in row.as_chunks::<3>().0 {
            bytes.extend([pixel[2], pixel[1], pixel[0]]);
        }
        bytes.extend(std::iter::repeat_n(0, (stride - width * 3) as usize));
    }
    bytes
}

struct PictureRequest {
    range: SourceRange,
    size: (u32, u32),
    rate: Option<u32>,
    frames: usize,
    stream: u32,
}

impl AudioBackend {
    fn decode_pictures(
        &self,
        source: &Path,
        work: &Path,
        request: PictureRequest,
        cancel: &AtomicBool,
        report: &mut impl FnMut(f64),
    ) -> Result<std::path::PathBuf, ConversionError> {
        let PictureRequest {
            range,
            size,
            rate,
            frames,
            stream,
        } = request;
        let output = work.join("frames.rgb");
        let progress = work.join("frames.progress");
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
                "-noaccurate_seek",
                "-ss",
                &format!("{:.6}", range.start),
                "-i",
            ])
            .arg(source)
            .args(["-map", &format!("0:{stream}"), "-an", "-sn", "-dn"]);
        let mut filters = Vec::new();
        if let Some(rate) = rate {
            filters.push(format!("trim=end={:.6}", range.end - range.start));
            filters.push(format!("fps={rate}:start_time=0:round=up:eof_action=pass"));
            command.args(["-t", &format!("{:.6}", range.end - range.start)]);
        } else {
            // Keep the frame covering this instant, including the last frame.
            // Accurate input seeking would discard it when its PTS is earlier.
            filters.push("fps=100:start_time=0:round=up:eof_action=pass".into());
        }
        filters.push(format!(
            "scale={}:{}:flags=lanczos,setsar=1",
            size.0, size.1
        ));
        command
            .args([
                "-vf",
                &filters.join(","),
                "-frames:v",
                &frames.to_string(),
                "-pix_fmt",
                "rgb24",
                "-c:v",
                "rawvideo",
                "-threads:v",
                "1",
                "-f",
                "rawvideo",
                "-progress",
            ])
            .arg(&progress)
            .arg(&output)
            .stdout(Stdio::null());
        crate::video_backend::run(
            &mut command,
            &work.join("decode.log"),
            Some(&progress),
            cancel,
            300,
            report,
        )?;
        Ok(output)
    }
    pub fn preview_frame(
        &self,
        source: &Path,
        time: f64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, ConversionError> {
        let probe = self.probe_media(source, cancel)?;
        let info = inspect(&probe)?;
        if !time.is_finite() || time < 0.0 || time > info.duration {
            return Err("Choose a position inside this video.".to_string().into());
        }
        // The EOF boundary has no picture after it. Stay one source-clock tick
        // inside it, rather than subtracting an estimated frame duration. This
        // also prevents input timestamp rounding from turning a near-EOF seek
        // into an empty decode on coarse clocks such as QuickTime's 1/600 s.
        let tick = probe["streams"]
            .as_array()
            .and_then(|streams| {
                streams
                    .iter()
                    .find(|s| s["index"].as_u64() == Some(u64::from(info.stream_index)))
            })
            .and_then(|stream| stream["time_base"].as_str())
            .and_then(|base| base.split_once('/'))
            .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
            .filter(|tick| tick.is_finite() && *tick > 0.0)
            .ok_or("The video clock could not be read.".to_string())?;
        let time = time.min((info.duration - tick).max(0.0));
        let work = tempfile::tempdir().map_err(|e| e.to_string())?;
        let size = info.dimensions(480);
        let raw = self.decode_pictures(
            source,
            work.path(),
            PictureRequest {
                range: SourceRange {
                    start: time,
                    end: time,
                },
                size,
                rate: None,
                frames: 1,
                stream: info.stream_index,
            },
            cancel,
            &mut |_| {},
        )?;
        let bytes = fs::read(raw).map_err(|e| e.to_string())?;
        if bytes.len() != size.0 as usize * size.1 as usize * 3 {
            return Err("No frame was found at this position.".to_string().into());
        }
        Ok(bmp(&bytes, size.0, size.1))
    }
    pub fn render_gif(
        &self,
        image: &ImageBackend,
        source: &Path,
        work: &Path,
        settings: (&AnimationOptions, SourceRange),
        cancel: &AtomicBool,
        report: &mut impl FnMut(f64, &str),
    ) -> Result<std::path::PathBuf, ConversionError> {
        let (options, range) = settings;
        let info = inspect(&self.probe_media(source, cancel)?)?;
        let (width, height, expected) = options.resolve(&info, range)?;
        report(0.02, "Reading selected range");
        let raw = self.decode_pictures(
            source,
            work,
            PictureRequest {
                range,
                size: (width, height),
                rate: Some(options.frame_rate),
                frames: expected,
                stream: info.stream_index,
            },
            cancel,
            &mut |t| {
                report(
                    (t / (range.end - range.start)).clamp(0.0, 1.0) * 0.65,
                    "Reading selected range",
                )
            },
        )?;
        let frame_bytes = u64::from(width) * u64::from(height) * 3;
        let length = fs::metadata(&raw).map_err(|e| e.to_string())?.len();
        let count = (length / frame_bytes) as usize;
        if length % frame_bytes != 0 || count == 0 || count > expected || count + 1 < expected {
            return Err("The selected range could not be decoded completely. Try a range within the visible video.".to_string().into());
        }
        let mut input = File::open(raw).map_err(|e| e.to_string())?;
        let mut pixels = vec![0; frame_bytes as usize];
        let mut names = Vec::new();
        for i in 0..count {
            check_cancel(cancel)?;
            input.read_exact(&mut pixels).map_err(|e| e.to_string())?;
            let name = format!("frame-{i:04}.bmp");
            fs::write(work.join(&name), bmp(&pixels, width, height)).map_err(|e| e.to_string())?;
            names.push(name);
        }
        let output = work.join("animation.gif");
        let mut command = image.command();
        command
            .current_dir(work)
            .env("MAGICK_TEMPORARY_PATH", work)
            .args(["-delay", "10"])
            .args(&names)
            .args([
                "-strip",
                "-dither",
                if options.dither {
                    "FloydSteinberg"
                } else {
                    "None"
                },
                "-colors",
                &options.colors.to_string(),
                "+remap",
                "-loop",
                "0",
                "-dispose",
                "None",
            ])
            .arg(&output)
            .stdout(Stdio::null());
        report(0.7, "Encoding GIF");
        crate::video_backend::run(
            &mut command,
            &work.join("encode.log"),
            None,
            cancel,
            300,
            &mut |_| {},
        )?;
        check_cancel(cancel)?;
        let mut file = File::options()
            .read(true)
            .write(true)
            .open(&output)
            .map_err(|e| e.to_string())?;
        let actual = crate::animation::inspect_gif(&mut file)?;
        if actual.width != width
            || actual.height != height
            || actual.delays.len() != count
            || (count > 1 && actual.loops != 0)
        {
            return Err("The GIF did not retain the requested frames or dimensions."
                .to_string()
                .into());
        }
        let ticks = ((range.end - range.start) * 100.0).round().max(1.0) as u32;
        let mut previous = 0;
        let mut delays: Vec<u32> = (1..=count)
            .map(|i| {
                let end = if i == count {
                    ticks
                } else {
                    ((i as f64 / f64::from(options.frame_rate)) * 100.0).round() as u32
                };
                let ms = end.saturating_sub(previous) * 10;
                previous = end;
                ms
            })
            .collect();
        if count > 1 && delays[count - 1] < 20 {
            let borrowed = 20 - delays[count - 1];
            delays[count - 2] = delays[count - 2].saturating_sub(borrowed);
            delays[count - 1] = 20;
        }
        actual.write_gif_delays(&mut file, &delays)?;
        report(0.98, "Checking GIF");
        Ok(output)
    }
    pub fn convert_gif(
        &self,
        image: &ImageBackend,
        source: &Path,
        folder: Option<&Path>,
        settings: (&AnimationOptions, SourceRange),
        cancel: &AtomicBool,
        mut report: impl FnMut(f64, &str),
    ) -> Result<OutputFile, ConversionError> {
        let (options, range) = settings;
        check_cancel(cancel)?;
        options.validate()?;
        let source = source
            .canonicalize()
            .map_err(|_| "The source file is no longer available.".to_string())?;
        let parent = folder.unwrap_or_else(|| source.parent().unwrap());
        let work = tempfile::Builder::new()
            .prefix("recast-gif-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let staged = work.path().join("source");
        let mut input = File::open(&source).map_err(|e| e.to_string())?;
        let mut copy = File::create(&staged).map_err(|e| e.to_string())?;
        let mut buffer = [0; 65536];
        loop {
            check_cancel(cancel)?;
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            copy.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        drop(copy);
        let gif = self.render_gif(
            image,
            &staged,
            work.path(),
            (options, range),
            cancel,
            &mut report,
        )?;
        let mut output = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(".gif")
            .tempfile_in(parent)
            .map_err(|_| "Cannot write to this output folder.".to_string())?;
        let mut input = File::open(gif).map_err(|e| e.to_string())?;
        loop {
            check_cancel(cancel)?;
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        output.as_file().sync_all().map_err(|e| e.to_string())?;
        let bytes = output
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        check_cancel(cancel)?;
        let path = publish(
            output,
            source.file_stem().unwrap_or_default(),
            parent,
            "gif",
        )?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
            warnings: vec![],
            details: Some(format!(
                "{:.2}–{:.2} s · {} fps · GIF · Loops continuously · No audio",
                range.start, range.end, options.frame_rate
            )),
        })
    }
}
