use super::*;
use crate::video_animation::{AnimationOptions, SourceRange};

fn settings() -> AnimationOptions {
    AnimationOptions {
        target: "gif".into(),
        quality: 85,
        lossless: false,
        size: 480,
        frame_rate: 15,
        colors: 256,
        dither: true,
        ranges: Default::default(),
    }
}
fn animation(path: &Path) -> crate::animation::Sequence {
    let mut file = fs::File::open(path).unwrap();
    if path.extension().unwrap() == "webp" {
        crate::animation::inspect_webp(&mut file).unwrap().unwrap()
    } else {
        crate::animation::inspect_gif(&mut file).unwrap()
    }
}

#[test]
fn video_gif_selected_ranges_cfr_vfr_rotation_and_av1_have_complete_timing() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-av1.mp4",
        "video-vp9-opus.webm",
        "video-vp8-vorbis.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        let original = fs::read(&source).unwrap();
        let info = crate::video_animation::inspect(&probe(&source)).unwrap();
        let range = SourceRange {
            start: 0.2,
            end: info.duration.min(0.87),
        };
        let output = audio_backend()
            .convert_animation(
                &backend(),
                &source,
                None,
                (&settings(), range),
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
        let gif = animation(Path::new(&output.path));
        assert_eq!(gif.loops, 0);
        assert!(
            (gif.delays.iter().sum::<u32>() as f64 - (range.end - range.start) * 1000.0).abs()
                <= 5.01
        );
        assert_eq!((gif.width, gif.height), info.dimensions(480));
        assert!(
            gif.delays.len() >= 9 && gif.delays.len() <= 11,
            "{name}: {:?}",
            gif.delays
        );
        assert_eq!(fs::read(source).unwrap(), original);
        if let Some(folder) = std::env::var_os("RECAST_GIF_EVIDENCE") {
            fs::create_dir_all(&folder).unwrap();
            fs::copy(&output.path, Path::new(&folder).join(format!("{name}.gif"))).unwrap();
        }
    }
    no_partials(dir.path());
}

#[test]
fn video_gif_ignores_audio_codecs_but_rejects_unpreserved_pixels_and_invalid_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("he-aac.mp4");
    ffmpeg(&[
        "-i",
        fixture("apple-h264.mp4").to_str().unwrap(),
        "-i",
        fixture("apple-aac-hev2.m4a").to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c",
        "copy",
        source.to_str().unwrap(),
    ]);
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert!(input.animation.is_some());
    assert!(serde_json::to_value(&input).unwrap()["targets"]
        .as_array()
        .unwrap()
        .contains(&"gif".into()));
    let info = input.animation.unwrap();
    for range in [
        SourceRange {
            start: -1.0,
            end: 1.0,
        },
        SourceRange {
            start: 1.0,
            end: 0.0,
        },
        SourceRange {
            start: 0.0,
            end: info.duration + 1.0,
        },
        SourceRange {
            start: f64::NAN,
            end: 1.0,
        },
    ] {
        assert!(settings().resolve(&info, range).is_err());
    }
    assert!(crate::video_animation::inspect(&probe(&fixture("video-av1-10bit.mkv"))).is_err());
    let large = crate::video_animation::AnimationInfo {
        duration: 100.0,
        width: 3840,
        height: 2160,
        stream_index: 0,
    };
    assert!(settings()
        .resolve(
            &large,
            SourceRange {
                start: 0.0,
                end: 100.0
            }
        )
        .is_err());
}

