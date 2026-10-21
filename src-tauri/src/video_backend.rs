//! Video conversion through the bundled media worker. All publication is no-clobber.
use crate::{
    audio_backend::AudioBackend,
    image_backend::{check_cancel, publish, ConversionError, OutputFile},
};
#[path = "video_encoder.rs"]
mod encoder;
#[path = "video_remux.rs"]
mod remux;
#[path = "video_size.rs"]
mod size;
pub use encoder::Acceleration;
use encoder::Encoder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use size::{EncodeAttempt, SizePlan};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoOptions {
    pub target: String,
    pub quality: u8,
    pub resolution: Option<u32>,
    pub frame_rate: Option<u32>,
    pub metadata: bool,
    #[serde(default)]
    pub acceleration: Acceleration,
    #[serde(default)]
    pub size_limit: Option<u64>,
    #[serde(default)]
    pub auto_resolution: bool,
    #[serde(default)]
    pub copy_streams: bool,
}
impl VideoOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !["mp4", "mkv", "mov", "webm"].contains(&self.target.as_str()) {
            return Err("Choose MP4, MKV, MOV, or WebM for this video conversion.".into());
        }
        if self.copy_streams
            && (!["mkv", "mov", "webm"].contains(&self.target.as_str())
                || self.size_limit.is_some()
                || self.resolution.is_some()
                || self.frame_rate.is_some())
        {
            return Err("Keep original is available for MKV, MOV, and WebM without size, resolution, or frame-rate changes.".into());
        }
        if !self.copy_streams && self.size_limit.is_none() && !(1..=100).contains(&self.quality) {
            return Err("Choose video quality from 1 to 100.".into());
        }
        if self
            .size_limit
            .is_some_and(|bytes| !(100_000..=10_000_000_000).contains(&bytes))
        {
            return Err("Choose a maximum size from 0.1 to 10,000 MB.".into());
        }
        if self.auto_resolution && (self.size_limit.is_none() || self.resolution.is_some()) {
            return Err("Automatic resolution is available only in Target size mode without a fixed resolution.".into());
        }
        if self
            .resolution
            .is_some_and(|r| ![480, 720, 1080].contains(&r))
            || self.frame_rate.is_some_and(|r| ![24, 30, 60].contains(&r))
        {
            return Err("Choose a supported resolution and frame rate.".into());
        }
        Ok(())
    }
    fn muxer(&self) -> &str {
        if self.target == "mkv" {
            "matroska"
        } else {
            &self.target
        }
    }
    fn precision(&self) -> f64 {
        if ["mkv", "webm"].contains(&self.target.as_str()) {
            0.00101
        } else {
            0.00001
        }
    }
    fn configure_muxer(
        &self,
        command: &mut Command,
        info: &VideoInfo,
    ) -> Result<(), ConversionError> {
        if ["mkv", "webm"].contains(&self.target.as_str()) {
            command.args(["-default_mode", "passthrough"]);
        } else {
            // A MOV edit expressed in rounded microseconds can expose an extra
            // sample in AVFoundation. Use an exact common audio clock, with at
            // least microsecond precision for the video timeline.
            let mut clock = 1_u64;
            if self.target == "mov" {
                for audio in streams(&info.probe).filter(|s| s["codec_type"] == "audio") {
                    let rate = num(&audio["sample_rate"]).unwrap() as u64;
                    let (mut a, mut b) = (clock, rate);
                    while b != 0 {
                        (a, b) = (b, a % b);
                    }
                    clock = (clock / a).checked_mul(rate).filter(|value| *value <= i32::MAX as u64).ok_or_else(|| "These audio sample rates cannot share an exact MOV timeline. Choose MKV to retain them.".to_string())?;
                }
            }
            clock *= 1_000_000_u64.div_ceil(clock);
            command.args([
                "-movflags",
                "+faststart",
                "-use_editlist",
                "1",
                "-movie_timescale",
                &clock.to_string(),
                "-video_track_timescale",
                "1000000",
                "-write_tmcd",
                "0",
            ]);
        }
        command.args(["-f", self.muxer()]);
        Ok(())
    }
    fn copies_audio(&self, audio: &Value) -> bool {
        self.copy_streams
            || (self.size_limit.is_none()
                && (self.target == "mkv"
                    || (self.target != "webm" && audio["codec_name"] == "aac")
                    || (self.target == "webm" && webm_audio_copy(audio))
                    || (self.target == "mov" && mov_audio_copy(audio))))
    }
    fn audio_rate(&self, audio: &Value) -> u32 {
        if self.target == "webm" && !self.copies_audio(audio) {
            48000
        } else {
            num(&audio["sample_rate"]).unwrap_or(0.0) as u32
        }
    }
    pub fn dimensions(&self, info: &VideoInfo) -> (u32, u32) {
        let (w, h) = if info.rotation.abs() == 90 || info.rotation.abs() == 270 {
            (info.height, info.width)
        } else {
            (info.width, info.height)
        };
        let factor = self
            .resolution
            .map(|r| {
                (f64::from(r) / f64::from(w.min(h)))
                    .min((f64::from(r) * 16.0 / 9.0) / f64::from(w.max(h)))
                    .min(1.0)
            })
            .unwrap_or(1.0);
        (
            ((f64::from(w) * factor) as u32 / 2 * 2).max(2),
            ((f64::from(h) * factor) as u32 / 2 * 2).max(2),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    pub container: String,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub rotation: i32,
    pub duration: f64,
    pub frame_rate: f64,
    pub audio_tracks: usize,
    pub subtitle_tracks: usize,
    pub issue: Option<String>,
    pub aac_issue: Option<String>,
    pub mov_copy_issue: Option<String>,
    pub webm_copy_issue: Option<String>,
    pub opus_issue: Option<String>,
    #[serde(skip)]
    probe: Value,
}
fn num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}
fn ratio(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let (a, b) = s.split_once('/').or_else(|| s.split_once(':'))?;
    let result = a.parse::<f64>().ok()? / b.parse::<f64>().ok()?;
    (result.is_finite() && result > 0.0).then_some(result)
}
fn streams(probe: &Value) -> impl Iterator<Item = &Value> {
    probe["streams"].as_array().into_iter().flatten()
}
fn video_stream(probe: &Value) -> Option<&Value> {
    streams(probe).find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1)
}
fn video_end(info: &VideoInfo) -> Option<f64> {
    let video = video_stream(&info.probe)?;
    let end = num(&video["duration"])
        .map(|duration| duration + num(&video["start_time"]).unwrap_or(0.0))
        .or_else(|| {
            let parts: Vec<_> = video["tags"]["DURATION"].as_str()?.split(':').collect();
            if parts.len() != 3 {
                return None;
            }
            Some(
                parts[0].parse::<f64>().ok()? * 3600.0
                    + parts[1].parse::<f64>().ok()? * 60.0
                    + parts[2].parse::<f64>().ok()?,
            )
        })?;
    Some(end - num(&info.probe["format"]["start_time"]).unwrap_or(0.0))
}
fn rotation(stream: &Value) -> i32 {
    stream["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|s| num(&s["rotation"]))
        .unwrap_or(0.0)
        .round() as i32
}
fn parse(probe: Value) -> Result<Option<VideoInfo>, String> {
    let Some(video) = video_stream(&probe) else {
        return Ok(None);
    };
    let width = num(&video["width"]).unwrap_or(0.0) as u32;
    let height = num(&video["height"]).unwrap_or(0.0) as u32;
    let duration = num(&probe["format"]["duration"])
        .filter(|v| *v > 0.0 && *v <= 86400.0)
        .ok_or("This video has no supported finite duration (up to 24 hours).")?;
    let fps = ratio(&video["avg_frame_rate"])
        .or_else(|| ratio(&video["r_frame_rate"]))
        .unwrap_or(0.0);
    let rot = rotation(video);
    let exact_rotation = video["side_data_list"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|s| num(&s["rotation"]))
        .unwrap_or(0.0);
    let codec = video["codec_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let container = probe["format"]["format_name"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let audios: Vec<_> = streams(&probe)
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    let subtitles: Vec<_> = streams(&probe)
        .filter(|s| s["codec_type"] == "subtitle")
        .collect();
    let mut issue = None;
    let mut reject = |message: &str| {
        if issue.is_none() {
            issue = Some(message.to_string());
        }
    };
    if !["mov,mp4,m4a,3gp,3g2,mj2", "matroska,webm", "avi"].contains(&container.as_str()) {
        reject("This video build accepts MP4/MOV, MKV/WebM, and AVI containers.");
    }
    if streams(&probe)
        .filter(|s| s["codec_type"] == "video")
        .count()
        != 1
        || streams(&probe).any(|s| {
            !["video", "audio", "subtitle"].contains(&s["codec_type"].as_str().unwrap_or(""))
        })
    {
        reject("This video contains extra video, attachment, or data tracks. Retaining those tracks is not supported yet.");
    }
    if probe["chapters"]
        .as_array()
        .is_some_and(|chapters| !chapters.is_empty())
    {
        reject("Chaptered video is not supported yet. Recast will not silently discard chapters.");
    }
    if ![
        "h264", "hevc", "vp8", "vp9", "av1", "mpeg4", "mjpeg", "prores",
    ]
    .contains(&codec.as_str())
    {
        reject(&format!(
            "{} video is not supported yet.",
            codec.to_uppercase()
        ));
    }
    if width < 2
        || height < 2
        || width > 8192
        || height > 8192
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || fps <= 0.0
        || fps > 240.0
    {
        reject("This build supports even video dimensions up to 8192 pixels and frame rates up to 240 fps.");
    }
    if (exact_rotation - f64::from(rot)).abs() > 0.0001
        || ![0, 90, -90, 180, -180, 270, -270].contains(&rot)
        || ratio(&video["sample_aspect_ratio"]).is_some_and(|r| (r - 1.0).abs() > 0.0001)
    {
        reject("Non-square pixels or non-right-angle rotation need a separate conversion path and are not supported yet.");
    }
    if ![
        "yuv420p", "yuvj420p", "yuv422p", "yuvj422p", "yuv444p", "yuvj444p", "nv12",
    ]
    .contains(&video["pix_fmt"].as_str().unwrap_or(""))
        || ["smpte2084", "arib-std-b67"].contains(&video["color_transfer"].as_str().unwrap_or(""))
        || video["color_primaries"] == "bt2020"
        || video["tags"].as_object().is_some_and(|tags| {
            tags.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("alpha_mode") && num(value).is_some_and(|mode| mode != 0.0)
            })
        })
        || video["side_data_list"].to_string().contains("DOVI")
    {
        reject("This output supports 8-bit SDR video. HDR, higher bit depth, and transparency need a preserving conversion path first.");
    }
    if !["progressive", "unknown"].contains(&video["field_order"].as_str().unwrap_or("unknown")) {
        reject("Interlaced video is not supported yet; it needs explicit deinterlacing or preservation.");
    }
    if audios.len() > 8 {
        reject("This build supports up to eight audio tracks.");
    }
    for a in &audios {
        let c = a["codec_name"].as_str().unwrap_or("");
        if ![
            "aac",
            "mp3",
            "opus",
            "vorbis",
            "flac",
            "alac",
            "pcm_u8",
            "pcm_s16le",
            "pcm_s24le",
            "pcm_s32le",
            "pcm_f32le",
            "pcm_f64le",
        ]
        .contains(&c)
        {
            reject(&format!(
                "An audio track uses unsupported {} audio. Recast will not silently drop it.",
                c.to_uppercase()
            ));
        }
        if c == "alac" && ![Some(16.0), Some(24.0)].contains(&num(&a["bits_per_raw_sample"])) {
            reject("This build supports 16-bit and 24-bit ALAC tracks; other ALAC depths cannot be decoded reliably yet.");
        }
        if c == "aac" && a["profile"] != "LC" {
            reject("HE-AAC and other AAC profiles need the deferred timing fix before they can be used in video conversion.");
        }
    }
    let aac_issue = audios.iter().find_map(|a| aac_track_issue(a));
    let mut info = VideoInfo {
        container,
        codec,
        width,
        height,
        rotation: rot,
        duration,
        frame_rate: fps,
        audio_tracks: audios.len(),
        subtitle_tracks: subtitles.len(),
        issue,
        aac_issue,
        mov_copy_issue: None,
        webm_copy_issue: None,
        opus_issue: audios.iter().find_map(|a| opus_track_issue(a)),
        probe,
    };
    info.mov_copy_issue = info.copy_issue("mov");
    info.webm_copy_issue = info.copy_issue("webm");
    Ok(Some(info))
}

