use crate::image_backend::{check_cancel, publish, ConversionError, OutputFile};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct AudioBackend {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioOptions {
    pub target: String,
    pub sample_rate: Option<u32>,
    pub sample_format: String,
    pub dither: bool,
    pub metadata: bool,
    pub allow_clipping: bool,
    #[serde(default)]
    pub mp3: Mp3Options,
    #[serde(default)]
    pub aac: AacOptions,
    #[serde(default)]
    pub opus: OpusOptions,
    #[serde(default)]
    pub vorbis: VorbisOptions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mp3Mode {
    Vbr,
    Cbr,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mp3Options {
    pub mode: Mp3Mode,
    pub quality: u8,
    pub bitrate: u16,
}

impl Default for Mp3Options {
    fn default() -> Self {
        Self {
            mode: Mp3Mode::Vbr,
            quality: 2,
            bitrate: 192,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AacOptions {
    pub bitrate: u16,
}

impl Default for AacOptions {
    fn default() -> Self {
        Self { bitrate: 192 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpusOptions {
    pub bitrate: u16,
}

impl Default for OpusOptions {
    fn default() -> Self {
        Self { bitrate: 128 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VorbisOptions {
    pub quality: i8,
}

impl Default for VorbisOptions {
    fn default() -> Self {
        Self { quality: 5 }
    }
}

pub const OPUS_BITRATES: &[u16] = &[
    16, 24, 32, 48, 64, 80, 96, 128, 160, 192, 224, 256, 320, 384, 448, 512,
];

pub const AAC_RATES: &[u32] = &[
    7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000,
];
pub const AAC_BITRATES: &[u16] = &[
    16, 24, 32, 48, 64, 80, 96, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768,
];

pub const MP3_RATES: &[u32] = &[8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

fn mp3_bitrates(rate: u32) -> &'static [u16] {
    if rate < 16000 {
        &[8, 16, 24, 32, 40, 48, 56, 64]
    } else if rate < 32000 {
        &[8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    } else {
        &[
            32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub container: String,
    pub codec: String,
    pub sample_rate: u32,
    pub sample_format: String,
    pub channels: u16,
    pub channel_layout: Option<String>,
    pub duration: Option<f64>,
    #[serde(skip)]
    tags: std::collections::BTreeMap<String, String>,
}

impl AudioInfo {
    fn decoded_float(&self) -> bool {
        self.sample_format.starts_with('f') || self.lossy()
    }

    fn lossy(&self) -> bool {
        matches!(self.codec.as_str(), "mp3" | "aac" | "opus" | "vorbis")
    }

    pub fn target_issue(&self, target: &str) -> Option<String> {
        if matches!(target, "opus" | "ogg")
            && !matches!(
                self.channel_layout.as_deref(),
                Some("mono" | "stereo" | "3.0" | "quad" | "5.0" | "5.1" | "6.1" | "7.1")
            )
        {
            return Some(format!("{} cannot retain this file’s {} channel layout. Use WAV or FLAC, or remove this file to convert the rest.", if target == "ogg" { "Vorbis" } else { "Opus" }, self.channel_layout.as_deref().unwrap_or("unknown")));
        }
        // Standard AAC channel configurations; do not silently remap a layout
        // into a different one or depend on less interoperable PCE layouts.
        if target == "m4a"
            && !matches!(
                self.channel_layout.as_deref(),
                Some("mono" | "stereo" | "3.0" | "4.0" | "5.0" | "5.1" | "6.1(back)" | "7.1")
            )
        {
            return Some(format!("AAC cannot retain this file’s {} channel layout. Use WAV or FLAC, or remove this file to convert the rest.", self.channel_layout.as_deref().unwrap_or("unknown")));
        }
        if target == "mp3" && !matches!(self.channel_layout.as_deref(), Some("mono" | "stereo")) {
            return Some("MP3 requires mono or stereo. Recast does not downmix channels; use WAV or FLAC, or remove this file to convert the rest.".into());
        }
        // These are the native ALAC layouts supported by the bundled encoder.
        // Reject others before FFmpeg can insert an automatic channel remap.
        if target == "alac"
            && self.channel_layout.as_deref().is_some_and(|layout| {
                ![
                    "mono",
                    "stereo",
                    "3.0",
                    "4.0",
                    "5.0",
                    "5.1",
                    "6.1(back)",
                    "7.1(wide)",
                ]
                .contains(&layout)
            })
        {
            return Some(format!(
                "ALAC cannot retain this file’s {} channel layout. Use WAV or FLAC, or remove this file to convert the rest.",
                self.channel_layout.as_deref().unwrap()
            ));
        }
        None
    }
}

impl AudioOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !["wav", "flac", "alac", "mp3", "m4a", "opus", "ogg"].contains(&self.target.as_str()) {
            return Err(
                "Choose WAV, FLAC, ALAC, MP3, AAC/M4A, Opus, or Ogg Vorbis for this audio conversion.".into(),
            );
        }
        if self.sample_rate.is_some_and(|rate| {
            ![
                7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200,
                96000, 176400, 192000,
            ]
            .contains(&rate)
        }) {
            return Err("Choose a supported audio sample rate.".into());
        }
        if self.sample_format != "source" && format_bits(&self.sample_format).is_none() {
            return Err("Choose a supported audio sample format.".into());
        }
        if self.target == "mp3" && matches!(self.mp3.mode, Mp3Mode::Vbr) && self.mp3.quality > 9 {
            return Err("Choose an MP3 VBR quality from 0 (highest) to 9 (smallest).".into());
        }
        Ok(())
    }

    pub fn resolve(&self, input: &AudioInfo) -> Result<(u32, String), String> {
        self.validate()?;
        if let Some(issue) = input.target_issue(&self.target) {
            return Err(issue);
        }
        // Opus has a dedicated, visible fixed 48 kHz output policy. Ignore the
        // PCM sample rate remembered while switching between destinations.
        if self.target == "opus" {
            if !OPUS_BITRATES.contains(&self.opus.bitrate)
                || u32::from(self.opus.bitrate) > 256 * u32::from(input.channels)
            {
                return Err(
                    "Choose a compatible Opus bitrate (at most 256 kbps per channel).".into(),
                );
            }
            return Ok((48000, "f32".into()));
        }
        let rate = self.sample_rate.unwrap_or(input.sample_rate);
        if self.target == "ogg" {
            if rate > 200000 {
                return Err("Vorbis cannot preserve this sample rate. Choose an explicit rate at or below 200000 Hz.".into());
            }
            if !(-1..=10).contains(&self.vorbis.quality) {
                return Err("Choose a Vorbis quality from -1 (smallest) to 10 (highest).".into());
            }
            return Ok((rate, "f32".into()));
        }
        if self.target == "mp3" {
            if !MP3_RATES.contains(&rate) {
                return Err("MP3 cannot preserve this sample rate. Choose an explicit supported rate from 8000 to 48000 Hz.".into());
            }
            if matches!(self.mp3.mode, Mp3Mode::Cbr)
                && !mp3_bitrates(rate).contains(&self.mp3.bitrate)
            {
                return Err(format!("{} kbps is not available for MP3 at {rate} Hz. Choose a compatible bitrate or use variable bitrate.", self.mp3.bitrate));
            }
            // MP3 receives floating-point samples; PCM format/dither settings
            // remembered from other destinations must never quantize this path.
            return Ok((rate, "f32".into()));
        }
        if self.target == "m4a" {
            if !AAC_RATES.contains(&rate) {
                return Err("AAC cannot preserve this sample rate. Choose an explicit supported rate from 7350 to 96000 Hz.".into());
            }
            // Native AAC silently clamps targets above 6144 bits/channel/frame.
            // Require a deliberate choice rather than accepting that fallback.
            if !AAC_BITRATES.contains(&self.aac.bitrate)
                || u32::from(self.aac.bitrate) * 1000 > 6 * rate * u32::from(input.channels)
            {
                return Err(format!("{} kbps is not available for AAC at {rate} Hz with {} channels. Choose a compatible bitrate.", self.aac.bitrate, input.channels));
            }
            return Ok((rate, "f32".into()));
        }
        let format = if self.sample_format == "source" {
            if input.lossy() {
                if self.target == "wav" {
                    "f32"
                } else {
                    "s24"
                }
            } else {
                &input.sample_format
            }
        } else {
            &self.sample_format
        };
        if ["flac", "alac"].contains(&self.target.as_str()) && !["s16", "s24"].contains(&format) {
            return Err(format!("This {} encoder supports 16-bit or 24-bit PCM. Choose one explicitly, or use WAV to keep the source sample format.", self.target.to_uppercase()));
        }
        if self.target == "flac" && rate > 655_350 {
            return Err("FLAC cannot preserve this sample rate. Choose an explicit compatible rate or use WAV.".into());
        }
        if self.target == "alac" && rate > 384_000 {
            return Err("ALAC output is limited to 384000 Hz. Choose an explicit compatible rate or use WAV.".into());
        }
        Ok((rate, format.to_owned()))
    }
}

fn format_bits(format: &str) -> Option<u8> {
    match format {
        "u8" => Some(8),
        "s16" => Some(16),
        "s24" => Some(24),
        "s32" | "f32" => Some(32),
        "f64" => Some(64),
        _ => None,
    }
}

fn codec(format: &str) -> &'static str {
    match format {
        "u8" => "pcm_u8",
        "s16" => "pcm_s16le",
        "s24" => "pcm_s24le",
        "s32" => "pcm_s32le",
        "f32" => "pcm_f32le",
        "f64" => "pcm_f64le",
        _ => unreachable!("validated sample format"),
    }
}

impl AudioBackend {
    pub fn command(&self, program: &str) -> Command {
        let mut command = Command::new(self.root.join(if cfg!(windows) {
            format!("{program}.exe")
        } else {
            program.into()
        }));
        command.stdin(Stdio::null()).env("LC_ALL", "C");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }

    pub fn verify(&self) -> Result<(), String> {
        for program in ["ffmpeg", "ffprobe"] {
            let output = self
                .command(program)
                .arg("-version")
                .output()
                .map_err(|_| {
                    "The bundled media converter is missing. Rebuild Recast with its audio backend."
                        .to_string()
                })?;
            if !output.status.success()
                || !String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("{program} version 9.0.2"))
            {
                return Err("The bundled media converter is out of date.".into());
            }
        }
        let codecs = self
            .command("ffmpeg")
            .args(["-hide_banner", "-codecs"])
            .output()
            .map_err(|_| "The bundled media codecs could not be checked.".to_string())?;
        let supported = ["alac", "mp3", "aac", "opus", "vorbis", "h264"]
            .iter()
            .all(|codec| {
                String::from_utf8_lossy(&codecs.stdout).lines().any(|line| {
                    let mut parts = line.split_whitespace();
                    parts.next().is_some_and(|flags| flags.starts_with("DE"))
                        && parts.next() == Some(codec)
                })
            });
        if !codecs.status.success() || !supported {
            return Err(
                "The bundled media converter needs rebuilding for audio and H.264 MP4 support."
                    .into(),
            );
        }
        Ok(())
    }

    pub fn inspect(&self, path: &Path, cancel: &AtomicBool) -> Result<AudioInfo, ConversionError> {
        let work =
            tempfile::tempdir().map_err(|_| "Audio inspection could not start.".to_string())?;
        let json_path = work.path().join("probe.json");
        let mut command = self.command("ffprobe");
        command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-show_entries",
            "format=format_name,duration:format_tags:stream_tags:stream=codec_type,codec_name,profile,sample_rate,channels,channel_layout,bits_per_sample,bits_per_raw_sample:stream_disposition=attached_pic",
            "-of", "json"])
            .arg(path)
            .stdout(File::create(&json_path).map_err(|_| "Audio inspection could not start.".to_string())?);
        run(
            &mut command,
            &work.path().join("probe.log"),
            cancel,
            Duration::from_secs(20),
        )?;
        let mut bytes = Vec::new();
        File::open(json_path)
            .map_err(|_| "The audio inspection result could not be read.".to_string())?
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| "The audio inspection result could not be read.".to_string())?;
        if bytes.len() > 1_048_576 {
            return Err("This file has too many audio streams to inspect."
                .to_string()
                .into());
        }
        let probe: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| "This audio file could not be inspected.".to_string())?;
        let info = parse_probe(&probe)?;
        if info.codec == "mp3" {
            crate::mp3::validate(path, cancel)?;
        }
        if info.container == "aac" {
            crate::aac::validate_adts(path, cancel)?;
        }
        if info.codec == "opus" {
            crate::opus::validate(path, cancel)?;
        } else if info.codec == "vorbis" {
            crate::vorbis::validate(path, cancel)?;
        }
        Ok(info)
    }

    pub fn convert(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &AudioOptions,
        cancel: &AtomicBool,
    ) -> Result<OutputFile, ConversionError> {
        self.convert_stream(source, folder, options, None, cancel)
    }

    pub(crate) fn convert_stream(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &AudioOptions,
        video_track: Option<u32>,
        cancel: &AtomicBool,
    ) -> Result<OutputFile, ConversionError> {
        check_cancel(cancel)?;
        options.validate()?;
        let source = source
            .canonicalize()
            .map_err(|_| "The source file is no longer available.".to_string())?;
        let parent = folder.unwrap_or_else(|| source.parent().unwrap());
        let work = tempfile::Builder::new()
            .prefix("recast-audio-")
            .tempdir()
            .map_err(|_| "An audio work folder could not be created.".to_string())?;
        let staged = work.path().join("input");
        let mut input =
            File::open(&source).map_err(|_| "The source file could not be opened.".to_string())?;
        if !input
            .metadata()
            .map_err(|_| "The source could not be read.".to_string())?
            .is_file()
        {
            return Err("Choose an individual audio file.".to_string().into());
        }
        let mut copy =
            File::create(&staged).map_err(|_| "The audio could not be prepared.".to_string())?;
        let mut buffer = [0; 65536];
        loop {
            check_cancel(cancel)?;
            let size = input
                .read(&mut buffer)
                .map_err(|_| "The source audio could not be read.".to_string())?;
            if size == 0 {
                break;
            }
            copy.write_all(&buffer[..size])
                .map_err(|_| "The audio could not be prepared. Check disk space.".to_string())?;
        }
        drop(copy);
        let info = if let Some(index) = video_track {
            let probe = self.probe_media(&staged, cancel)?;
            crate::audio_extract::selected_track(&probe, Some(index))?
                .supported()?
                .clone()
        } else {
            self.inspect(&staged, cancel)?
        };
        let source_map = video_track.map_or_else(|| "0:a:0".into(), |index| format!("0:{index}"));
        let (rate, sample_format) = options.resolve(&info)?;
        let bits = format_bits(&sample_format).unwrap();
        let mp3_output = options.target == "mp3";
        let aac_output = options.target == "m4a";
        let opus_output = options.target == "opus";
        let vorbis_output = options.target == "ogg";
        let lossy_output = mp3_output || aac_output || opus_output || vorbis_output;
        let integer = !sample_format.starts_with('f');
        let resampling = rate != info.sample_rate;
        let quantizing = integer
            && (resampling
                || info.decoded_float()
                || bits < format_bits(&info.sample_format).unwrap());
        let dither = quantizing && options.dither;
        let mut warnings = Vec::new();
        // Keep unchanged/widened integer samples on the integer path. A double
        // intermediate checks resampling and float -> integer before an encoder
        // can clamp away overload. Lossy encoding also validates finite input and
        // compare the final gapless sample count without integer quantization.
        let intermediate = work.path().join("processed.f64");
        let processed = lossy_output
            || matches!(info.codec.as_str(), "aac" | "opus" | "vorbis")
            || resampling
            || (integer && info.decoded_float());
        if processed {
            let decoded_ogg = work.path().join("decoded.f64");
            if info.container == "ogg" {
                let frames = if info.codec == "opus" {
                    crate::opus::validate(&staged, cancel)?
                } else {
                    crate::vorbis::validate(&staged, cancel)?
                };
                let mut command = self.transcoder();
                command
                    .args(["-f", "ogg", "-i"])
                    .arg(&staged)
                    .args(["-map", "0:a:0", "-c:a", "pcm_f64le", "-f", "f64le"])
                    .arg(&decoded_ogg);
                run(
                    &mut command,
                    &work.path().join("decode.log"),
                    cancel,
                    Duration::from_secs(21600),
                )?;
                let bytes = std::fs::metadata(&decoded_ogg)
                    .map_err(|_| "The decoded audio could not be checked.".to_string())?
                    .len();
                if Some(bytes) != frames.checked_mul(u64::from(info.channels) * 8) {
                    return Err("The Ogg decoder did not retain the complete audio length. Cropped or unusual stream timing is not supported yet.".to_string().into());
                }
            }
            if info.container == "ogg" && !resampling {
                std::fs::rename(&decoded_ogg, &intermediate)
                    .map_err(|_| "The decoded audio could not be prepared.".to_string())?;
            } else {
                let mut command = self.transcoder();
                if info.container == "ogg" {
                    command.args([
                        "-f",
                        "f64le",
                        "-ar",
                        &info.sample_rate.to_string(),
                        "-ac",
                        &info.channels.to_string(),
                    ]);
                    if let Some(layout) = &info.channel_layout {
                        command.args(["-channel_layout", layout]);
                    }
                    command.arg("-i").arg(&decoded_ogg);
                } else {
                    command.args(["-f", &info.container, "-i"]).arg(&staged);
                }
                command
                    .args([
                        "-map",
                        if info.container == "ogg" {
                            "0:a:0"
                        } else {
                            &source_map
                        },
                        "-af",
                        &format!("aresample={rate}:osf=dbl:tsf=dblp:filter_size=64:cutoff=0.97"),
                        "-c:a",
                        "pcm_f64le",
                        "-f",
                        "f64le",
                    ])
                    .arg(&intermediate);
                run(
                    &mut command,
                    &work.path().join("process.log"),
                    cancel,
                    Duration::from_secs(21600),
                )?;
            }
            let overloaded = scan_samples(&intermediate, cancel)?;
            if integer && overloaded {
                if !options.allow_clipping {
                    return Err(ConversionError::Clipping);
                }
                warnings.push(
                    "Clipping occurred: peaks outside the integer output range were clipped."
                        .into(),
                );
            }
        }
        let extension = if options.target == "alac" {
            "m4a"
        } else {
            &options.target
        };
        let output = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{extension}"))
            .tempfile_in(parent)
            .map_err(|_| {
                "Cannot write to this output folder. Choose another folder.".to_string()
            })?;
        let mut command = self.transcoder();
        if processed {
            command.args([
                "-f",
                "f64le",
                "-ar",
                &rate.to_string(),
                "-ac",
                &info.channels.to_string(),
            ]);
            if let Some(layout) = &info.channel_layout {
                command.args(["-channel_layout", layout]);
            }
            command.arg("-i").arg(&intermediate);
        } else {
            command.args(["-f", &info.container, "-i"]).arg(&staged);
        }
        command.args([
            "-map",
            if processed { "0:a:0" } else { &source_map },
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
        ]);
        // Copy ordinary text tags deliberately. Do not propagate stale loudness
        // analysis, unsupported production data, or artwork as opaque text.
        if options.metadata {
            for (name, value) in &info.tags {
                command.args(["-metadata", &format!("{name}={value}")]);
            }
        }
        if integer {
            let storage = if bits == 8 {
                "u8"
            } else if bits == 16 {
                "s16"
            } else {
                "s32"
            };
            command.args([
                "-af",
                &format!(
                    "aresample=osf={storage}:output_sample_bits={bits}:dither_method={}",
                    if dither { "triangular" } else { "none" }
                ),
            ]);
        }
        if mp3_output {
            command.args([
                "-c:a",
                "libmp3lame",
                "-sample_fmt",
                "fltp",
                "-compression_level",
                "2",
                "-write_xing",
                "1",
                "-id3v2_version",
                "3",
            ]);
            match options.mp3.mode {
                Mp3Mode::Vbr => {
                    command.args(["-q:a", &options.mp3.quality.to_string()]);
                }
                Mp3Mode::Cbr => {
                    command.args(["-b:a", &format!("{}k", options.mp3.bitrate)]);
                }
            }
        } else if aac_output {
            command.args([
                "-c:a",
                "aac",
                "-profile:a",
                "aac_low",
                "-aac_coder",
                "twoloop",
                "-b:a",
                &format!("{}k", options.aac.bitrate),
                "-sample_fmt",
                "fltp",
                "-movflags",
                "+faststart",
                "-use_editlist",
                "1",
                // Movie edit durations must have sample precision, including
                // clips shorter than a millisecond and partial final frames.
                "-movie_timescale",
                &rate.to_string(),
            ]);
        } else if opus_output {
            command.args([
                "-c:a",
                "libopus",
                "-sample_fmt",
                "flt",
                "-application",
                "audio",
                "-vbr",
                "on",
                "-compression_level",
                "10",
                "-frame_duration",
                "20",
                "-b:a",
                &format!("{}k", options.opus.bitrate),
                "-mapping_family",
                if info.channels > 2 { "1" } else { "0" },
            ]);
        } else if vorbis_output {
            command.args([
                "-c:a",
                "libvorbis",
                "-sample_fmt",
                "fltp",
                "-q:a",
                &options.vorbis.quality.to_string(),
            ]);
        } else if options.target == "flac" {
            command.args([
                "-c:a",
                "flac",
                "-compression_level",
                "5",
                "-sample_fmt",
                if bits == 16 { "s16" } else { "s32" },
                "-bits_per_raw_sample",
                &bits.to_string(),
            ]);
        } else if options.target == "alac" {
            command.args([
                "-c:a",
                "alac",
                "-compression_level",
                "2",
                "-sample_fmt",
                if bits == 16 { "s16p" } else { "s32p" },
                "-bits_per_raw_sample",
                &bits.to_string(),
                "-movflags",
                "+faststart",
            ]);
        } else {
            command.args(["-c:a", codec(&sample_format), "-rf64", "auto"]);
        }
        command
            .args([
                "-f",
                if options.target == "alac" || aac_output {
                    "ipod"
                } else {
                    &options.target
                },
            ])
            .arg(output.path());
        run(
            &mut command,
            &work.path().join("encode.log"),
            cancel,
            Duration::from_secs(21600),
        )?;
        let actual = self.inspect(output.path(), cancel)?;
        if (options.target == "alac" && actual.codec != "alac")
            || (mp3_output && actual.codec != "mp3")
            || (aac_output && actual.codec != "aac")
            || (opus_output && actual.codec != "opus")
            || (vorbis_output && actual.codec != "vorbis")
            || actual.sample_rate != rate
            || (!lossy_output && actual.sample_format != sample_format)
            || actual.channels != info.channels
            || (info.channel_layout.is_some() && actual.channel_layout != info.channel_layout)
        {
            return Err("The converted audio did not retain the requested sample format, rate, or channel layout.".to_string().into());
        }
        if lossy_output {
            // Check the complete decoded output and gapless sample count before
            // publishing. MP3 headers include codec delay/padding in duration.
            let decoded = work.path().join("output.f64");
            let mut command = self.transcoder();
            command
                .args(["-f", &actual.container, "-i"])
                .arg(output.path())
                .args(["-map", "0:a:0", "-c:a", "pcm_f64le", "-f", "f64le"])
                .arg(&decoded);
            run(
                &mut command,
                &work.path().join("validate.log"),
                cancel,
                Duration::from_secs(21600),
            )?;
            let expected = std::fs::metadata(&intermediate)
                .map_err(|_| "The processed audio could not be checked.".to_string())?
                .len();
            let actual = std::fs::metadata(&decoded)
                .map_err(|_| "The encoded audio could not be checked.".to_string())?
                .len();
            if actual != expected {
                return Err(
                    "The encoded audio did not retain the complete audio length."
                        .to_string()
                        .into(),
                );
            }
            if scan_samples(&decoded, cancel)? {
                warnings.push(format!("Decoded {} peaks exceed full scale and may clip during playback. No normalization was applied.", if aac_output { "AAC" } else if opus_output { "Opus" } else if vorbis_output { "Vorbis" } else { "MP3" }));
            }
        }
        if info.lossy() {
            warnings.push(if lossy_output {
                format!(
                    "Re-encoding {} adds another lossy generation.",
                    info.codec.to_uppercase()
                )
            } else {
                format!(
                    "Converting {} to PCM or a lossless format does not restore lost detail.",
                    info.codec.to_uppercase()
                )
            });
        }
        if info.container == "aac" {
            warnings.push(
                "Raw AAC has no reliable gapless trim metadata. All decoded samples were retained."
                    .into(),
            );
        }
        let bytes = output
            .as_file()
            .metadata()
            .map_err(|_| "The converted audio could not be read.".to_string())?
            .len();
        output
            .as_file()
            .sync_all()
            .map_err(|_| "The converted audio could not be saved. Check disk space.".to_string())?;
        check_cancel(cancel)?;
        let path = publish(
            output,
            source.file_stem().unwrap_or_default(),
            parent,
            extension,
        )?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
            warnings,
            details: Some(if mp3_output {
                format!(
                    "{rate} Hz · {} channels · MP3 {}",
                    info.channels,
                    match options.mp3.mode {
                        Mp3Mode::Vbr => format!("VBR V{}", options.mp3.quality),
                        Mp3Mode::Cbr => format!("{} kbps CBR", options.mp3.bitrate),
                    }
                )
            } else if vorbis_output {
                format!(
                    "{rate} Hz · {} channels · Vorbis quality {} · VBR",
                    info.channels, options.vorbis.quality
                )
            } else if opus_output {
                format!(
                    "{rate} Hz · {} channels · Opus · {} kbps VBR target",
                    info.channels, options.opus.bitrate
                )
            } else if aac_output {
                format!(
                    "{rate} Hz · {} channels · AAC-LC · {} kbps target",
                    info.channels, options.aac.bitrate
                )
            } else {
                format!(
                    "{rate} Hz · {bits}-bit {} · {} channels · Dither {}",
                    if integer { "PCM" } else { "float" },
                    info.channels,
                    if dither { "applied" } else { "off" }
                )
            }),
        })
    }

    fn transcoder(&self) -> Command {
        let mut command = self.command("ffmpeg");
        command
            .args([
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
                "1",
                "-protocol_whitelist",
                "file,pipe",
            ])
            .stdout(Stdio::null());
        command
    }
}

fn parse_probe(probe: &serde_json::Value) -> Result<AudioInfo, String> {
    let container = probe["format"]["format_name"].as_str().unwrap_or("");
    let container = match container {
        "wav" | "flac" | "mp3" | "aac" | "ogg" => container,
        "mov,mp4,m4a,3gp,3g2,mj2" => "mov",
        _ => return Err(
            "This audio build accepts PCM WAV, FLAC, MP3, raw AAC, Ogg Opus/Vorbis, and ALAC/AAC in M4A/MP4 files."
                .into(),
        ),
    };
    let streams = probe["streams"]
        .as_array()
        .ok_or("No audio stream was found.")?;
    let audio: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    if audio.len() != 1
        || streams
            .iter()
            .any(|s| s["codec_type"] != "audio" && s["disposition"]["attached_pic"] != 1)
    {
        return Err("Choose a file containing one audio stream. Combining or selecting streams is not supported yet.".into());
    }
    let stream = audio[0];
    let codec = stream["codec_name"].as_str().unwrap_or("");
    if container == "mov" && !["alac", "aac"].contains(&codec) {
        return Err("This M4A/MP4 file does not contain supported ALAC or AAC audio.".into());
    }
    if container == "ogg" && !["opus", "vorbis"].contains(&codec) {
        return Err("This build accepts Opus or Vorbis audio in Ogg files.".into());
    }
    if container == "mp3" && codec != "mp3" {
        return Err("This build accepts MPEG Layer III (MP3), not Layer I or II audio.".into());
    }
    parse_audio_stream(probe, stream, container)
}

pub(crate) fn parse_audio_stream(
    probe: &serde_json::Value,
    stream: &serde_json::Value,
    container: &str,
) -> Result<AudioInfo, String> {
    let codec = stream["codec_name"].as_str().unwrap_or("");
    if codec == "aac" && stream["profile"].as_str() != Some("LC") {
        return Err("This build supports AAC-LC. HE-AAC/HE-AACv2 are not enabled because the bundled decoder does not yet preserve their timing reliably; other AAC profiles are also unsupported.".into());
    }
    let number = |key: &str| {
        stream[key]
            .as_u64()
            .or_else(|| stream[key].as_str()?.parse().ok())
    };
    let sample_rate = number("sample_rate")
        .filter(|n| *n > 0 && *n <= 768_000)
        .ok_or("This sample rate is not supported yet.")? as u32;
    let channels = number("channels")
        .filter(|n| *n > 0 && *n <= 8)
        .ok_or("This build accepts audio with one to eight channels.")? as u16;
    let sample_format =
        match codec {
            "mp3" | "opus" | "vorbis" | "aac" => "lossy",
            "pcm_u8" => "u8",
            "pcm_s16le" => "s16",
            "pcm_s24le" => "s24",
            "pcm_s32le" => "s32",
            "pcm_f32le" => "f32",
            "pcm_f64le" => "f64",
            "alac" => match number("bits_per_raw_sample") {
                Some(16) => "s16",
                Some(24) => "s24",
                _ => {
                    return Err(
                        "This build accepts 16-bit and 24-bit ALAC. Other ALAC sample depths are not supported yet."
                            .into(),
                    )
                }
            },
            "flac" => match number("bits_per_raw_sample") {
                Some(16) => "s16",
                Some(24) => "s24",
                Some(32) => "s32",
                _ => {
                    return Err(format!(
                        "This build accepts {} with 16-bit, 24-bit, or 32-bit samples.",
                        codec.to_uppercase()
                    ))
                }
            },
            _ => return Err(
                "This audio encoding is not supported yet."
                    .into(),
            ),
        }
        .to_string();
    let channel_layout = stream["channel_layout"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string)
        // Ordinary mono/stereo WAV has no channel mask, unlike ALAC. These
        // standard layouts are equivalent; do not treat a WAV round trip as a
        // lost mapping merely because ffprobe omits the redundant label.
        .or_else(|| match channels {
            1 => Some("mono".into()),
            2 => Some("stereo".into()),
            _ => None,
        });
    if channels > 2 && channel_layout.is_none() {
        return Err("This multichannel file has no recognized channel layout. Recast cannot safely preserve its channel mapping yet.".into());
    }
    let duration = probe["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0);
    let mut tags = std::collections::BTreeMap::new();
    for object in [&probe["format"]["tags"], &stream["tags"]] {
        if let Some(object) = object.as_object() {
            for (key, value) in object {
                let key = key.to_lowercase();
                if [
                    "title",
                    "artist",
                    "album",
                    "album_artist",
                    "composer",
                    "performer",
                    "genre",
                    "date",
                    "track",
                    "disc",
                    "comment",
                    "copyright",
                    "description",
                    "language",
                ]
                .contains(&key.as_str())
                {
                    if let Some(text) = value.as_str() {
                        tags.insert(key, text.to_owned());
                    }
                }
            }
        }
    }
    Ok(AudioInfo {
        tags,
        container: container.into(),
        codec: codec.into(),
        sample_rate,
        sample_format,
        channels,
        channel_layout,
        duration,
    })
}

// Read the actual double samples, not rounded peak values printed in a log.
fn scan_samples(path: &Path, cancel: &AtomicBool) -> Result<bool, ConversionError> {
    let mut file =
        File::open(path).map_err(|_| "The processed audio could not be checked.".to_string())?;
    let length = file
        .metadata()
        .map_err(|_| "The processed audio could not be checked.".to_string())?
        .len();
    if length == 0 || length % 8 != 0 {
        return Err("No complete audio samples were produced."
            .to_string()
            .into());
    }
    let mut remaining = length;
    let mut bytes = [0u8; 65536];
    let mut overloaded = false;
    while remaining > 0 {
        check_cancel(cancel)?;
        let size = remaining.min(bytes.len() as u64) as usize;
        file.read_exact(&mut bytes[..size])
            .map_err(|_| "The processed audio could not be checked.".to_string())?;
        for sample in bytes[..size].as_chunks::<8>().0 {
            let value = f64::from_le_bytes(*sample);
            if !value.is_finite() {
                return Err("The audio contains invalid (NaN or infinite) samples."
                    .to_string()
                    .into());
            }
            overloaded |= !(-1.0..=1.0).contains(&value);
        }
        remaining -= size as u64;
    }
    Ok(overloaded)
}

fn run(
    command: &mut Command,
    diagnostics: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), ConversionError> {
    check_cancel(cancel)?;
    command.stderr(
        File::create(diagnostics)
            .map_err(|_| "The audio converter could not start.".to_string())?,
    );
    let mut child = command
        .spawn()
        .map_err(|_| "The bundled media converter could not start.".to_string())?;
    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Relaxed) || started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            check_cancel(cancel)?;
            return Err("The audio converter took too long to respond."
                .to_string()
                .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The audio converter stopped unexpectedly."
                    .to_string()
                    .into());
            }
        }
    };
    check_cancel(cancel)?;
    if !status.success() {
        let mut text = String::new();
        if let Ok(file) = File::open(diagnostics) {
            let _ = file.take(4096).read_to_string(&mut text);
        }
        return Err(format!(
            "Audio conversion failed: {}",
            text.lines()
                .next()
                .unwrap_or("The audio could not be decoded or encoded.")
        )
        .into());
    }
    Ok(())
}
