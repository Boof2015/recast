//! Per-file budgets are estimates; only the measured, validated MP4 may be published.
use super::{num, streams, StreamStats, VideoInfo, VideoOptions};
use crate::image_backend::{check_cancel, ConversionError};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

const MIN_VIDEO_RATE: u64 = 64_000;

#[derive(Clone, Debug)]
pub(super) struct SizePlan {
    pub limit: u64,
    pub video_rate: u64,
    pub audio_rates: Vec<u64>,
    pub width: u32,
    pub height: u32,
}

impl SizePlan {
    pub fn new(
        options: &VideoOptions,
        info: &VideoInfo,
        stats: &BTreeMap<usize, StreamStats>,
    ) -> Result<Option<Self>, String> {
        let Some(limit) = options.size_limit else {
            return Ok(None);
        };
        let duration = stats.values().map(|s| s.end).fold(info.duration, f64::max);
        let fps = options.frame_rate.map(f64::from).unwrap_or(info.frame_rate);
        let frames = options
            .frame_rate
            .map(|r| (duration * f64::from(r)).ceil())
            .unwrap_or(stats[&0].count as f64);
        let audio: Vec<_> = streams(&info.probe)
            .filter(|s| s["codec_type"] == "audio")
            .collect();
        let channels: f64 = audio.iter().map(|a| num(&a["channels"]).unwrap()).sum();
        // Reserve MP4 sample tables, audio packets, tags, and a 6% rate-control margin.
        let overhead = 16_384.0
            + frames * 24.0
            + audio
                .iter()
                .map(|a| duration * num(&a["sample_rate"]).unwrap() / 1024.0 * 24.0)
                .sum::<f64>()
            + if options.metadata {
                info.probe.to_string().len() as f64 * 2.0
            } else {
                0.0
            };
        let total_rate = ((limit as f64 * 0.94 - overhead).max(0.0) * 8.0 / duration) as u64;
        let audio_rates: Vec<_> = audio
            .iter()
            .map(|a| {
                let count = num(&a["channels"]).unwrap();
                let rate = num(&a["sample_rate"]).unwrap();
                ((total_rate as f64 * 0.2 / channels)
                    .clamp(16_000.0, 64_000.0)
                    .min(rate * 6.0)
                    * count) as u64
            })
            .collect();
        let video_rate = total_rate
            .saturating_sub(audio_rates.iter().sum())
            .min(240_000_000);
        if video_rate < MIN_VIDEO_RATE {
            return Err(too_small());
        }
        let mut dimensions = options.dimensions(info);
        if options.auto_resolution {
            for resolution in [None, Some(1080), Some(720), Some(480), Some(360), Some(240)] {
                let mut candidate = options.clone();
                candidate.resolution = resolution;
                dimensions = candidate.dimensions(info);
                let needed = f64::from(dimensions.0) * f64::from(dimensions.1) * fps * 0.04;
                if video_rate as f64 >= needed {
                    break;
                }
            }
            // Do not turn high-frame-rate clips into arbitrarily tiny pictures.
            if (video_rate as f64) < f64::from(dimensions.0) * f64::from(dimensions.1) * fps * 0.015
            {
                return Err(too_small());
            }
        }
        Ok(Some(Self {
            limit,
            video_rate,
            audio_rates,
            width: dimensions.0,
            height: dimensions.1,
        }))
    }

    pub fn reduce(&mut self, actual: u64) -> Result<(), String> {
        let rate =
            (self.video_rate as f64 * (self.limit as f64 * 0.9 / actual as f64).min(0.9)) as u64;
        if rate < MIN_VIDEO_RATE {
            return Err(too_small());
        }
        self.video_rate = rate;
        Ok(())
    }
}

pub(super) fn too_small() -> String {
    "This size limit is too small for the complete clip and its retained tracks. Increase the limit or choose a lower resolution or frame rate. No output was saved.".into()
}

pub(super) enum EncodeAttempt<T> {
    Validated(T),
    TooLarge(u64),
}

pub(super) fn fit<T>(
    plan: Option<&SizePlan>,
    cancel: &AtomicBool,
    mut encode: impl FnMut(Option<&SizePlan>, u8) -> Result<EncodeAttempt<T>, ConversionError>,
) -> Result<T, ConversionError> {
    let mut plan = plan.cloned();
    for attempt in 0..if plan.is_some() { 2 } else { 1 } {
        check_cancel(cancel)?;
        match encode(plan.as_ref(), attempt)? {
            EncodeAttempt::Validated(file) => return Ok(file),
            EncodeAttempt::TooLarge(bytes) => {
                if attempt == 1 || plan.is_none() {
                    break;
                }
                plan.as_mut().unwrap().reduce(bytes)?;
            }
        }
    }
    Err(too_small().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn size_retry_is_bounded_and_only_a_validated_attempt_can_succeed() {
        let plan = SizePlan {
            limit: 1_000_000,
            video_rate: 1_000_000,
            audio_rates: vec![128_000],
            width: 640,
            height: 480,
        };
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let output = fit(Some(&plan), &cancel, |revised, attempt| {
            calls += 1;
            if attempt == 0 {
                return Ok(EncodeAttempt::TooLarge(1_200_000));
            }
            let revised = revised.unwrap();
            assert!(revised.video_rate < plan.video_rate);
            assert_eq!(revised.audio_rates, plan.audio_rates);
            assert_eq!((revised.width, revised.height), (plan.width, plan.height));
            Ok(EncodeAttempt::Validated("checked output"))
        })
        .unwrap();
        assert_eq!(output, "checked output");
        assert_eq!(calls, 2);
        calls = 0;
        let result: Result<(), _> = fit(Some(&plan), &cancel, |_, _| {
            calls += 1;
            Ok(EncodeAttempt::TooLarge(1_200_000))
        });
        assert!(result.is_err());
        assert_eq!(calls, 2);
        calls = 0;
        let result: Result<(), _> = fit(Some(&plan), &cancel, |_, _| {
            calls += 1;
            cancel.store(true, Ordering::Relaxed);
            Ok(EncodeAttempt::TooLarge(1_200_000))
        });
        assert!(matches!(result, Err(ConversionError::Cancelled)));
        assert_eq!(calls, 1);
    }
}