fn aac_track_issue(a: &Value) -> Option<String> {
    let layout = audio_layout(a);
    if ![
        "mono",
        "stereo",
        "3.0",
        "4.0",
        "5.0",
        "5.1",
        "6.1(back)",
        "7.1",
    ]
    .contains(&layout)
    {
        return Some("An audio channel layout cannot be retained by AAC. Choose MKV with Keep original or Quality to retain the source audio.".to_string());
    }
    let rate = num(&a["sample_rate"]).unwrap_or(0.0) as u32;
    if ![
        7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000,
    ]
    .contains(&rate)
    {
        return Some("An audio sample rate cannot be retained by AAC. Choose MKV with Keep original or Quality to retain the source audio.".to_string());
    }
    None
}

fn audio_layout(audio: &Value) -> &str {
    audio["channel_layout"]
        .as_str()
        .unwrap_or(match audio["channels"].as_u64() {
            Some(1) => "mono",
            Some(2) => "stereo",
            _ => "unknown",
        })
}

fn mov_audio_copy(audio: &Value) -> bool {
    [
        "aac",
        "alac",
        "pcm_u8",
        "pcm_s16le",
        "pcm_s24le",
        "pcm_s32le",
        "pcm_f32le",
        "pcm_f64le",
    ]
    .contains(&audio["codec_name"].as_str().unwrap_or(""))
}

