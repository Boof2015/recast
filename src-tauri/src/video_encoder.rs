//! Encoder selection is proven by a short encode and software decode, not a GPU name.
//! Drivers stay optional: software works on machines without any hardware runtime.
use super::{frame_stats, run, AudioBackend, ConversionError};
use crate::image_backend::check_cancel;
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::AtomicBool,
    time::Instant,
};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Acceleration {
    #[default]
    Automatic,
    Software,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Encoder {
    Software,
    VideoToolbox,
    Nvenc,
    Qsv,
    Amf,
    Vaapi(PathBuf),
}
impl Encoder {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::Software => "Software",
            Self::VideoToolbox => "Apple VideoToolbox",
            Self::Nvenc => "NVIDIA NVENC",
            Self::Qsv => "Intel Quick Sync",
            Self::Amf => "AMD AMF",
            Self::Vaapi(_) => "VA-API",
        }
    }
    pub(super) fn initialize(&self, command: &mut Command) {
        match self {
            Self::Qsv => {
                command.args([
                    "-init_hw_device",
                    "qsv=recast:hw_any,child_device_type=d3d11va",
                ]);
            }
            Self::Vaapi(path) => {
                command.arg("-vaapi_device").arg(path);
            }
            _ => (),
        }
    }
    pub(super) fn filters(&self) -> Vec<String> {
        if matches!(self, Self::Vaapi(_)) {
            vec!["format=nv12".into(), "hwupload".into()]
        } else {
            vec![]
        }
    }
    pub(super) fn configure(
        &self,
        command: &mut Command,
        quality: u8,
        width: u32,
        height: u32,
        fps: f64,
    ) {
        // Each API has its own quality model. These are useful ranges, not claims
        // that the same slider position produces equivalent pictures/file sizes.
        let quantizer = 40.0 - f64::from(quality) * 24.0 / 100.0;
        let q = format!("{quantizer:.0}");
        match self {
            Self::Software => {
                command.args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "medium",
                    "-crf",
                    &format!("{quantizer:.2}"),
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            Self::VideoToolbox => {
                command.args([
                    "-c:v",
                    "h264_videotoolbox",
                    "-allow_sw",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                ]);
                if cfg!(target_arch = "aarch64") {
                    command.args(["-q:v", &quality.to_string()]);
                } else {
                    // Intel VideoToolbox has no constant-quality mode. Derive an
                    // average bitrate from geometry/fps; this is not a size cap.
                    let bits_per_pixel = 0.025 + 0.225 * (f64::from(quality) / 100.0).powi(2);
                    let bitrate = (f64::from(width) * f64::from(height) * fps * bits_per_pixel)
                        .clamp(64_000.0, 240_000_000.0) as u64;
                    command.args(["-b:v", &bitrate.to_string()]);
                }
            }
            Self::Nvenc => {
                command.args([
                    "-c:v",
                    "h264_nvenc",
                    "-preset",
                    "p4",
                    "-rc",
                    "vbr",
                    "-cq",
                    &q,
                    "-b:v",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            Self::Qsv => {
                command.args([
                    "-c:v",
                    "h264_qsv",
                    "-preset",
                    "medium",
                    "-global_quality",
                    &q,
                    "-look_ahead",
                    "0",
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            Self::Amf => {
                command.args([
                    "-c:v",
                    "h264_amf",
                    "-usage",
                    "transcoding",
                    "-quality",
                    "balanced",
                    "-rc",
                    "cqp",
                    "-qp_i",
                    &q,
                    "-qp_p",
                    &q,
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            Self::Vaapi(_) => {
                command.args(["-c:v", "h264_vaapi", "-rc_mode", "CQP", "-qp", &q]);
            }
        }
    }
    pub(super) fn configure_bitrate(&self, command: &mut Command, bitrate: u64) {
        match self {
            Self::Software => {
                command.args([
                    "-c:v", "libx264", "-preset", "medium", "-pix_fmt", "yuv420p",
                ]);
            }
            Self::VideoToolbox => {
                command.args([
                    "-c:v",
                    "h264_videotoolbox",
                    "-allow_sw",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            Self::Nvenc => {
                command.args([
                    "-c:v",
                    "h264_nvenc",
                    "-preset",
                    "p4",
                    "-rc",
                    "vbr",
                    "-pix_fmt",
                    "yuv420p",
                ]);
            }
            Self::Qsv => {
                command.args([
                    "-c:v",
                    "h264_qsv",
                    "-preset",
                    "medium",
                    "-look_ahead",
                    "0",
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            Self::Amf => {
                command.args([
                    "-c:v",
                    "h264_amf",
                    "-usage",
                    "transcoding",
                    "-quality",
                    "balanced",
                    "-rc",
                    "vbr_peak",
                    "-pix_fmt",
                    "nv12",
                ]);
            }
            Self::Vaapi(_) => {
                command.args(["-c:v", "h264_vaapi", "-rc_mode", "VBR"]);
            }
        }
        command.args(["-b:v", &bitrate.to_string()]);
        if !matches!(self, Self::VideoToolbox | Self::Software) {
            command.args([
                "-maxrate",
                &(bitrate * 2).to_string(),
                "-bufsize",
                &(bitrate * 2).to_string(),
            ]);
        }
    }
}

fn candidates(system: &str, render_nodes: Vec<PathBuf>) -> Vec<Encoder> {
    match system {
        "macos" => vec![Encoder::VideoToolbox],
        "windows" => vec![Encoder::Nvenc, Encoder::Qsv, Encoder::Amf],
        "linux" => std::iter::once(Encoder::Nvenc)
            .chain(render_nodes.into_iter().take(8).map(Encoder::Vaapi))
            .collect(),
        _ => vec![],
    }
}

/// A failed hardware attempt includes failed output validation. Never publish
/// that output, and never retry cancellation (or a failed software attempt).
pub(super) fn with_fallback<T>(
    chosen: &Encoder,
    mut attempt: impl FnMut(&Encoder) -> Result<T, ConversionError>,
) -> Result<(T, Encoder, bool), ConversionError> {
    match attempt(chosen) {
        Ok(value) => Ok((value, chosen.clone(), false)),
        Err(ConversionError::Failed(reason)) if *chosen != Encoder::Software => {
            #[cfg(test)]
            eprintln!("{} fallback: {reason}", chosen.label());
            let _ = reason;
            attempt(&Encoder::Software).map(|value| (value, Encoder::Software, true))
        }
        Err(error) => Err(error),
    }
}

impl AudioBackend {
    pub(super) fn choose_video_encoder(
        &self,
        work: &Path,
        cancel: &AtomicBool,
    ) -> Result<Encoder, ConversionError> {
        let mut nodes: Vec<_> = if cfg!(target_os = "linux") {
            fs::read_dir("/dev/dri")
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| {
                    e.file_name().to_str().is_some_and(|n| {
                        n.strip_prefix("renderD")
                            .is_some_and(|n| n.parse::<u32>().is_ok())
                    })
                })
                .map(|e| e.path())
                .collect()
        } else {
            vec![]
        };
        nodes.sort();
        let started = Instant::now();
        for encoder in candidates(std::env::consts::OS, nodes) {
            check_cancel(cancel)?;
            if started.elapsed().as_secs() >= 20 {
                break;
            }
            match self.probe_video_encoder(&encoder, work, cancel) {
                Ok(()) => return Ok(encoder),
                Err(ConversionError::Cancelled) => return Err(ConversionError::Cancelled),
                Err(_) => (),
            }
        }
        Ok(Encoder::Software)
    }
    fn probe_video_encoder(
        &self,
        encoder: &Encoder,
        work: &Path,
        cancel: &AtomicBool,
    ) -> Result<(), ConversionError> {
        let raw = work.join("hardware-probe.rgb");
        // A changing picture also catches runtimes that initialize but emit no frames.
        let mut pixels = vec![0; 320 * 192 * 3 * 3];
        for (i, p) in pixels.iter_mut().enumerate() {
            *p = ((i * 13 + i / (320 * 192 * 3) * 41) % 256) as u8;
        }
        fs::write(&raw, pixels).map_err(|e| e.to_string())?;
        let output = work.join("hardware-probe.mp4");
        let mut cmd = self.video_command();
        encoder.initialize(&mut cmd);
        cmd.args([
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgb24",
            "-video_size",
            "320x192",
            "-framerate",
            "30",
            "-i",
        ])
        .arg(&raw);
        let filters = encoder.filters();
        if !filters.is_empty() {
            cmd.args(["-vf", &filters.join(",")]);
        }
        encoder.configure(&mut cmd, 85, 320, 192, 30.0);
        cmd.args(["-bf", "0", "-threads:v", "2", "-frames:v", "3", "-f", "mp4"])
            .arg(&output);
        run(
            &mut cmd,
            &work.join("hardware-probe.log"),
            None,
            cancel,
            5,
            &mut |_| {},
        )?;
        let info = self
            .inspect_video(&output, cancel)?
            .ok_or_else(|| "No hardware probe video.".to_string())?;
        if info.width != 320 || info.height != 192 || info.codec != "h264" {
            return Err("The hardware probe changed the expected picture."
                .to_string()
                .into());
        }
        let checked =
            self.video_fingerprint(&output, work, "hardware-probe", 0, cancel, &mut |_| {})?;
        if frame_stats(&checked, cancel)?
            .get(&0)
            .is_none_or(|s| s.count != 3)
        {
            return Err("The hardware probe did not decode all frames."
                .to_string()
                .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_retries_software_once_and_cancellation_never_retries() {
        let mut attempts = vec![];
        let (value, used, fallback) = with_fallback(&Encoder::Nvenc, |encoder| {
            attempts.push(encoder.clone());
            if *encoder == Encoder::Nvenc {
                Err("Driver lost its device.".to_string().into())
            } else {
                Ok(42)
            }
        })
        .unwrap();
        assert_eq!(attempts, vec![Encoder::Nvenc, Encoder::Software]);
        assert_eq!((value, used, fallback), (42, Encoder::Software, true));
        let mut count = 0;
        let result: Result<((), _, _), _> = with_fallback(&Encoder::Nvenc, |_| {
            count += 1;
            Err(ConversionError::Cancelled)
        });
        assert!(matches!(result, Err(ConversionError::Cancelled)));
        assert_eq!(count, 1);
        count = 0;
        let result: Result<((), _, _), _> = with_fallback(&Encoder::Software, |_| {
            count += 1;
            Err("Invalid video".to_string().into())
        });
        assert!(result.is_err());
        assert_eq!(count, 1);
    }
}