#[test]
fn video_preview_frames_are_small_seekable_and_cancellable() {
    let source = fixture("apple-h264.mp4");
    let a = audio_backend()
        .preview_frame(&source, 0.0, &AtomicBool::new(false))
        .unwrap();
    let b = audio_backend()
        .preview_frame(&source, 0.75, &AtomicBool::new(false))
        .unwrap();
    let duration = crate::video_animation::inspect(&probe(&source))
        .unwrap()
        .duration;
    for time in [
        duration - 0.01,
        duration - 0.001,
        duration - 0.000001,
        duration,
    ] {
        audio_backend()
            .preview_frame(&source, time, &AtomicBool::new(false))
            .unwrap();
    }
    assert_eq!(&a[..2], b"BM");
    assert_ne!(a, b);
    assert!(a.len() <= 480 * 480 * 3 + 2000);
    assert!(audio_backend()
        .preview_frame(&source, 999.0, &AtomicBool::new(false))
        .is_err());
    assert!(matches!(
        audio_backend().preview_frame(&source, 0.0, &AtomicBool::new(true)),
        Err(ConversionError::Cancelled)
    ));
    if let Some(folder) = std::env::var_os("RECAST_GIF_EVIDENCE") {
        fs::create_dir_all(&folder).unwrap();
        fs::write(Path::new(&folder).join("frame-075.bmp"), b).unwrap();
    }
}