fn webm_audio_copy(audio: &Value) -> bool {
    ["opus", "vorbis"].contains(&audio["codec_name"].as_str().unwrap_or(""))
}

fn opus_track_issue(audio: &Value) -> Option<String> {
    if !["mono", "stereo", "3.0", "quad", "5.0", "5.1", "6.1", "7.1"].contains(&audio_layout(audio))
    {
        return Some("An audio channel layout cannot be retained by Opus. Choose MKV with Keep original or Quality to retain the source audio.".into());
    }
    let rate = num(&audio["sample_rate"]).unwrap_or(0.0);
    if !(7350.0..=192000.0).contains(&rate) {
        return Some(
            "An audio sample rate is outside the supported resampling range for WebM.".into(),
        );
    }
    None
}

impl VideoInfo {
    fn copy_issue(&self, target: &str) -> Option<String> {
        if target == "webm" {
            if !["vp8", "vp9", "av1"].contains(&self.codec.as_str()) {
                return Some("This video codec cannot be copied to WebM. Choose Quality or Target size to encode it to VP9.".into());
            }
            if streams(&self.probe).any(|s| s["codec_type"] == "audio" && !webm_audio_copy(s)) {
                return Some("An audio codec cannot be copied to WebM. Choose Quality or Target size to encode it to Opus.".into());
            }
            return None;
        }
        if target != "mov" {
            return None;
        }
        if !["h264", "hevc", "mpeg4", "mjpeg"].contains(&self.codec.as_str()) {
            return Some("This video codec cannot be copied to MOV. Choose Quality or Target size to encode it to H.264.".into());
        }
        if streams(&self.probe).any(|s| s["codec_type"] == "audio" && !mov_audio_copy(s)) {
            return Some("An audio codec cannot be copied to MOV. Choose Quality or Target size to encode it to AAC.".into());
        }
        None
    }

