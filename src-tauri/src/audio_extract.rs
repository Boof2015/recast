use crate::{
    audio_backend::{parse_audio_stream, AudioBackend, AudioInfo, AudioOptions},
    image_backend::{ConversionError, OutputFile},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::atomic::AtomicBool};

pub const TARGETS: [&str; 7] = ["wav", "flac", "alac", "mp3", "m4a", "opus", "ogg"];

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VideoAudioOptions {
    pub audio: AudioOptions,
    // Absolute stream indexes, keyed by the canonical input path. Frozen with
    // the job so retry cannot change which soundtrack is extracted.
    pub tracks: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub index: u32,
    pub label: String,
    pub default: bool,
    pub audio: Option<AudioInfo>,
    pub issue: Option<String>,
    pub targets: Vec<String>,
    pub target_issues: BTreeMap<String, String>,
}

impl AudioTrack {
    pub fn supported(&self) -> Result<&AudioInfo, String> {
        self.audio.as_ref().ok_or_else(|| {
            self.issue
                .clone()
                .unwrap_or_else(|| "This audio track is not supported yet.".into())
        })
    }
}

pub fn has_video(probe: &Value) -> bool {
    probe["streams"].as_array().is_some_and(|streams| {
        streams
            .iter()
            .any(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1)
    })
}

pub fn tracks(probe: &Value) -> Vec<AudioTrack> {
    let container = match probe["format"]["format_name"].as_str().unwrap_or("") {
        "mov,mp4,m4a,3gp,3g2,mj2" => Some("mov"),
        "matroska,webm" => Some("matroska"),
        "avi" => Some("avi"),
        _ => None,
    };
    probe["streams"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["codec_type"] == "audio")
        .enumerate()
        .filter_map(|(ordinal, stream)| {
            let index = u32::try_from(stream["index"].as_u64()?).ok()?;
            let default = stream["disposition"]["default"] == 1;
            let mut label = format!("Track {}", ordinal + 1);
            for key in ["title", "language"] {
                if let Some(value) = stream["tags"][key]
                    .as_str()
                    .filter(|v| !v.is_empty() && *v != "und")
                {
                    label.push_str(" · ");
                    label.extend(value.chars().take(80));
                }
            }
            if default {
                label.push_str(" · Default");
            }
            let parsed = container
                .ok_or_else(|| {
                    "Audio extraction supports MP4/MOV, MKV/WebM, and AVI containers.".to_string()
                })
                .and_then(|container| parse_audio_stream(probe, stream, container));
            let mut targets = Vec::new();
            let mut target_issues = BTreeMap::new();
            for target in TARGETS {
                let issue = match &parsed {
                    Ok(audio) => audio.target_issue(target),
                    Err(issue) => Some(issue.clone()),
                };
                if let Some(issue) = issue {
                    target_issues.insert(target.into(), issue);
                } else {
                    targets.push(target.into());
                }
            }
            let (audio, issue) = match parsed {
                Ok(info) => (Some(info), None),
                Err(issue) => (None, Some(issue)),
            };
            Some(AudioTrack {
                index,
                label,
                default,
                audio,
                issue,
                targets,
                target_issues,
            })
        })
        .collect()
}

pub fn selected_track(probe: &Value, selected: Option<u32>) -> Result<AudioTrack, String> {
    if !has_video(probe) {
        return Err("This file no longer contains video.".into());
    }
    let tracks = tracks(probe);
    let selected = selected.or_else(|| {
        tracks
            .iter()
            .find(|t| t.default)
            .or_else(|| tracks.first())
            .map(|t| t.index)
    });
    match selected {
        Some(index) => tracks.into_iter().find(|t| t.index == index)
            .ok_or_else(|| "The selected audio track is no longer available. Add this video again to choose a track.".into()),
        None => Err("This video has no audio track. Remove it to convert the rest to audio, or choose a video format.".into()),
    }
}

impl AudioBackend {
    pub fn extract_audio(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &AudioOptions,
        selected: Option<u32>,
        cancel: &AtomicBool,
    ) -> Result<OutputFile, ConversionError> {
        let probe = self.probe_media(source, cancel)?;
        let track = selected_track(&probe, selected)?;
        options.resolve(track.supported()?)?;
        // Reinspect the staged copy and map this exact stream, never FFmpeg's
        // automatic 'best' track. Video pixels and other tracks are not decoded.
        let mut output = self.convert_stream(source, folder, options, Some(track.index), cancel)?;
        if let Some(details) = &mut output.details {
            details.push_str(" · ");
            details.push_str(&track.label);
        }
        Ok(output)
    }
}