#[test]
fn video_gif_cancel_before_and_after_decoding_leaves_no_output() {
    for target in ["gif", "webp"] {
        let options = AnimationOptions {
            target: target.into(),
            ..settings()
        };
        let dir = tempfile::tempdir().unwrap();
        let source = copy("apple-h264.mp4", &dir.path().join("video.mp4"));
        for phase in [
            "Reading selected range",
            &format!("Encoding {}", options.label()),
        ] {
            let cancel = AtomicBool::new(false);
            let result = audio_backend().convert_animation(
                &backend(),
                &source,
                None,
                (
                    &options,
                    SourceRange {
                        start: 0.0,
                        end: 0.5,
                    },
                ),
                &cancel,
                |_, current| {
                    if current == phase {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
            );
            assert!(matches!(result, Err(ConversionError::Cancelled)));
            assert!(!dir.path().join(format!("video.{target}")).exists());
            no_partials(dir.path());
        }
    }
}

#[test]
fn video_gif_mixed_batch_retry_freezes_each_range_and_group_options() {
    for target in ["gif", "webp"] {
        let options = AnimationOptions {
            target: target.into(),
            ..settings()
        };
        let dir = tempfile::tempdir().unwrap();
        let a = copy("apple-h264.mp4", &dir.path().join("a.mp4"));
        let b = dir.path().join("b.mp4");
        let image = copy("rgba.png", &dir.path().join("still.png"));
        let mut req = request(&[a.clone(), b.clone(), image], dir.path());
        req.animation_options = Some(AnimationOptions {
            ranges: [
                (
                    a.to_string_lossy().into_owned(),
                    SourceRange {
                        start: 0.1,
                        end: 0.6,
                    },
                ),
                (
                    b.to_string_lossy().into_owned(),
                    SourceRange {
                        start: 0.5,
                        end: 1.2,
                    },
                ),
            ]
            .into(),
            ..options.clone()
        });
        let manager = JobManager::default();
        let job = manager.prepare("gif", req.clone(), false).unwrap();
        run_job(&job, &backend(), &audio_backend(), |_| {});
        let before = job.snapshot();
        assert_eq!(before.files[0].status, FileStatus::Succeeded, "{before:?}");
        assert_eq!(before.files[1].status, FileStatus::Failed);
        assert_eq!(before.files[2].status, FileStatus::Succeeded);
        copy("apple-h264.mp4", &b);
        req.animation_options.as_mut().unwrap().ranges.clear();
        req.animation_options.as_mut().unwrap().target = "invalid".into();
        let retry = manager.prepare("gif", req, true).unwrap();
        run_job(&retry, &backend(), &audio_backend(), |_| {});
        let after = retry.snapshot();
        assert!(
            after
                .files
                .iter()
                .all(|f| f.status == FileStatus::Succeeded),
            "{after:?}"
        );
        assert_eq!(before.files[0].output_path, after.files[0].output_path);
        assert_eq!(
            animation(Path::new(after.files[1].output_path.as_ref().unwrap()))
                .delays
                .iter()
                .sum::<u32>(),
            700
        );
        no_partials(dir.path());
    }
}

#[test]
fn video_gif_tiny_end_ranges_and_nonzero_stream_index_use_the_visible_picture() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("audio-first.mp4");
    ffmpeg(&[
        "-i",
        fixture("apple-aac-lc.m4a").to_str().unwrap(),
        "-i",
        fixture("apple-h264.mp4").to_str().unwrap(),
        "-map",
        "0:a",
        "-map",
        "1:v",
        "-c",
        "copy",
        source.to_str().unwrap(),
    ]);
    let info = crate::video_animation::inspect(&probe(&source)).unwrap();
    assert_eq!(info.stream_index, 1);
    let mut with_cover = probe(&source);
    let mut cover = with_cover["streams"][1].clone();
    cover["index"] = serde_json::json!(2);
    cover["disposition"]["attached_pic"] = serde_json::json!(1);
    with_cover["streams"]
        .as_array_mut()
        .unwrap()
        .insert(0, cover);
    assert_eq!(
        crate::video_animation::inspect(&with_cover)
            .unwrap()
            .stream_index,
        1
    );
    for range in [
        SourceRange {
            start: 0.0,
            end: 0.01,
        },
        SourceRange {
            start: info.duration - 0.01,
            end: info.duration,
        },
        SourceRange {
            start: 0.0,
            end: 0.07,
        },
        SourceRange {
            start: 0.0,
            end: 0.08,
        },
        SourceRange {
            start: 0.0,
            end: 0.14,
        },
        SourceRange {
            start: 0.0,
            end: 0.67,
        },
        SourceRange {
            start: 0.0,
            end: info.duration,
        },
    ] {
        let output = audio_backend()
            .convert_animation(
                &backend(),
                &source,
                None,
                (&settings(), range),
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
        let gif = animation(Path::new(&output.path));
        if gif.animated() {
            assert!(gif.delays.iter().all(|ms| *ms >= 20));
        }
        assert!(
            (gif.delays.iter().sum::<u32>() as f64 - (range.end - range.start) * 1000.0).abs()
                <= 5.01
        );
        assert_eq!((gif.width, gif.height), info.dimensions(480));
    }
}

#[test]
#[ignore = "manual real-file verification; requires RECAST_GIF_SOURCE and RECAST_GIF_EVIDENCE"]
fn video_gif_real_source() {
    let source = PathBuf::from(std::env::var_os("RECAST_GIF_SOURCE").unwrap());
    let folder = PathBuf::from(std::env::var_os("RECAST_GIF_EVIDENCE").unwrap());
    fs::create_dir_all(&folder).unwrap();
    let range = SourceRange {
        start: 37.25,
        end: 42.25,
    };
    let result = audio_backend()
        .convert_animation(
            &backend(),
            &source,
            Some(&folder),
            (&settings(), range),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
    assert_eq!(
        animation(Path::new(&result.path))
            .delays
            .iter()
            .sum::<u32>(),
        5000
    );
    eprintln!("{}", result.path);
}

#[test]
fn video_preview_final_boundary_uses_the_last_frame_for_vfr_and_high_rates() {
    let dir = tempfile::tempdir().unwrap();
    let high_rate = make_video(dir.path(), "fast.mp4", "240", None);
    for source in [fixture("apple-vfr-rotated.mov"), high_rate] {
        let duration = crate::video_animation::inspect(&probe(&source))
            .unwrap()
            .duration;
        let last = audio_backend()
            .preview_frame(&source, duration, &AtomicBool::new(false))
            .unwrap();
        let inside = audio_backend()
            .preview_frame(&source, duration - 0.001, &AtomicBool::new(false))
            .unwrap();
        let earlier = audio_backend()
            .preview_frame(&source, duration - 0.1, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(last, inside);
        assert_ne!(last, earlier);
        assert!(audio_backend()
            .preview_frame(&source, duration + 0.001, &AtomicBool::new(false))
            .is_err());
    }
}

fn webp_settings() -> AnimationOptions {
    AnimationOptions {
        target: "webp".into(),
        ..settings()
    }
}
fn webp_evidence(path: &Path, name: &str) {
    if let Some(folder) = std::env::var_os("RECAST_VIDEO_WEBP_EVIDENCE") {
        fs::create_dir_all(&folder).unwrap();
        fs::copy(path, Path::new(&folder).join(name)).unwrap();
    }
}
#[test]
fn video_webp_selected_ranges_preserve_sampled_frames_dimensions_and_millisecond_timing() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-av1.mp4",
        "video-av1-vfr.mkv",
        "video-vp9-opus.webm",
        "video-vp8-vorbis.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        let before = fs::read(&source).unwrap();
        let range = SourceRange {
            start: 0.2,
            end: 0.871,
        };
        let info =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert!(serde_json::to_value(&info).unwrap()["targets"]
            .as_array()
            .unwrap()
            .contains(&"animated-webp".into()));
        let output = audio_backend()
            .convert_animation(
                &backend(),
                &source,
                None,
                (&webp_settings(), range),
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
        let path = Path::new(&output.path);
        assert_eq!(path.extension().unwrap(), "webp");
        let actual = animation(path);
        assert_eq!(
            (actual.width, actual.height),
            info.animation.unwrap().dimensions(480)
        );
        assert_eq!(actual.loops, 0);
        assert_eq!(actual.delays.iter().sum::<u32>(), 671);
        assert!(actual.delays.iter().all(|ms| *ms >= 20));
        assert!((10..=11).contains(&actual.delays.len()));
        assert_eq!(fs::read(&source).unwrap(), before);
        webp_evidence(path, &format!("{name}.webp"));
        // Publication protects both existing exports and source files.
        let another = audio_backend()
            .convert_animation(
                &backend(),
                &source,
                None,
                (&webp_settings(), range),
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
        assert_ne!(another.path, output.path);
        assert!(path.is_file());
    }
    no_partials(dir.path());
}

#[test]
fn video_webp_lossless_matches_decoded_pixels_and_quality_100_remains_lossy() {
    let source = fixture("apple-h264.mp4");
    let range = SourceRange {
        start: 0.1,
        end: 0.7,
    };
    let mut sizes = Vec::new();
    for (quality, lossless) in [(1, false), (100, false), (85, true)] {
        let work = tempfile::tempdir().unwrap();
        let options = AnimationOptions {
            quality,
            lossless,
            size: 320,
            ..webp_settings()
        };
        let path = audio_backend()
            .render_animation(
                &backend(),
                &source,
                work.path(),
                (&options, range),
                &AtomicBool::new(false),
                &mut |_, _| {},
            )
            .unwrap();
        let actual = animation(&path);
        let bytes = fs::read(&path).unwrap();
        assert!(bytes
            .windows(4)
            .any(|chunk| chunk == if lossless { b"VP8L" } else { b"VP8 " }));
        sizes.push(bytes.len());
        if lossless {
            for index in 0..actual.delays.len() {
                let output = backend()
                    .command()
                    .arg(&path)
                    .args([
                        "-coalesce",
                        "-clone",
                        &index.to_string(),
                        "-delete",
                        &format!("0-{}", actual.delays.len() - 1),
                        "-depth",
                        "8",
                        "-define",
                        "bmp:format=bmp3",
                        "BMP:-",
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                // Compare normalized image signatures: BMP storage direction may differ.
                let frame = work.path().join("frame.bmp");
                fs::write(&frame, output.stdout).unwrap();
                assert_eq!(
                    identify(&frame, "%#"),
                    identify(&work.path().join(format!("frame-{index:04}.bmp")), "%#")
                );
            }
        }
        assert!(identify(&path, "%[profiles]").is_empty());
        webp_evidence(
            &path,
            &format!("quality-{quality}-lossless-{lossless}.webp"),
        );
        if lossless {
            webp_evidence(&work.path().join("frames.rgb"), "lossless-reference.rgb");
        }
    }
    assert!(sizes[0] < sizes[1], "{sizes:?}");
}

#[test]
fn video_webp_short_final_frames_and_all_rates_keep_the_requested_duration() {
    let source = fixture("apple-h264.mp4");
    let info = crate::video_animation::inspect(&probe(&source)).unwrap();
    for rate in [10, 15, 20, 25, 30] {
        for duration in [0.01, 0.04, 0.07, 0.08, 0.14, 0.671] {
            let work = tempfile::tempdir().unwrap();
            let range = SourceRange {
                start: info.duration - duration,
                end: info.duration,
            };
            let options = AnimationOptions {
                frame_rate: rate,
                lossless: true,
                ..webp_settings()
            };
            let path = audio_backend()
                .render_animation(
                    &backend(),
                    &source,
                    work.path(),
                    (&options, range),
                    &AtomicBool::new(false),
                    &mut |_, _| {},
                )
                .unwrap();
            let sequence =
                crate::animation::inspect_webp(&mut fs::File::open(&path).unwrap()).unwrap();
            if let Some(actual) = sequence {
                assert_eq!(
                    actual.delays.iter().sum::<u32>(),
                    (duration * 1000.0).round() as u32
                );
                assert!(actual.delays.iter().all(|ms| *ms >= 20));
            } else {
                // One sampled picture is encoded as an ordinary still WebP.
                assert_eq!(identify(&path, "%n"), "1");
                let (width, height, _) = options.resolve(&info, range).unwrap();
                assert_eq!(
                    fs::metadata(work.path().join("frames.rgb")).unwrap().len(),
                    u64::from(width) * u64::from(height) * 3
                );
            }
            webp_evidence(&path, &format!("end-{duration}-{rate}fps.webp"));
        }
    }
}

#[test]
fn video_webp_rejects_unsupported_options_picture_features_and_unbounded_ranges() {
    let info = crate::video_animation::AnimationInfo {
        duration: 1e10,
        width: 1920,
        height: 1080,
        stream_index: 0,
    };
    for options in [
        AnimationOptions {
            target: "png".into(),
            ..webp_settings()
        },
        AnimationOptions {
            quality: 0,
            ..webp_settings()
        },
        AnimationOptions {
            frame_rate: 60,
            ..webp_settings()
        },
        AnimationOptions {
            size: 2000,
            ..webp_settings()
        },
    ] {
        assert!(options.validate().is_err());
    }
    for range in [
        SourceRange {
            start: f64::NAN,
            end: 1.0,
        },
        SourceRange {
            start: 0.0,
            end: 1e10,
        },
        SourceRange {
            start: 1.0,
            end: 0.0,
        },
    ] {
        assert!(webp_settings().resolve(&info, range).is_err());
    }
    let input = crate::inputs::inspect_with_audio(
        &fixture("video-av1-10bit.mkv"),
        &audio_backend(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let json = serde_json::to_value(input).unwrap();
    assert!(!json["targets"]
        .as_array()
        .unwrap()
        .contains(&"animated-webp".into()));
    assert!(json["targetIssues"]["animated-webp"]
        .as_str()
        .unwrap()
        .contains("bit"));
}

#[test]
#[ignore = "manual real-file verification; requires RECAST_VIDEO_WEBP_SOURCE and RECAST_VIDEO_WEBP_EVIDENCE"]
fn video_webp_real_source() {
    let source = PathBuf::from(std::env::var_os("RECAST_VIDEO_WEBP_SOURCE").unwrap());
    let folder = PathBuf::from(std::env::var_os("RECAST_VIDEO_WEBP_EVIDENCE").unwrap());
    fs::create_dir_all(&folder).unwrap();
    let before = crate::video_frame::fingerprint(&source).unwrap();
    let range = SourceRange {
        start: 37.25,
        end: 42.25,
    };
    let result = audio_backend()
        .convert_animation(
            &backend(),
            &source,
            Some(&folder),
            (&webp_settings(), range),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
    let actual = animation(Path::new(&result.path));
    assert_eq!(actual.delays.iter().sum::<u32>(), 5000);
    assert_eq!(actual.delays.len(), 75);
    assert_eq!(actual.loops, 0);
    crate::video_frame::unchanged(&source, &before).unwrap();
    eprintln!(
        "{}: {} × {}, {} bytes",
        result.path, actual.width, actual.height, result.bytes
    );
}