    pub fn target_issue(&self, target: &str) -> Option<String> {
        if let Some(issue) = &self.issue {
            return Some(issue.clone());
        }
        if target == "mp4" {
            if let Some(issue) = &self.aac_issue {
                return Some(issue.clone());
            }
            if streams(&self.probe)
                .any(|s| s["codec_type"] == "subtitle" && s["codec_name"] != "mov_text")
            {
                return Some("MP4 can retain existing MP4 text subtitles. Choose MKV for supported SRT, ASS, or WebVTT subtitles.".into());
            }
        } else if target == "mov" {
            if streams(&self.probe)
                .any(|s| s["codec_type"] == "subtitle" && s["codec_name"] != "mov_text")
            {
                return Some("MOV can retain existing MP4/MOV text subtitles. Choose MKV for supported SRT, ASS, or WebVTT subtitles.".into());
            }
            if let Some(issue) = streams(&self.probe)
                .filter(|s| s["codec_type"] == "audio" && !mov_audio_copy(s))
                .find_map(aac_track_issue)
            {
                return Some(issue);
            }
        } else if target == "webm" {
            if streams(&self.probe)
                .any(|s| s["codec_type"] == "subtitle" && s["codec_name"] != "webvtt")
            {
                return Some("WebM can retain WebVTT subtitles. Choose MKV for SRT/ASS, or MP4/MOV for existing MP4 text subtitles.".into());
            }
            if let Some(issue) = streams(&self.probe)
                .filter(|s| s["codec_type"] == "audio" && !webm_audio_copy(s))
                .find_map(opus_track_issue)
            {
                return Some(issue);
            }
        } else if target == "mkv"
            && streams(&self.probe).any(|s| {
                s["codec_type"] == "subtitle"
                    && !["subrip", "ass", "ssa", "webvtt"]
                        .contains(&s["codec_name"].as_str().unwrap_or(""))
            })
        {
            return Some("These subtitles cannot be retained in MKV yet. MP4 text subtitles can stay in MP4.".into());
        }
        None
    }
    pub fn require_options(&self, options: &VideoOptions) -> Result<(), String> {
        if let Some(issue) = self.target_issue(&options.target) {
            return Err(issue);
        }
        if options.copy_streams {
            if let Some(issue) = self.copy_issue(&options.target) {
                return Err(issue);
            }
        }
        if options.size_limit.is_some() {
            if let Some(issue) = if options.target == "webm" {
                &self.opus_issue
            } else {
                &self.aac_issue
            } {
                return Err(issue.clone());
            }
        }
        Ok(())
    }
}

