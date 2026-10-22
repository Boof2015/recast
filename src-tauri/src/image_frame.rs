use crate::{
    animation::Sequence,
    image_backend::{check_cancel, ConversionError, ImageBackend, ImageOptions},
    video_frame::{fingerprint, unchanged, FramePreview, FrameSelection},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageFrameOptions {
    pub image: ImageOptions,
    pub frames: BTreeMap<String, FrameSelection>,
}
impl ImageFrameOptions {
    pub fn validate(&self) -> Result<(), String> {
        self.image.validate()?;
        for frame in self.frames.values() {
            if frame.pts.parse::<usize>().is_err() || !frame.time.is_finite() || frame.time < 0.0 {
                return Err("Choose a valid animation frame.".into());
            }
        }
        Ok(())
    }
}

// For animations the saved identifier is a frame ordinal, not a timestamp.
// Zero-delay frames can share timestamps but must still be individually selectable.
pub fn resolve(sequence: &Sequence, selection: Option<&FrameSelection>) -> Result<usize, String> {
    let Some(selection) = selection else {
        return Ok(0);
    };
    let index = selection
        .pts
        .parse::<usize>()
        .map_err(|_| "The frame number is invalid.")?;
    if index >= sequence.delays.len()
        || !selection.time.is_finite()
        || (time(sequence, index) - selection.time).abs() > 1e-8
    {
        return Err("The selected frame no longer matches this animation. Choose it again.".into());
    }
    Ok(index)
}
fn time(sequence: &Sequence, index: usize) -> f64 {
    sequence.delays[..index]
        .iter()
        .map(|ms| u64::from(*ms))
        .sum::<u64>() as f64
        / 1000.0
}
pub(crate) fn select(command: &mut Command, sequence: &Sequence, index: usize) {
    // Clone only after coalescing: a stored rectangle may depend on every prior
    // frame's blend/disposal. Delete the complete original sequence afterwards.
    command.args([
        "-clone",
        &index.to_string(),
        "-delete",
        &format!("0-{}", sequence.delays.len() - 1),
        "+repage",
    ]);
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationFramePreview {
    #[serde(flatten)]
    pub frame: FramePreview,
    pub index: usize,
    pub count: usize,
    pub previous_frame: Option<usize>,
    pub next_frame: Option<usize>,
    pub mime: &'static str,
}

impl ImageBackend {
    pub fn preview_animation_frame(
        &self,
        source: &Path,
        requested_time: f64,
        index: Option<usize>,
        cancel: &AtomicBool,
    ) -> Result<(AnimationFramePreview, Vec<u8>), ConversionError> {
        check_cancel(cancel)?;
        let identity = fingerprint(source)?;
        let input = crate::inputs::require_supported(source)?;
        let sequence = input
            .sequence
            .as_ref()
            .filter(|s| s.animated())
            .ok_or("Choose an animated GIF or WebP.".to_string())?;
        let duration = time(sequence, sequence.delays.len());
        if !requested_time.is_finite() || requested_time < 0.0 || requested_time > duration {
            return Err("Choose a position inside this animation."
                .to_string()
                .into());
        }
        let index = index.unwrap_or_else(|| {
            (0..sequence.delays.len())
                .rfind(|&i| time(sequence, i) <= requested_time + 1e-9)
                .unwrap_or(0)
        });
        if index >= sequence.delays.len() {
            return Err("The frame number is invalid.".to_string().into());
        }
        let work = tempfile::Builder::new()
            .prefix("recast-image-preview-")
            .tempdir()
            .map_err(|e| e.to_string())?;
        let staged = work.path().join("input");
        let mut input = File::open(source).map_err(|e| e.to_string())?;
        let mut output = File::create(&staged).map_err(|e| e.to_string())?;
        let mut buffer = [0; 65536];
        loop {
            check_cancel(cancel)?;
            let n = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        drop(output);
        unchanged(source, &identity)?;
        let inspected = crate::inputs::require_supported(&staged)?;
        let sequence = inspected
            .sequence
            .as_ref()
            .filter(|s| s.animated())
            .ok_or("This file is no longer an animation.".to_string())?;
        if index >= sequence.delays.len() {
            return Err("The selected frame is no longer available."
                .to_string()
                .into());
        }
        let selection = FrameSelection {
            pts: index.to_string(),
            time: time(sequence, index),
            source: identity.clone(),
        };
        resolve(sequence, Some(&selection))?;
        let picture = work.path().join("frame.png");
        let diagnostics = work.path().join("preview.log");
        let mut command = self.command();
        command
            .current_dir(work.path())
            .env("MAGICK_TEMPORARY_PATH", work.path())
            .args(["-limit", "list-length", "1001"])
            .arg(&staged)
            .args(["-alpha", "set", "-coalesce"]);
        select(&mut command, sequence, index);
        command
            .args(["-auto-orient", "-resize", "480x480>", "-profile"])
            .arg(self.root.join("sRGB.icc"))
            .args(["-colorspace", "sRGB", "-strip", "PNG:-"])
            .stdout(File::create(&picture).map_err(|e| e.to_string())?)
            .stderr(Stdio::from(
                File::create(&diagnostics).map_err(|e| e.to_string())?,
            ));
        crate::image_backend::run(&mut command, &diagnostics, cancel)?;
        crate::image_format::ImageFormat::Png.validate_output(&picture)?;
        if fs::metadata(&picture).map_err(|e| e.to_string())?.len() > 4_000_000 {
            return Err("The frame preview is too large.".to_string().into());
        }
        let bytes = fs::read(picture).map_err(|e| e.to_string())?;
        unchanged(source, &identity)?;
        check_cancel(cancel)?;
        Ok((
            AnimationFramePreview {
                frame: FramePreview {
                    selection,
                    previous: index.checked_sub(1).map(|i| time(sequence, i)),
                    next: (index + 1 < sequence.delays.len()).then(|| time(sequence, index + 1)),
                },
                index,
                count: sequence.delays.len(),
                previous_frame: index.checked_sub(1),
                next_frame: (index + 1 < sequence.delays.len()).then_some(index + 1),
                mime: "image/png",
            },
            bytes,
        ))
    }
}
