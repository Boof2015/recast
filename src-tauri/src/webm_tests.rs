use super::*;

fn settings(copy_streams: bool) -> VideoOptions {
    VideoOptions {
        target: "webm".into(),
        copy_streams,
        ..video_options()
    }
}

#[test]
fn webm_copies_independent_vp8_vp9_opus_and_vorbis_without_reencoding() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["video-vp8-vorbis.webm", "video-vp9-opus.webm"] {
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
        assert!(output.path.ends_with(".webm"));
        assert!(output.details.unwrap().contains("Copied streams"));
        assert!(phases.iter().any(|phase| phase == "Copying streams"));
        assert!(!phases
            .iter()
            .any(|phase| phase.contains("Encoding") || phase.contains("hardware")));
        assert_eq!(fs::read(&source).unwrap(), original);
        let repeated = convert(Path::new(&output.path), &settings(true));
        assert_ne!(output.path, repeated.path);
        if let Some(evidence) = std::env::var_os("RECAST_WEBM_EVIDENCE") {
            fs::create_dir_all(&evidence).unwrap();
            fs::copy(
                &output.path,
                Path::new(&evidence).join(format!("{name}.webm")),
            )
            .unwrap();
        }
    }
    no_partials(dir.path());
}

#[test]
fn webm_quality_and_size_keep_complete_video_audio_and_rotation() {
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
                    target: "webm".into(),
                    ..size_options(100_000)
                }
            } else {
                settings(false)
            };
            let output = convert(&source, &options);
            let data = probe(Path::new(&output.path));
            assert_eq!(data["streams"][0]["codec_name"], "vp9");
            if name.contains("opus") {
                assert_eq!(data["streams"][1]["codec_name"], "opus");
            }
            if size {
                assert!(output.bytes <= 100_000);
            }
            if let Some(evidence) = std::env::var_os("RECAST_WEBM_EVIDENCE") {
                fs::create_dir_all(&evidence).unwrap();
                fs::copy(
                    &output.path,
                    Path::new(&evidence).join(format!(
                        "{name}-{}.webm",
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
fn webm_retains_multiple_aac_tracks_offsets_metadata_and_exact_playable_samples() {
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
            settings(false),
            VideoOptions {
                target: "webm".into(),
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
                    if let Some(evidence) = std::env::var_os("RECAST_WEBM_EVIDENCE") {
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
                            Path::new(&evidence).join(format!("offset-{offset}-{mode}.webm")),
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
fn webm_explicit_frame_rate_mixed_batches_and_retry_preserve_successes() {
    let dir = tempfile::tempdir().unwrap();
    let first = make_video(dir.path(), "first.mp4", "30", None);
    let second = copy("apple-vfr-rotated.mov", &dir.path().join("rotated.mov"));
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let missing = dir.path().join("later.mp4");
    let mut request = request(&[first, second, image, missing.clone()], dir.path());
    request.video_options = Some(VideoOptions {
        target: "webm".into(),
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
fn webm_automatic_encoder_validates_quality_and_size_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-vfr-rotated.mov", &dir.path().join("rotated.mov"));
    for mut options in [
        settings(false),
        VideoOptions {
            target: "webm".into(),
            ..size_options(100_000)
        },
    ] {
        options.acceleration = crate::video_backend::Acceleration::Automatic;
        let output = convert(&source, &options);
        if let Ok(required) = std::env::var("RECAST_REQUIRE_VP9_HARDWARE") {
            assert!(output.details.unwrap().contains(&required));
            assert!(output.warnings.is_empty());
        }
    }
}

#[test]
fn webm_cancellation_and_invalid_copy_settings_never_publish_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("video-vp9-opus.webm", &dir.path().join("source.webm"));
    for copy_streams in [true, false] {
        for stage in [
            "Checking source",
            if copy_streams {
                "Copying streams"
            } else {
                "Encoding"
            },
            "Checking WEBM",
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

fn save_evidence(source: &Path, name: &str) {
    if let Some(folder) = std::env::var_os("RECAST_WEBM_EVIDENCE") {
        fs::create_dir_all(&folder).unwrap();
        fs::copy(source, Path::new(&folder).join(name)).unwrap();
    }
}

fn pcm_video(
    dir: &Path,
    rate: u32,
    layout: &str,
    channels: u32,
    samples: u32,
    offset: &str,
) -> PathBuf {
    let raw = dir.join("audio.f64");
    let mut file = fs::File::create(&raw).unwrap();
    for sample in 0..samples {
        for channel in 0..channels {
            let value = 0.2
                * (f64::from(sample)
                    * (150.0 + f64::from(channel) * 110.0)
                    * std::f64::consts::TAU
                    / f64::from(rate))
                .sin();
            file.write_all(&value.to_le_bytes()).unwrap();
        }
    }
    drop(file);
    let source = dir.join(format!("pcm-{rate}-{layout}-{samples}-{offset}.mkv"));
    ffmpeg(&[
        "-i",
        fixture("apple-h264.mp4").to_str().unwrap(),
        "-itsoffset",
        offset,
        "-f",
        "f64le",
        "-ar",
        &rate.to_string(),
        "-ac",
        &channels.to_string(),
        "-channel_layout",
        layout,
        "-i",
        raw.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "copy",
        "-c:a",
        if channels > 2 { "flac" } else { "pcm_f64le" },
        source.to_str().unwrap(),
    ]);
    source
}

#[test]
fn webm_opus_resamples_without_mixing_channels_or_losing_the_tail() {
    let dir = tempfile::tempdir().unwrap();
    for (rate, layout, channels, samples, offset) in [
        (7350, "mono", 1, 7471, "0.005"),
        (8000, "stereo", 2, 8117, "2.25"),
        (44100, "stereo", 2, 44217, "0.25"),
        (48000, "3.0", 3, 48117, "0"),
        (48000, "quad", 4, 48117, "0"),
        (48000, "5.0", 5, 48117, "0"),
        (48000, "5.1", 6, 48117, "0"),
        (48000, "6.1", 7, 48117, "0"),
        (48000, "7.1", 8, 48117, "0"),
        (192000, "stereo", 2, 192117, "0.005"),
        (48000, "mono", 1, 117, "0.25"),
        (48000, "mono", 1, 1, "0"),
    ] {
        let source = pcm_video(dir.path(), rate, layout, channels, samples, offset);
        for options in [
            settings(false),
            VideoOptions {
                target: "webm".into(),
                ..size_options(300_000)
            },
        ] {
            let output = audio_backend()
                .convert_video(&source, None, &options, &AtomicBool::new(false), |_, _| {})
                .unwrap_or_else(|error| panic!("{rate}/{layout}/{samples}/{offset}: {error:?}"));
            let data = probe(Path::new(&output.path));
            assert_eq!(data["streams"][1]["codec_name"], "opus");
            assert_eq!(data["streams"][1]["sample_rate"], "48000");
            assert_eq!(data["streams"][1]["channel_layout"], layout);
            let mode = if options.size_limit.is_some() {
                "size"
            } else {
                "quality"
            };
            save_evidence(&source, source.file_name().unwrap().to_str().unwrap());
            save_evidence(
                Path::new(&output.path),
                &format!("pcm-{rate}-{layout}-{samples}-{offset}-{mode}.webm"),
            );
        }
    }
    no_partials(dir.path());
}

#[test]
fn webm_retains_webvtt_and_audio_when_streams_are_reordered() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("reordered.mkv");
    ffmpeg(&[
        "-i",
        fixture("video-webvtt.mkv").to_str().unwrap(),
        "-i",
        fixture("video-vp9-opus.webm").to_str().unwrap(),
        "-map",
        "0:s",
        "-map",
        "1:a",
        "-map",
        "1:v",
        "-c",
        "copy",
        source.to_str().unwrap(),
    ]);
    save_evidence(&source, "webvtt-source.mkv");
    for options in [
        settings(true),
        settings(false),
        VideoOptions {
            target: "webm".into(),
            ..size_options(100_000)
        },
    ] {
        let mode = if options.copy_streams {
            "copy"
        } else if options.size_limit.is_some() {
            "size"
        } else {
            "quality"
        };
        let output = convert(
            &source,
            &VideoOptions {
                metadata: false,
                ..options
            },
        );
        let data = probe(Path::new(&output.path));
        assert_eq!(data["streams"].as_array().unwrap().len(), 3);
        let subtitle = data["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["codec_type"] == "subtitle")
            .unwrap();
        assert_eq!(subtitle["codec_name"], "webvtt");
        assert_eq!(subtitle["tags"]["language"], "eng");
        save_evidence(Path::new(&output.path), &format!("webvtt-{mode}.webm"));
    }
}

#[test]
fn webm_compatibility_is_specific_to_destination_and_mode() {
    let backend = audio_backend();
    let cancel = AtomicBool::new(false);
    let input =
        crate::inputs::inspect_with_audio(&fixture("apple-h264.mp4"), &backend, &cancel).unwrap();
    assert!(serde_json::to_value(&input).unwrap()["targets"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("webm")));
    assert!(input
        .require_video_target(&settings(true))
        .unwrap_err()
        .contains("VP9"));
    input.require_video_target(&settings(false)).unwrap();
    for name in ["video-captions.mp4", "video-srt.mkv", "video-ass.mkv"] {
        let input = crate::inputs::inspect_with_audio(&fixture(name), &backend, &cancel).unwrap();
        assert!(!serde_json::to_value(&input).unwrap()["targets"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("webm")));
        assert!(input
            .require_video_target(&settings(false))
            .unwrap_err()
            .contains("subtitles"));
    }
    let dir = tempfile::tempdir().unwrap();
    let incompatible = pcm_video(dir.path(), 48000, "4.0", 4, 4800, "0");
    let input = crate::inputs::inspect_with_audio(&incompatible, &backend, &cancel).unwrap();
    assert!(input
        .require_video_target(&settings(false))
        .unwrap_err()
        .contains("channel layout"));
    assert!(serde_json::to_value(&input).unwrap()["targets"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("mkv")));
    let source = dir.path().join("vp9-aac.mkv");
    ffmpeg(&[
        "-i",
        fixture("video-vp9-opus.webm").to_str().unwrap(),
        "-c:v",
        "copy",
        "-c:a",
        "aac",
        source.to_str().unwrap(),
    ]);
    let input = crate::inputs::inspect_with_audio(&source, &backend, &cancel).unwrap();
    assert!(input
        .require_video_target(&settings(true))
        .unwrap_err()
        .contains("Opus"));
    input.require_video_target(&settings(false)).unwrap();
    convert(&source, &settings(false));
}

#[test]
fn webm_quality_extremes_and_preserved_vorbis_work_in_the_same_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("video-vp8-vorbis.webm", &dir.path().join("vp8.webm"));
    for quality in [1, 100] {
        let output = convert(
            &source,
            &VideoOptions {
                quality,
                ..settings(false)
            },
        );
        let data = probe(Path::new(&output.path));
        assert_eq!(data["streams"][0]["codec_name"], "vp9");
        assert_eq!(data["streams"][1]["codec_name"], "vorbis");
        assert_eq!(
            data["streams"][1]["sample_rate"],
            probe(&source)["streams"][1]["sample_rate"]
        );
        save_evidence(
            Path::new(&output.path),
            &format!("vp8-vorbis-quality-{quality}.webm"),
        );
    }
    let output = convert(
        &source,
        &VideoOptions {
            target: "webm".into(),
            ..size_options(100_000)
        },
    );
    assert!(output.bytes <= 100_000);
    assert_eq!(
        probe(Path::new(&output.path))["streams"][1]["codec_name"],
        "opus"
    );
}