impl AudioBackend {
    pub fn inspect_video(
        &self,
        path: &Path,
        cancel: &AtomicBool,
    ) -> Result<Option<VideoInfo>, ConversionError> {
        let work = tempfile::tempdir().map_err(|e| e.to_string())?;
        let output = work.path().join("probe.json");
        let mut cmd = self.command("ffprobe");
        cmd.args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-show_streams",
            "-show_data",
            "-show_format",
            "-show_chapters",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout(File::create(&output).map_err(|e| e.to_string())?);
        run(
            &mut cmd,
            &work.path().join("probe.log"),
            None,
            cancel,
            30,
            &mut |_| {},
        )?;
        let mut data = Vec::new();
        File::open(output)
            .map_err(|e| e.to_string())?
            .take(4_194_305)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() > 4_194_304 {
            return Err("This video's stream metadata is too large to inspect."
                .to_string()
                .into());
        }
        parse(
            serde_json::from_slice(&data)
                .map_err(|_| "Video inspection returned invalid data.".to_string())?,
        )
        .map_err(Into::into)
    }
    fn video_command(&self) -> Command {
        let mut c = self.command("ffmpeg");
        c.args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-nostats",
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
        ])
        .stdout(Stdio::null());
        c
    }
    fn video_fingerprint(
        &self,
        source: &Path,
        work: &Path,
        name: &str,
        audio_tracks: usize,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(f64),
    ) -> Result<std::path::PathBuf, ConversionError> {
        let output = work.join(format!("{name}.frames"));
        let status = work.join(format!("{name}.progress"));
        let mut cmd = self.video_command();
        cmd.args(["-copyts", "-start_at_zero", "-i"])
            .arg(source)
            .args([
                "-map",
                "0:v:0",
                "-map",
                "0:a?",
                "-c:v",
                "rawvideo",
                "-c:a",
                "pcm_f64le",
                "-threads:v",
                "1",
                "-enc_time_base:v",
                "demux",
                "-fps_mode",
                "passthrough",
                "-f",
                "framehash",
                "-progress",
            ])
            .arg(&status)
            .arg(&output);
        if audio_tracks > 0 {
            // Hash the continuous PCM stream as well: containers may repacketize
            // unchanged audio, so per-frame hashes alone are not comparable.
            cmd.args(["-map", "0:a", "-c:a", "pcm_f64le", "-f", "streamhash"])
                .arg(work.join(format!("{name}.audio-hashes")));
        }
        run(
            &mut cmd,
            &work.join(format!("{name}.log")),
            Some(&status),
            cancel,
            86400,
            progress,
        )?;
        Ok(output)
    }
    pub fn convert_video(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &VideoOptions,
        cancel: &AtomicBool,
        mut progress: impl FnMut(f64, &str),
    ) -> Result<OutputFile, ConversionError> {
        options.validate()?;
        check_cancel(cancel)?;
        let source = source.canonicalize().map_err(|e| e.to_string())?;
        let parent = folder.unwrap_or_else(|| source.parent().unwrap());
        let work = tempfile::Builder::new()
            .prefix("recast-video-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let staged = work.path().join("input");
        let mut input = File::open(&source).map_err(|e| e.to_string())?;
        let mut copy = File::create(&staged).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 262144];
        progress(0.0, "Preparing video");
        loop {
            check_cancel(cancel)?;
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            copy.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        drop(copy);
        let info = self
            .inspect_video(&staged, cancel)?
            .ok_or_else(|| "This file does not contain a video track.".to_string())?;
        info.require_options(options)?;
        progress(0.0, "Checking source");
        let original = self.video_fingerprint(
            &staged,
            work.path(),
            "source",
            info.audio_tracks,
            cancel,
            &mut |t| progress((t / info.duration).clamp(0.0, 1.0) * 0.2, "Checking source"),
        )?;
        let mut source_stats = frame_stats(&original, cancel)?;
        // MOV edit lists can trim the last displayed frame without changing its
        // decoded duration. Keep that visible endpoint instead of extending it.
        let source_video = source_stats.get_mut(&0).unwrap();
        let video_end = video_end(&info).unwrap_or(source_video.end);
        // Matroska rounds packet timestamps/durations to milliseconds. For
        // example, a 24 fps frame can decode as 41 ms while its declared end
        // rounds up by 1 ms. Retain that end without accepting missing frames.
        let source_precision = if info.container == "matroska,webm" {
            0.00101
        } else {
            0.00001
        };
        if video_end < source_video.end - source_video.duration - source_precision
            || video_end > source_video.end + source_precision
        {
            return Err(
                "This video's declared duration disagrees with its decoded frames."
                    .to_string()
                    .into(),
            );
        }
        source_video.end = video_end;
        let size = SizePlan::new(options, &info, &source_stats)?;
        let (width, height) = size
            .as_ref()
            .map(|p| (p.width, p.height))
            .unwrap_or_else(|| options.dimensions(&info));
        let chosen = if !options.copy_streams && options.acceleration == Acceleration::Automatic {
            progress(0.2, "Checking hardware");
            self.choose_video_encoder(options.target == "webm", work.path(), cancel)?
        } else if options.target == "webm" {
            Encoder::Vp9Software
        } else {
            Encoder::Software
        };
        let job = VideoJob {
            staged: &staged,
            parent,
            work: work.path(),
            options,
            info: &info,
            original: &original,
            source_stats: &source_stats,
            video_end,
            size: size.as_ref(),
        };
        let (mut temporary, used, fallback) = if options.copy_streams {
            (
                self.remux_video(&job, cancel, &mut progress)?,
                Encoder::Software,
                false,
            )
        } else {
            encoder::with_fallback(&chosen, |encoder| {
                if encoder.is_software() && !chosen.is_software() {
                    progress(0.2, "Retrying with software");
                }
                self.encode_video(&job, encoder, cancel, &mut progress)
            })?
        };
        check_cancel(cancel)?;
        temporary.as_file_mut().flush().map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        let bytes = temporary
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        if options.size_limit.is_some_and(|limit| bytes > limit) {
            return Err(size::too_small().into());
        }
        let path = publish(
            temporary,
            source.file_stem().unwrap_or_default(),
            parent,
            &options.target,
        )?;
        progress(1.0, "Converted");
        let converted_audio = streams(&info.probe)
            .filter(|s| s["codec_type"] == "audio" && !options.copies_audio(s))
            .count();
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
            warnings: {
                let mut warnings = Vec::new();
                if fallback {
                    warnings.push("Hardware encoding could not complete or pass verification. Converted with software instead.".into());
                }
                warnings
            },
            details: Some(
                format!(
                    "{width} × {height} · {} · {} · {} audio track(s) · {}",
                    if options.copy_streams {
                        info.codec.to_uppercase()
                    } else if options.target == "webm" {
                        "VP9".into()
                    } else {
                        "H.264".into()
                    },
                    options
                        .frame_rate
                        .map(|r| format!("{r} fps"))
                        .unwrap_or("Original timing".into()),
                    info.audio_tracks,
                    if options.copy_streams {
                        "Copied streams"
                    } else if used.is_software() && options.acceleration == Acceleration::Automatic
                    {
                        "Software (hardware unavailable for this clip)"
                    } else {
                        used.label()
                    }
                ) + &size
                    .as_ref()
                    .map(|p| {
                        format!(
                            " · {:.2} MB / {:.2} MB limit",
                            bytes as f64 / 1_000_000.0,
                            p.limit as f64 / 1_000_000.0
                        )
                    })
                    .unwrap_or_default()
                    + &if converted_audio > 0 {
                        if options.target == "webm" {
                            format!(" · {converted_audio} audio track(s) encoded to Opus at 48,000 Hz; original channels retained.")
                        } else {
                            format!(" · {converted_audio} audio track(s) encoded to AAC; original sample rates and channels retained.")
                        }
                    } else {
                        String::new()
                    },
            ),
        })
    }
    fn encode_video(
        &self,
        job: &VideoJob<'_>,
        encoder: &Encoder,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(f64, &str),
    ) -> Result<tempfile::NamedTempFile, ConversionError> {
        size::fit(job.size, cancel, |size, attempt| {
            self.encode_video_attempt(job, encoder, size, attempt, cancel, progress)
        })
    }
    fn encode_video_attempt(
        &self,
        job: &VideoJob<'_>,
        encoder: &Encoder,
        size: Option<&SizePlan>,
        attempt: u8,
        cancel: &AtomicBool,
        progress: &mut impl FnMut(f64, &str),
    ) -> Result<EncodeAttempt<tempfile::NamedTempFile>, ConversionError> {
        let VideoJob {
            staged,
            parent,
            work,
            options,
            info,
            video_end,
            ..
        } = job;
        let (width, height) = size
            .map(|p| (p.width, p.height))
            .unwrap_or_else(|| options.dimensions(info));
        let temporary = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{}", options.target))
            .tempfile_in(parent)
            .map_err(|_| "The output folder is not writable.".to_string())?;
        let status = work.join("encode.progress");
        let passes = if size.is_some() && encoder.is_software() {
            2
        } else {
            1
        };
        let passlog = work.join(format!("size-pass-{attempt}"));
        for pass in 1..=passes {
            check_cancel(cancel)?;
            let phase = if passes == 2 {
                format!(
                    "{} · Software · pass {pass} of 2{}",
                    if pass == 1 {
                        "Analyzing size"
                    } else {
                        "Encoding"
                    },
                    if attempt > 0 { " · fitting size" } else { "" }
                )
            } else {
                format!(
                    "{} · {}",
                    if attempt > 0 {
                        "Fitting size"
                    } else {
                        "Encoding"
                    },
                    encoder.label()
                )
            };
            let start = 0.2 + f64::from(pass - 1) / f64::from(passes) * 0.65;
            progress(start, &phase);
            // A previous pass/attempt must not supply stale progress while starting.
            File::create(&status).map_err(|e| e.to_string())?;
            let mut cmd = self.video_command();
            encoder.initialize(&mut cmd);
            cmd.args(["-copyts", "-start_at_zero", "-i"])
                .arg(staged)
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "0:a?",
                    "-map",
                    "0:s?",
                    "-map_metadata",
                    if options.metadata { "0" } else { "-1" },
                    "-map_chapters",
                    if options.metadata { "0" } else { "-1" },
                ]);
            let mut filters = vec![
                format!("scale={width}:{height}:flags=lanczos"),
                "setsar=1".to_string(),
            ];
            if let Some(rate) = options.frame_rate {
                filters.push(format!("fps={rate}"));
                // FPS conversion must not expose the trimmed tail of a MOV edit.
                filters.push(format!("trim=end={video_end:.9}"));
            }
            filters.extend(encoder.filters());
            cmd.args(["-vf", &filters.join(",")]);
            if let Some(size) = size {
                encoder.configure_bitrate(&mut cmd, size.video_rate);
            } else {
                encoder.configure(
                    &mut cmd,
                    options.quality,
                    width,
                    height,
                    options.frame_rate.map(f64::from).unwrap_or(info.frame_rate),
                );
            }
            if passes == 2 {
                cmd.args(["-pass", &pass.to_string(), "-passlogfile"])
                    .arg(&passlog);
            }
            cmd.args([
                // Avoid reordered DTS durations shortening VFR MP4 edit lists.
                "-bf",
                "0",
                "-threads:v",
                "2",
                "-enc_time_base:v",
                if options.frame_rate.is_some() {
                    "filter"
                } else {
                    "demux"
                },
                "-fps_mode",
                "passthrough",
                "-c:s",
                "copy",
            ]);
            // Hardware encoders may omit packet durations. Presentation timestamps
            // plus the inspected visible endpoint define them without guessing a
            // nominal rate for VFR. B-frames are disabled, so next PTS is ordered.
            let duration = if let Some(rate) = options.frame_rate {
                format!("round(1/({rate}*TB))")
            } else {
                format!("max(1\\,min(if(gt(NEXT_PTS\\,PTS)\\,NEXT_PTS-PTS\\,round({video_end:.9}/TB)-PTS)\\,round({video_end:.9}/TB)-PTS))")
            };
            cmd.args(["-bsf:v", &format!("setts=duration={duration}")]);
            for (i, a) in streams(&info.probe)
                .filter(|s| s["codec_type"] == "audio")
                .enumerate()
            {
                if options.copies_audio(a) {
                    cmd.args([format!("-c:a:{i}"), "copy".into()]);
                } else {
                    let rate = options.audio_rate(a);
                    let channels = num(&a["channels"]).unwrap() as u32;
                    let bitrate = size.map(|p| p.audio_rates[i]).unwrap_or_else(|| {
                        u64::from(
                            ((if options.target == "webm" {
                                64000
                            } else {
                                96000
                            }) * channels)
                                .min(rate * channels * 6)
                                .max(16000),
                        )
                    });
                    if options.target == "webm" {
                        cmd.args([
                            format!("-c:a:{i}"),
                            "libopus".into(),
                            format!("-ar:a:{i}"),
                            "48000".into(),
                            format!("-filter:a:{i}"),
                            "aresample=48000:filter_size=64:cutoff=0.97".into(),
                            format!("-mapping_family:a:{i}"),
                            if channels <= 2 { "0" } else { "1" }.into(),
                            format!("-b:a:{i}"),
                            bitrate.to_string(),
                        ]);
                    } else {
                        cmd.args([
                            format!("-c:a:{i}"),
                            "aac".into(),
                            format!("-profile:a:{i}"),
                            "aac_low".into(),
                            format!("-b:a:{i}"),
                            bitrate.to_string(),
                        ]);
                    }
                }
            }
            if !options.metadata {
                cmd.args(["-map_metadata:s", "-1"]);
            }
            // Language and default/forced flags identify retained tracks even with ordinary tags off.
            for kind in ["audio", "subtitle"] {
                let letter = if kind == "audio" { "a" } else { "s" };
                for (i, s) in streams(&info.probe)
                    .filter(|s| s["codec_type"] == kind)
                    .enumerate()
                {
                    if let Some(lang) = s["tags"]["language"].as_str() {
                        cmd.args([
                            format!("-metadata:s:{letter}:{i}"),
                            format!("language={lang}"),
                        ]);
                    }
                    let dispositions: Vec<_> = s["disposition"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter(|(_, v)| **v == 1)
                        .map(|(k, _)| k.as_str())
                        .collect();
                    cmd.args([
                        format!("-disposition:{letter}:{i}"),
                        if dispositions.is_empty() {
                            "0".into()
                        } else {
                            dispositions.join("+")
                        },
                    ]);
                }
            }
            self.configure_copied_audio_timing(job, &mut cmd)?;
            options.configure_muxer(&mut cmd, info)?;
            cmd.args(["-avoid_negative_ts", "disabled", "-progress"])
                .arg(&status);
            if passes == 2 && pass == 1 {
                cmd.args(["-an", "-sn", "-f", "null", "-"]);
            } else {
                cmd.arg(temporary.path());
            }
            run(
                &mut cmd,
                &work.join("encode.log"),
                Some(&status),
                cancel,
                86400,
                &mut |t| {
                    progress(
                        start + (t / info.duration).clamp(0.0, 1.0) * 0.65 / f64::from(passes),
                        &phase,
                    )
                },
            )?;
        }
        check_cancel(cancel)?;
        let bytes = temporary
            .as_file()
            .metadata()
            .map_err(|e| e.to_string())?
            .len();
        if size.is_some_and(|plan| bytes > plan.limit) {
            // Do not spend another full decode validating an output we'll discard.
            return Ok(EncodeAttempt::TooLarge(bytes));
        }
        let actual = self
            .inspect_video(temporary.path(), cancel)?
            .ok_or_else(|| "The output has no video track.".to_string())?;
        if actual.codec
            != if options.target == "webm" {
                "vp9"
            } else {
                "h264"
            }
            || actual.width != width
            || actual.height != height
            || actual.rotation != 0
            || actual.audio_tracks != info.audio_tracks
            || actual.subtitle_tracks != info.subtitle_tracks
        {
            return Err(
                "The output did not retain the expected dimensions or tracks."
                    .to_string()
                    .into(),
            );
        }
        let precision = options.precision();
        let tolerance = options
            .frame_rate
            .map(|rate| 1.0 / f64::from(rate) + precision)
            .unwrap_or(precision);
        if self::video_end(&actual).is_none_or(|end| (end - video_end).abs() > tolerance) {
            return Err("The output did not preserve the video's visible duration."
                .to_string()
                .into());
        }
        if ["mkv", "mov", "webm"].contains(&options.target.as_str()) {
            self.check_retained_tracks(job, &actual, temporary.path(), cancel)?;
        }
        for (before, after) in streams(&info.probe)
            .filter(|s| s["codec_type"] == "audio")
            .zip(streams(&actual.probe).filter(|s| s["codec_type"] == "audio"))
        {
            if f64::from(options.audio_rate(before)) != num(&after["sample_rate"]).unwrap_or(0.0)
                || before["channels"] != after["channels"]
                || audio_layout(before) != audio_layout(after)
            {
                return Err(
                    "An audio track's rate or channel layout changed unexpectedly."
                        .to_string()
                        .into(),
                );
            }
        }
        let checking = format!("Checking {}", options.target.to_uppercase());
        progress(0.85, &checking);
        let checked = self.video_fingerprint(
            temporary.path(),
            work,
            "output",
            info.audio_tracks,
            cancel,
            &mut |t| progress(0.85 + (t / info.duration).clamp(0.0, 1.0) * 0.14, &checking),
        )?;
        validate_timing(
            job,
            &checked,
            &frame_stats(&checked, cancel)?,
            precision,
            cancel,
        )?;
        self.check_copied_audio(job)?;
        Ok(EncodeAttempt::Validated(temporary))
    }
}

