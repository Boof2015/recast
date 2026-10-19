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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioInfo {
    pub container: String,
    pub sample_rate: u32,
    pub sample_format: String,
    pub channels: u16,
    pub channel_layout: Option<String>,
    pub duration: Option<f64>,
    #[serde(skip)]
    tags: std::collections::BTreeMap<String, String>,
}

impl AudioOptions {
    pub fn validate(&self) -> Result<(), String> {
        if !["wav", "flac"].contains(&self.target.as_str()) {
            return Err("Choose WAV or FLAC for this audio conversion.".into());
        }
        if self.sample_rate.is_some_and(|rate| {
            ![
                8000, 11025, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 176400, 192000,
            ]
            .contains(&rate)
        }) {
            return Err("Choose a supported audio sample rate.".into());
        }
        if self.sample_format != "source" && format_bits(&self.sample_format).is_none() {
            return Err("Choose a supported audio sample format.".into());
        }
        Ok(())
    }

    pub fn resolve(&self, input: &AudioInfo) -> Result<(u32, String), String> {
        self.validate()?;
        let rate = self.sample_rate.unwrap_or(input.sample_rate);
        let format = if self.sample_format == "source" {
            &input.sample_format
        } else {
            &self.sample_format
        };
        if self.target == "flac" && !["s16", "s24"].contains(&format.as_str()) {
            return Err("This FLAC encoder supports 16-bit or 24-bit PCM. Choose one explicitly, or use WAV to keep the source sample format.".into());
        }
        if self.target == "flac" && rate > 655_350 {
            return Err("FLAC cannot preserve this sample rate. Choose an explicit compatible rate or use WAV.".into());
        }
        Ok((rate, format.clone()))
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
                    "The bundled audio converter is missing. Rebuild Recast with its audio backend."
                        .to_string()
                })?;
            if !output.status.success()
                || !String::from_utf8_lossy(&output.stdout)
                    .contains(&format!("{program} version 9.0.2"))
            {
                return Err("The bundled audio converter is out of date.".into());
            }
        }
        Ok(())
    }

    pub fn inspect(&self, path: &Path, cancel: &AtomicBool) -> Result<AudioInfo, ConversionError> {
        let work =
            tempfile::tempdir().map_err(|_| "Audio inspection could not start.".to_string())?;
        let json_path = work.path().join("probe.json");
        let mut command = self.command("ffprobe");
        command.args(["-v", "error", "-protocol_whitelist", "file,pipe", "-show_entries",
            "format=format_name,duration:format_tags:stream_tags:stream=codec_type,codec_name,sample_rate,channels,channel_layout,bits_per_sample,bits_per_raw_sample:stream_disposition=attached_pic",
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
        parse_probe(&probe).map_err(Into::into)
    }

    pub fn convert(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &AudioOptions,
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
        let info = self.inspect(&staged, cancel)?;
        let (rate, sample_format) = options.resolve(&info)?;
        let bits = format_bits(&sample_format).unwrap();
        let integer = !sample_format.starts_with('f');
        let resampling = rate != info.sample_rate;
        let quantizing = integer
            && (resampling
                || info.sample_format.starts_with('f')
                || bits < format_bits(&info.sample_format).unwrap());
        let dither = quantizing && options.dither;
        let mut warnings = Vec::new();
        // Keep unchanged/widened integer samples on the integer path. A double
        // intermediate is only needed for resampling or float -> integer, where
        // we must detect overload BEFORE the encoder clamps away the evidence.
        let intermediate = work.path().join("processed.f64");
        let processed = resampling || (integer && info.sample_format.starts_with('f'));
        if processed {
            let mut command = self.transcoder();
            command
                .args(["-f", &info.container, "-i"])
                .arg(&staged)
                .args([
                    "-map",
                    "0:a:0",
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
        let output = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(&format!(".{}", options.target))
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
            "0:a:0",
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
        if options.target == "flac" {
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
        } else {
            command.args(["-c:a", codec(&sample_format), "-rf64", "auto"]);
        }
        command.args(["-f", &options.target]).arg(output.path());
        run(
            &mut command,
            &work.path().join("encode.log"),
            cancel,
            Duration::from_secs(21600),
        )?;
        let actual = self.inspect(output.path(), cancel)?;
        if actual.sample_rate != rate
            || actual.sample_format != sample_format
            || actual.channels != info.channels
            || (info.channel_layout.is_some() && actual.channel_layout != info.channel_layout)
        {
            return Err("The converted audio did not retain the requested sample format, rate, or channel layout.".to_string().into());
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
            &options.target,
        )?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
            warnings,
            details: Some(format!(
                "{rate} Hz · {bits}-bit {} · {} channels · Dither {}",
                if integer { "PCM" } else { "float" },
                info.channels,
                if dither { "applied" } else { "off" }
            )),
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
    if !["wav", "flac"].contains(&container) {
        return Err("This audio build currently accepts PCM WAV and FLAC files.".into());
    }
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
        match stream["codec_name"].as_str().unwrap_or("") {
            "pcm_u8" => "u8",
            "pcm_s16le" => "s16",
            "pcm_s24le" => "s24",
            "pcm_s32le" => "s32",
            "pcm_f32le" => "f32",
            "pcm_f64le" => "f64",
            "flac" => match number("bits_per_raw_sample") {
                Some(16) => "s16",
                Some(24) => "s24",
                Some(32) => "s32",
                _ => {
                    return Err(
                        "This build accepts FLAC with 16-bit, 24-bit, or 32-bit samples.".into(),
                    )
                }
            },
            _ => return Err(
                "This WAV uses an unsupported encoding. Choose integer PCM or floating-point WAV."
                    .into(),
            ),
        }
        .to_string();
    let channel_layout = stream["channel_layout"]
        .as_str()
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string);
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
        .map_err(|_| "The bundled audio converter could not start.".to_string())?;
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
