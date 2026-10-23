use super::*;

fn settings(copy_streams: bool) -> VideoOptions {
    VideoOptions {
        target: "mkv".into(),
        copy_streams,
        ..video_options()
    }
}

#[test]
fn mkv_copies_independent_cfr_vfr_rotation_and_vp9_opus_without_reencoding() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-vp9-opus.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        let original = fs::read(&source).unwrap();
        let mut phases = Vec::new();
        let output = audio_backend()
            .convert_video(
                &source,
                None,
                &settings(true),
                &AtomicBool::new(false),
                |_, phase| phases.push(phase.to_string()),
            )
            .unwrap();
        assert!(output.path.ends_with(".mkv"));
        assert!(output.details.unwrap().contains("Copied streams"));
        assert!(phases.iter().any(|phase| phase == "Copying streams"));
        assert!(!phases
            .iter()
            .any(|phase| phase.contains("Encoding") || phase.contains("hardware")));
        assert_eq!(fs::read(&source).unwrap(), original);
        let repeated = convert(Path::new(&output.path), &settings(true));
        assert_ne!(output.path, repeated.path);
        if let Some(evidence) = std::env::var_os("RECAST_MKV_EVIDENCE") {
            fs::create_dir_all(&evidence).unwrap();
            fs::copy(
                &output.path,
                Path::new(&evidence).join(format!("{name}.mkv")),
            )
            .unwrap();
        }
    }
    no_partials(dir.path());
}

#[test]
fn mkv_quality_and_size_keep_complete_video_audio_and_rotation() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-vp9-opus.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        for size in [false, true] {
            let options = if size {
                VideoOptions {
                    target: "mkv".into(),
                    ..size_options(100_000)
                }
            } else {
                settings(false)
            };
            let output = convert(&source, &options);
            let data = probe(Path::new(&output.path));
            assert_eq!(data["streams"][0]["codec_name"], "h264");
            if name.contains("opus") {
                assert_eq!(
                    data["streams"][1]["codec_name"],
                    if size { "aac" } else { "opus" }
                );
            }
            if size {
                assert!(output.bytes <= 100_000);
            }
            if let Some(evidence) = std::env::var_os("RECAST_MKV_EVIDENCE") {
                fs::create_dir_all(&evidence).unwrap();
                fs::copy(
                    &output.path,
                    Path::new(&evidence).join(format!(
                        "{name}-{}.mkv",
                        if size { "size" } else { "quality" }
                    )),
                )
                .unwrap();
            }
        }
    }
    no_partials(dir.path());
}