struct VideoJob<'a> {
    staged: &'a Path,
    parent: &'a Path,
    work: &'a Path,
    options: &'a VideoOptions,
    info: &'a VideoInfo,
    original: &'a Path,
    source_stats: &'a BTreeMap<usize, StreamStats>,
    video_end: f64,
    size: Option<&'a SizePlan>,
}

#[derive(Default)]
struct StreamStats {
    count: u64,
    start: f64,
    end: f64,
    duration: f64,
    samples: u64,
    discontinuity: f64,
}
fn frame_rows(
    path: &Path,
) -> Result<impl Iterator<Item = Result<String, std::io::Error>>, ConversionError> {
    Ok(BufReader::new(File::open(path).map_err(|e| e.to_string())?).lines())
}
fn frame_stats(
    path: &Path,
    cancel: &AtomicBool,
) -> Result<BTreeMap<usize, StreamStats>, ConversionError> {
    let mut times = BTreeMap::new();
    let mut stats = BTreeMap::new();
    for line in frame_rows(path)? {
        check_cancel(cancel)?;
        let line = line.map_err(|e| e.to_string())?;
        if let Some(header) = line.strip_prefix("#tb ") {
            let (i, v) = header
                .split_once(": ")
                .ok_or_else(|| "Invalid frame timing.".to_string())?;
            times.insert(
                i.parse::<usize>().map_err(|e| e.to_string())?,
                ratio(&Value::String(v.into()))
                    .ok_or_else(|| "Invalid frame time base.".to_string())?,
            );
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let row: Vec<_> = line.split(',').map(str::trim).collect();
        if row.len() != 6 {
            return Err("The video frame check is incomplete.".to_string().into());
        }
        let i = row[0].parse::<usize>().map_err(|e| e.to_string())?;
        let tb = times
            .get(&i)
            .ok_or_else(|| "Missing frame time base.".to_string())?;
        let pts = row[2].parse::<f64>().map_err(|e| e.to_string())? * tb;
        let duration = row[3].parse::<f64>().map_err(|e| e.to_string())? * tb;
        if !pts.is_finite() || !duration.is_finite() || duration <= 0.0 {
            return Err("Unsupported video frame timing.".to_string().into());
        }
        let entry = stats.entry(i).or_insert_with(|| StreamStats {
            start: pts,
            ..Default::default()
        });
        if entry.count > 0 && pts + 0.00001 < entry.end - entry.duration {
            return Err("This stream has non-monotonic timestamps."
                .to_string()
                .into());
        }
        entry.discontinuity = entry
            .discontinuity
            .max((pts - entry.start - entry.samples as f64 * tb).abs());
        entry.count += 1;
        entry.samples += row[3].parse::<u64>().map_err(|e| e.to_string())?;
        entry.end = pts + duration;
        entry.duration = duration;
    }
    if stats.get(&0).is_none_or(|s| s.count == 0) {
        return Err("This video has no decodable frames.".to_string().into());
    }
    Ok(stats)
}
fn validate_timing(
    job: &VideoJob<'_>,
    checked: &Path,
    after: &BTreeMap<usize, StreamStats>,
    precision: f64,
    cancel: &AtomicBool,
) -> Result<(), ConversionError> {
    let before = job.source_stats;
    let original = job.original;
    let fps = job.options.frame_rate;
    let original_fps = job.info.frame_rate;
    let fail = || {
        ConversionError::from(
            "The output did not preserve complete video/audio timing. No output was published."
                .to_string(),
        )
    };
    if before.len() != after.len() {
        return Err(fail());
    }
    for (i, a) in before {
        let b = after.get(i).ok_or_else(fail)?;
        let tolerance = if *i == 0 {
            1.0 / f64::from(fps.unwrap_or(original_fps.ceil() as u32)).max(1.0) + 0.00001
        } else {
            precision.max(0.00101)
        };
        let (minimum_samples, maximum_samples) = if *i == 0 {
            (a.samples, a.samples)
        } else {
            let audio = streams(&job.info.probe)
                .filter(|s| s["codec_type"] == "audio")
                .nth(i - 1)
                .ok_or_else(fail)?;
            let source_rate = num(&audio["sample_rate"]).ok_or_else(fail)? as u64;
            let scaled = a
                .samples
                .checked_mul(u64::from(job.options.audio_rate(audio)))
                .ok_or_else(fail)?;
            // A changed rate can place the endpoint between samples. SWR may
            // round either way; unchanged rates still require an exact count.
            (scaled / source_rate, scaled.div_ceil(source_rate))
        };
        if (a.start - b.start).abs() > tolerance
            || (a.end - b.end).abs() > tolerance
            || (*i == 0 && fps.is_none() && a.count != b.count)
            || (*i != 0 && !(minimum_samples..=maximum_samples).contains(&b.samples))
        {
            #[cfg(test)]
            eprintln!("Stream {i}: before start {} end {} count {}; after start {} end {} count {}; tolerance {tolerance}", a.start, a.end, a.count, b.start, b.end, b.count);
            return Err(fail());
        }
    }
    if fps.is_none() {
        // Compare each decoded frame's presentation timestamp, including VFR gaps.
        let get_pts = |path: &Path| -> Result<Vec<f64>, ConversionError> {
            let mut tb = 0.0;
            let mut pts = Vec::new();
            for line in frame_rows(path)? {
                check_cancel(cancel)?;
                let line = line.map_err(|e| e.to_string())?;
                if let Some(v) = line.strip_prefix("#tb 0: ") {
                    tb = ratio(&Value::String(v.into())).ok_or_else(fail)?;
                }
                if line.starts_with("0,") {
                    let v = line
                        .split(',')
                        .nth(2)
                        .ok_or_else(fail)?
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| fail())?;
                    pts.push(v * tb);
                }
                if pts.len() > 10_000_000 {
                    return Err("This video exceeds the frame-validation budget."
                        .to_string()
                        .into());
                }
            }
            Ok(pts)
        };
        let a = get_pts(original)?;
        let b = get_pts(checked)?;
        if a.len() != b.len() || a.iter().zip(b).any(|(a, b)| (a - b).abs() > precision) {
            return Err(fail());
        }
    }
    Ok(())
}
fn run(
    command: &mut Command,
    log: &Path,
    progress_path: Option<&Path>,
    cancel: &AtomicBool,
    timeout: u64,
    report: &mut impl FnMut(f64),
) -> Result<(), ConversionError> {
    check_cancel(cancel)?;
    command.stderr(File::create(log).map_err(|e| e.to_string())?);
    let mut child = command
        .spawn()
        .map_err(|_| "The bundled video converter could not start.".to_string())?;
    let started = Instant::now();
    let mut reported = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            check_cancel(cancel)?;
            return Err("The video converter took too long to respond."
                .to_string()
                .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => (),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string().into());
            }
        }
        if reported.elapsed() > Duration::from_millis(250) {
            if let Some(path) = progress_path {
                if let Ok(mut file) = File::open(path) {
                    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
                    let _ = file.seek(SeekFrom::Start(len.saturating_sub(4096)));
                    let mut text = String::new();
                    let _ = file.take(4096).read_to_string(&mut text);
                    if let Some(t) = text
                        .lines()
                        .filter_map(|l| l.strip_prefix("out_time_us=")?.parse::<f64>().ok())
                        .next_back()
                    {
                        report(t / 1_000_000.0);
                    }
                }
            }
            reported = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    check_cancel(cancel)?;
    if !status.success() {
        let mut text = String::new();
        let _ = File::open(log).map(|f| f.take(4096).read_to_string(&mut text));
        return Err(format!(
            "Video conversion failed: {}",
            text.lines()
                .next()
                .unwrap_or("The source could not be decoded or encoded.")
        )
        .into());
    }
    Ok(())
}
