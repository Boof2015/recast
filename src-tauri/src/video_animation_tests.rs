use super::*;
use crate::video_animation::{AnimationOptions, SourceRange};

fn settings() -> AnimationOptions {
    AnimationOptions {
        size: 480,
        frame_rate: 15,
        colors: 256,
        dither: true,
        ranges: Default::default(),
    }
}
fn animation(path: &Path) -> crate::animation::Sequence {
    crate::animation::inspect_gif(&mut fs::File::open(path).unwrap()).unwrap()
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
            .convert_gif(
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
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("video.mp4"));
    for phase in ["Reading selected range", "Encoding GIF"] {
        let cancel = AtomicBool::new(false);
        let result = audio_backend().convert_gif(
            &backend(),
            &source,
            None,
            (
                &settings(),
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
        assert!(!dir.path().join("video.gif").exists());
        no_partials(dir.path());
    }
}

#[test]
fn video_gif_mixed_batch_retry_freezes_each_range_and_group_options() {
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
        ..settings()
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
            .convert_gif(
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
        .convert_gif(
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