#[test]
fn mkv_retains_multiple_aac_tracks_offsets_metadata_and_exact_playable_samples() {
    let dir = tempfile::tempdir().unwrap();
    let silent = make_video(dir.path(), "silent.mp4", "30", None);
    let audio = fixture("apple-aac-lc.m4a");
    for offset in ["0.005", "0.25", "2.25"] {
        let source = dir.path().join(format!("tracks-{offset}.mp4"));
        ffmpeg(&[
            "-i",
            silent.to_str().unwrap(),
            "-i",
            audio.to_str().unwrap(),
            "-itsoffset",
            offset,
            "-i",
            audio.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-c",
            "copy",
            "-metadata",
            "title=Retained title",
            "-metadata:s:a:0",
            "language=eng",
            "-metadata:s:a:1",
            "language=fra",
            "-disposition:a:0",
            "default",
            "-disposition:a:1",
            "0",
            source.to_str().unwrap(),
        ]);
        for mode in [
            settings(true),
            settings(false),
            VideoOptions {
                target: "mkv".into(),
                ..size_options(100_000)
            },
        ] {
            for metadata in [true, false] {
                let output = convert(
                    &source,
                    &VideoOptions {
                        metadata,
                        ..mode.clone()
                    },
                );
                let data = probe(Path::new(&output.path));
                assert_eq!(data["streams"][1]["tags"]["language"], "eng");
                assert_eq!(data["streams"][2]["tags"]["language"], "fra");
                assert_eq!(data["streams"][2]["disposition"]["default"], 0);
                assert_eq!(
                    data["format"]["tags"]["title"] == "Retained title",
                    metadata
                );
                let before: f64 = probe(&source)["format"]["duration"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                let after: f64 = data["format"]["duration"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                assert!((before - after).abs() <= 0.00101, "{before} vs {after}");
                if metadata {
                    if let Some(evidence) = std::env::var_os("RECAST_MKV_EVIDENCE") {
                        fs::create_dir_all(&evidence).unwrap();
                        let mode = if mode.copy_streams {
                            "copy"
                        } else if mode.size_limit.is_some() {
                            "size"
                        } else {
                            "quality"
                        };
                        fs::copy(
                            &output.path,
                            Path::new(&evidence).join(format!("offset-{offset}-{mode}.mkv")),
                        )
                        .unwrap();
                    }
                }
            }
        }
    }
    no_partials(dir.path());
}

#[test]
fn mkv_track_mapping_does_not_depend_on_video_being_the_first_stream() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("reordered.mkv");
    ffmpeg(&[
        "-i",
        fixture("video-srt.mkv").to_str().unwrap(),
        "-i",
        fixture("apple-aac-lc.m4a").to_str().unwrap(),
        "-map",
        "0:s",
        "-map",
        "1:a",
        "-map",
        "0:v",
        "-c",
        "copy",
        source.to_str().unwrap(),
    ]);
    for options in [
        settings(true),
        settings(false),
        VideoOptions {
            target: "mkv".into(),
            ..size_options(100_000)
        },
    ] {
        let output = convert(
            &source,
            &VideoOptions {
                metadata: false,
                ..options
            },
        );
        assert_eq!(
            probe(Path::new(&output.path))["streams"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }
}

#[test]
fn mkv_retains_supported_audio_codecs_and_high_rates_without_silent_aac_conversion() {
    let dir = tempfile::tempdir().unwrap();
    for codec in [
        "flac",
        "pcm_s24le",
        "pcm_f32le",
        "alac",
        "libmp3lame",
        "libvorbis",
    ] {
        let source = dir.path().join(format!("{codec}.mkv"));
        ffmpeg(&[
            "-i",
            fixture("apple-h264.mp4").to_str().unwrap(),
            "-i",
            fixture("apple-aac-lc.m4a").to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "copy",
            "-c:a",
            codec,
            "-ar",
            if codec.starts_with("pcm") {
                "192000"
            } else {
                "48000"
            },
            source.to_str().unwrap(),
        ]);
        let input = probe(&source);
        let inspected =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        for copy_streams in [true, false] {
            inspected
                .require_video_target(&settings(copy_streams))
                .unwrap();
            let result = convert(&source, &settings(copy_streams));
            let output = probe(Path::new(&result.path));
            assert_eq!(
                input["streams"][1]["codec_name"],
                output["streams"][1]["codec_name"]
            );
            assert_eq!(
                input["streams"][1]["sample_rate"],
                output["streams"][1]["sample_rate"]
            );
        }
        if codec.starts_with("pcm") {
            assert!(inspected
                .require_video_target(&video_options())
                .unwrap_err()
                .contains("sample rate"));
            assert!(inspected
                .require_video_target(&VideoOptions {
                    target: "mkv".into(),
                    ..size_options(100_000)
                })
                .unwrap_err()
                .contains("sample rate"));
        }
    }
}

#[test]
fn mkv_explicit_frame_rate_mixed_batches_and_retry_preserve_successes() {
    let dir = tempfile::tempdir().unwrap();
    let first = make_video(dir.path(), "first.mp4", "30", None);
    let second = copy("apple-vfr-rotated.mov", &dir.path().join("rotated.mov"));
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let missing = dir.path().join("later.mp4");
    let mut request = request(&[first, second, image, missing.clone()], dir.path());
    request.video_options = Some(VideoOptions {
        target: "mkv".into(),
        resolution: Some(480),
        auto_resolution: false,
        frame_rate: Some(24),
        ..size_options(100_000)
    });
    let manager = JobManager::default();
    let job = manager
        .prepare("mkv-batch", request.clone(), false)
        .unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert!(
        before.files[..3]
            .iter()
            .all(|f| f.status == FileStatus::Succeeded),
        "{before:?}"
    );
    for file in &before.files[..2] {
        assert!(file.output_bytes.unwrap() <= 100_000);
        assert_eq!(
            probe(Path::new(file.output_path.as_ref().unwrap()))["streams"][0]["avg_frame_rate"],
            "24/1"
        );
    }
    assert_eq!(before.files[3].status, FileStatus::Failed);
    copy("apple-h264.mp4", &missing);
    let retry = manager.prepare("mkv-batch", request, true).unwrap();
    run_job(&retry, &backend(), &audio_backend(), |_| {});
    let after = retry.snapshot();
    assert!(
        after
            .files
            .iter()
            .all(|f| f.status == FileStatus::Succeeded),
        "{after:?}"
    );
    for (a, b) in before.files[..3].iter().zip(&after.files) {
        assert_eq!(a.output_path, b.output_path);
    }
    no_partials(dir.path());
}

#[test]
fn mkv_automatic_encoder_validates_quality_and_size_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-vfr-rotated.mov", &dir.path().join("rotated.mov"));
    for mut options in [
        settings(false),
        VideoOptions {
            target: "mkv".into(),
            ..size_options(100_000)
        },
    ] {
        options.acceleration = crate::video_backend::Acceleration::Automatic;
        let output = convert(&source, &options);
        if let Ok(required) = std::env::var("RECAST_REQUIRE_HARDWARE") {
            assert!(output.details.unwrap().contains(&required));
            assert!(output.warnings.is_empty());
        }
    }
}

#[test]
fn mkv_retains_srt_subtitles_and_rejects_incompatible_subtitles_per_destination() {
    let dir = tempfile::tempdir().unwrap();
    for (name, codec) in [
        ("video-srt.mkv", "subrip"),
        ("video-ass.mkv", "ass"),
        ("video-webvtt.mkv", "webvtt"),
    ] {
        let srt = copy(name, &dir.path().join(name));
        let inspected =
            crate::inputs::inspect_with_audio(&srt, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        inspected.require_video_target(&settings(true)).unwrap();
        assert!(inspected.require_video_target(&video_options()).is_err());
        for options in [
            settings(true),
            settings(false),
            VideoOptions {
                target: "mkv".into(),
                ..size_options(100_000)
            },
        ] {
            let output = convert(&srt, &options);
            let data = probe(Path::new(&output.path));
            assert_eq!(data["streams"][1]["codec_name"], codec);
            assert_eq!(data["streams"][1]["tags"]["language"], "eng");
            assert_eq!(data["streams"][1]["disposition"]["default"], 1);
        }
    }
    let mov_text = fixture("video-captions.mp4");
    let inspected =
        crate::inputs::inspect_with_audio(&mov_text, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert!(inspected
        .require_video_target(&settings(true))
        .unwrap_err()
        .contains("subtitles"));
    inspected.require_video_target(&video_options()).unwrap();
}

#[test]
fn mkv_cancellation_and_invalid_copy_settings_never_publish_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    for copy_streams in [true, false] {
        for stage in [
            "Checking source",
            if copy_streams {
                "Copying streams"
            } else {
                "Encoding"
            },
            "Checking MKV",
        ] {
            let cancel = AtomicBool::new(false);
            let result = audio_backend().convert_video(
                &source,
                None,
                &settings(copy_streams),
                &cancel,
                |_, phase| {
                    if phase.starts_with(stage) {
                        cancel.store(true, Ordering::Relaxed);
                    }
                },
            );
            assert!(
                matches!(result, Err(ConversionError::Cancelled)),
                "{stage}: {result:?}"
            );
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
    for bad in [
        VideoOptions {
            size_limit: Some(100_000),
            ..settings(true)
        },
        VideoOptions {
            resolution: Some(480),
            ..settings(true)
        },
        VideoOptions {
            frame_rate: Some(24),
            ..settings(true)
        },
        VideoOptions {
            target: "mp4".into(),
            ..settings(true)
        },
    ] {
        assert!(bad.validate().is_err());
    }
    no_partials(dir.path());
}
