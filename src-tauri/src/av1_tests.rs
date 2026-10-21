use super::*;

fn save_evidence(output: &crate::image_backend::OutputFile, name: &str) {
    if let Some(folder) = std::env::var_os("RECAST_AV1_EVIDENCE") {
        fs::create_dir_all(&folder).unwrap();
        fs::copy(&output.path, Path::new(&folder).join(name)).unwrap();
    }
}

#[test]
fn av1_cfr_vfr_and_opus_convert_to_every_video_destination() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "video-av1.mp4",
        "video-av1-vfr.mkv",
        "video-av1-opus.webm",
        "video-av1-24fps.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        let before = fs::read(&source).unwrap();
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(input.kind, "video");
        let input = serde_json::to_value(input).unwrap();
        assert!(input["conversionIssue"].is_null());
        assert_eq!(
            input["targets"],
            serde_json::json!(["mp4", "mkv", "mov", "webm"])
        );
        let info = audio_backend()
            .inspect_video(&source, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(info.codec, "av1");
        assert!(info.issue.is_none(), "{:?}", info.issue);
        for target in ["mp4", "mkv", "mov", "webm"] {
            assert!(info.target_issue(target).is_none());
            for size in [false, true] {
                let options = VideoOptions {
                    target: target.into(),
                    ..if size {
                        size_options(100_000)
                    } else {
                        video_options()
                    }
                };
                // convert_video checks every frame timestamp and audio endpoint,
                // not just whether the encoder exits successfully.
                let output = convert(&source, &options);
                let actual = probe(Path::new(&output.path));
                assert_eq!(
                    actual["streams"][0]["codec_name"],
                    if target == "webm" { "vp9" } else { "h264" }
                );
                assert_eq!(
                    actual["streams"].as_array().unwrap().len(),
                    1 + info.audio_tracks
                );
                if size {
                    assert!(output.bytes <= 100_000);
                }
                save_evidence(
                    &output,
                    &format!(
                        "{name}-{}.{}",
                        if size { "size" } else { "quality" },
                        target
                    ),
                );
            }
        }
        assert_eq!(fs::read(&source).unwrap(), before);
    }
    no_partials(dir.path());
}

#[test]
fn av1_copy_to_mkv_and_webm_preserves_decoded_content_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "video-av1.mp4",
        "video-av1-vfr.mkv",
        "video-av1-opus.webm",
        "video-av1-24fps.webm",
    ] {
        let source = copy(name, &dir.path().join(name));
        let before = fs::read(&source).unwrap();
        for target in ["mkv", "webm"] {
            let options = VideoOptions {
                target: target.into(),
                copy_streams: true,
                ..video_options()
            };
            let mut phases = Vec::new();
            let output = audio_backend()
                .convert_video(
                    &source,
                    None,
                    &options,
                    &AtomicBool::new(false),
                    |_, phase| phases.push(phase.to_string()),
                )
                .unwrap();
            assert_eq!(
                probe(Path::new(&output.path))["streams"][0]["codec_name"],
                "av1"
            );
            assert!(phases.iter().any(|phase| phase == "Copying streams"));
            assert!(!phases.iter().any(|phase| phase.contains("Encoding")));
            assert!(output.details.as_ref().unwrap().contains("Copied streams"));
            let repeated = convert(Path::new(&output.path), &options);
            assert_ne!(output.path, repeated.path);
            save_evidence(&output, &format!("{name}-copy.{target}"));
        }
        assert_eq!(fs::read(&source).unwrap(), before);
    }
    no_partials(dir.path());
}

#[test]
fn av1_high_bit_depth_is_explained_and_never_silently_reduced() {
    let source = fixture("video-av1-10bit.mkv");
    let info = audio_backend()
        .inspect_video(&source, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(info.codec, "av1");
    assert!(info.issue.as_ref().unwrap().contains("8-bit SDR"));
    for target in ["mp4", "mkv", "mov", "webm"] {
        assert!(info
            .target_issue(target)
            .unwrap()
            .contains("higher bit depth"));
        let dir = tempfile::tempdir().unwrap();
        let result = audio_backend().convert_video(
            &source,
            Some(dir.path()),
            &VideoOptions {
                target: target.into(),
                ..video_options()
            },
            &AtomicBool::new(false),
            |_, _| {},
        );
        assert!(result.is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

#[test]
#[ignore = "manual full-file check; requires RECAST_AV1_SOURCE and RECAST_AV1_EVIDENCE"]
fn av1_real_file_target_size() {
    let source = PathBuf::from(std::env::var_os("RECAST_AV1_SOURCE").expect("source path"));
    let folder = PathBuf::from(std::env::var_os("RECAST_AV1_EVIDENCE").expect("output folder"));
    fs::create_dir_all(&folder).unwrap();
    let before = fs::metadata(&source).unwrap();
    let mut last_phase = String::new();
    let mut last_percent = 0;
    let output = audio_backend()
        .convert_video(
            &source,
            Some(&folder),
            &VideoOptions {
                target: "webm".into(),
                ..size_options(10_000_000)
            },
            &AtomicBool::new(false),
            |fraction, phase| {
                let percent = (fraction * 100.0) as u32;
                if phase != last_phase || percent >= last_percent + 10 {
                    eprintln!("{percent}% {phase}");
                    last_phase = phase.into();
                    last_percent = percent;
                }
            },
        )
        .unwrap();
    assert!(output.bytes <= 10_000_000);
    let after = fs::metadata(&source).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    fs::write(
        folder.join("result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "path": output.path, "bytes": output.bytes, "details": output.details,
            "probe": probe(Path::new(&output.path))
        }))
        .unwrap(),
    )
    .unwrap();
    no_partials(&folder);
}

#[test]
fn av1_declared_end_outside_timestamp_precision_still_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = fs::read(fixture("video-av1-24fps.webm")).unwrap();
    let tag = b"00:00:02.000000000";
    let positions: Vec<_> = data
        .windows(tag.len())
        .enumerate()
        .filter_map(|(i, bytes)| (bytes == tag).then_some(i))
        .collect();
    assert_eq!(positions.len(), 1);
    let start = positions[0];
    data[start..start + tag.len()].copy_from_slice(b"00:00:02.010000000");
    let source = dir.path().join("inconsistent.webm");
    fs::write(&source, &data).unwrap();
    let result = audio_backend().convert_video(
        &source,
        Some(dir.path()),
        &video_options(),
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(
        matches!(result, Err(crate::image_backend::ConversionError::Failed(message))
        if message.contains("declared duration disagrees"))
    );
    assert_eq!(fs::read(&source).unwrap(), data);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
