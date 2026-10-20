use super::*;
use crate::video_backend::VideoOptions;
use serde_json::Value;
use std::io::Write;

fn video_options() -> VideoOptions {
    VideoOptions {
        target: "mp4".into(),
        quality: 85,
        resolution: None,
        frame_rate: None,
        metadata: true,
    }
}
fn ffmpeg(args: &[&str]) {
    let result = audio_backend()
        .command("ffmpeg")
        .args([
            "-hide_banner",
            "-v",
            "error",
            "-nostdin",
            "-y",
            "-threads",
            "2",
            "-filter_threads",
            "1",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
fn probe(path: &Path) -> Value {
    let result = audio_backend()
        .command("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-show_chapters",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(result.status.success());
    serde_json::from_slice(&result.stdout).unwrap()
}
fn make_video(dir: &Path, name: &str, rate: &str, audio: Option<&str>) -> PathBuf {
    let raw = dir.join("frames.rgb");
    let mut file = fs::File::create(&raw).unwrap();
    for f in 0..60u32 {
        for y in 0..48u32 {
            for x in 0..64u32 {
                file.write_all(&[
                    ((x * 7 + f * 13) % 256) as u8,
                    ((y * 7 + f * 9) % 256) as u8,
                    ((x * y + f * 17) % 256) as u8,
                ])
                .unwrap();
            }
        }
    }
    drop(file);
    let source = dir.join(name);
    let mut args = vec![
        "-f",
        "rawvideo",
        "-pixel_format",
        "rgb24",
        "-video_size",
        "64x48",
        "-framerate",
        rate,
        "-i",
        raw.to_str().unwrap(),
    ];
    let audio_file = fixture("apple-aac-lc.m4a");
    if audio.is_some() {
        args.extend([
            "-i",
            audio_file.to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
        ]);
    }
    args.extend(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-threads:v", "2"]);
    if let Some(codec) = audio {
        args.extend(["-c:a", codec]);
    }
    args.extend([
        "-metadata",
        "title=Recast video fixture",
        "-movflags",
        "+faststart",
        source.to_str().unwrap(),
    ]);
    ffmpeg(&args);
    source.canonicalize().unwrap()
}
fn convert(source: &Path, options: &VideoOptions) -> crate::image_backend::OutputFile {
    audio_backend()
        .convert_video(source, None, options, &AtomicBool::new(false), |_, _| {})
        .unwrap()
}

#[test]
fn video_mp4_preserves_source_silent_frames_and_same_format_names() {
    audio_backend().verify().unwrap();
    let dir = tempfile::tempdir().unwrap();
    for rate in ["30", "30000/1001"] {
        let source = make_video(dir.path(), "source.mp4", rate, None);
        let bytes = fs::read(&source).unwrap();
        let inspected =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(inspected.kind, "video");
        inspected.require_video_target(&video_options()).unwrap();
        let output = convert(&source, &video_options());
        assert_ne!(Path::new(&output.path), source);
        assert_eq!(fs::read(&source).unwrap(), bytes);
        let data = probe(Path::new(&output.path));
        assert_eq!(data["streams"][0]["codec_name"], "h264");
        assert_eq!(data["streams"][0]["nb_frames"], "60");
        assert_eq!(data["format"]["tags"]["title"], "Recast video fixture");
        assert_eq!(data["streams"].as_array().unwrap().len(), 1);
    }
    no_partials(dir.path());
}

#[test]
fn video_keeps_aac_tracks_and_encodes_other_audio_without_rate_or_channel_changes() {
    let dir = tempfile::tempdir().unwrap();
    for codec in ["copy", "alac"] {
        let source = make_video(dir.path(), "audio.mp4", "30", Some(codec));
        let output = convert(&source, &video_options());
        let data = probe(Path::new(&output.path));
        assert_eq!(data["streams"][1]["codec_name"], "aac");
        assert_eq!(data["streams"][1]["sample_rate"], "48000");
        assert_eq!(data["streams"][1]["channels"], 2);
        assert_eq!(output.warnings.is_empty(), codec == "copy");
    }
}

#[test]
fn video_explicit_frame_rate_and_metadata_off_work_without_upscaling() {
    let dir = tempfile::tempdir().unwrap();
    let source = make_video(dir.path(), "source.mp4", "30", None);
    let mut options = video_options();
    options.resolution = Some(480);
    options.frame_rate = Some(24);
    options.metadata = false;
    let output = convert(&source, &options);
    let data = probe(Path::new(&output.path));
    assert_eq!(data["streams"][0]["nb_frames"], "48");
    assert_eq!(data["streams"][0]["width"], 64);
    assert_eq!(data["streams"][0]["height"], 48);
    assert!(data["format"]["tags"]["title"].is_null());
}

#[test]
fn video_independent_apple_inputs_preserve_variable_timing_and_apply_rotation() {
    let dir = tempfile::tempdir().unwrap();
    for (name, width, height, frames) in [
        ("apple-h264.mp4", 64, 48, "60"),
        ("apple-vfr-rotated.mov", 48, 64, "9"),
    ] {
        let source = copy(name, &dir.path().join("misnamed.wav"));
        let output = convert(&source, &video_options());
        let timestamps = |path: &Path| {
            let result = audio_backend()
                .command("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_frames",
                    "-show_entries",
                    "frame=best_effort_timestamp_time,duration_time",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .unwrap();
            let frames: Value = serde_json::from_slice(&result.stdout).unwrap();
            frames["frames"]
                .as_array()
                .unwrap()
                .iter()
                .map(|f| {
                    f["best_effort_timestamp_time"]
                        .as_str()
                        .unwrap()
                        .parse::<f64>()
                        .unwrap()
                })
                .collect::<Vec<_>>()
        };
        let original = timestamps(&source);
        let actual = timestamps(Path::new(&output.path));
        assert_eq!(original.len(), actual.len());
        for (a, b) in original.iter().zip(actual) {
            assert!((a - b).abs() < 0.000002, "timestamp changed: {a} -> {b}");
        }
        if let Some(folder) = std::env::var_os("RECAST_VIDEO_EVIDENCE") {
            fs::create_dir_all(&folder).unwrap();
            fs::copy(
                &output.path,
                PathBuf::from(folder).join(format!("{name}.mp4")),
            )
            .unwrap();
        }
        let p = probe(Path::new(&output.path));
        let original_duration = probe(&source)["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        let actual_duration = p["format"]["duration"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap();
        assert!(
            (original_duration - actual_duration).abs() < 0.00001,
            "Movie duration changed: {original_duration} -> {actual_duration}"
        );
        assert_eq!(p["streams"][0]["width"], width);
        assert_eq!(p["streams"][0]["height"], height);
        assert_eq!(p["streams"][0]["nb_frames"], frames);
        assert!(p["streams"][0]["side_data_list"]
            .as_array()
            .is_none_or(|s| s
                .iter()
                .all(|s| s["rotation"].is_null() || s["rotation"] == 0)));
    }
}

#[test]
fn video_multiple_audio_tracks_languages_and_offsets_are_retained() {
    let dir = tempfile::tempdir().unwrap();
    let silent = make_video(dir.path(), "silent.mp4", "30", None);
    let source = dir.path().join("two.mp4");
    let audio = fixture("apple-aac-lc.m4a");
    ffmpeg(&[
        "-i",
        silent.to_str().unwrap(),
        "-i",
        audio.to_str().unwrap(),
        "-itsoffset",
        "0.25",
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
    let output = convert(&source, &video_options());
    let p = probe(Path::new(&output.path));
    assert_eq!(p["streams"].as_array().unwrap().len(), 3);
    assert_eq!(p["streams"][1]["tags"]["language"], "eng");
    assert_eq!(p["streams"][2]["tags"]["language"], "fra");
    assert_eq!(p["streams"][1]["disposition"]["default"], 1);
    assert_eq!(p["streams"][2]["disposition"]["default"], 0);
    assert_eq!(
        p["streams"][2]["start_time"],
        probe(&source)["streams"][2]["start_time"]
    );
    // A stream copy retains the encoded packets, including trim information.
    for index in [0, 1] {
        let digest = |file: &Path| {
            let output = audio_backend()
                .command("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    &format!("a:{index}"),
                    "-show_packets",
                    "-show_data_hash",
                    "sha256",
                    "-show_entries",
                    "packet=pts_time,duration_time,data_hash",
                    "-of",
                    "json",
                ])
                .arg(file)
                .output()
                .unwrap();
            serde_json::from_slice::<Value>(&output.stdout).unwrap()
        };
        assert_eq!(digest(&source), digest(Path::new(&output.path)));
    }
}

#[test]
fn video_rejects_unsupported_tracks_hdr_and_corrupt_sources_without_publishing() {
    let dir = tempfile::tempdir().unwrap();
    let silent = make_video(dir.path(), "source.mp4", "30", None);
    let multiple = dir.path().join("multiple.mp4");
    ffmpeg(&[
        "-i",
        silent.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "0:v",
        "-c",
        "copy",
        multiple.to_str().unwrap(),
    ]);
    let hdr = dir.path().join("hdr.mp4");
    ffmpeg(&[
        "-i",
        silent.to_str().unwrap(),
        "-c:v",
        "libx264",
        "-threads:v",
        "2",
        "-pix_fmt",
        "yuv420p10le",
        "-color_trc",
        "smpte2084",
        "-color_primaries",
        "bt2020",
        hdr.to_str().unwrap(),
    ]);
    for (path, issue) in [(&multiple, "extra video"), (&hdr, "8-bit SDR")] {
        let inspected =
            crate::inputs::inspect_with_audio(path, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert!(inspected
            .require_video_target(&video_options())
            .unwrap_err()
            .contains(issue));
        assert!(audio_backend()
            .convert_video(
                path,
                None,
                &video_options(),
                &AtomicBool::new(false),
                |_, _| {}
            )
            .is_err());
    }
    let damaged = dir.path().join("damaged.mp4");
    let mut bytes = fs::read(silent).unwrap();
    bytes.truncate(bytes.len() / 2);
    fs::write(&damaged, bytes).unwrap();
    assert!(audio_backend()
        .convert_video(
            &damaged,
            None,
            &video_options(),
            &AtomicBool::new(false),
            |_, _| {}
        )
        .is_err());
    assert!(!dir.path().join("damaged (1).mp4").exists());
    no_partials(dir.path());
}

#[test]
fn video_resize_quality_and_mixed_batch_retry_keep_successes() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("元 video.mp4"));
    let large = dir.path().join("large.mp4");
    ffmpeg(&[
        "-i",
        source.to_str().unwrap(),
        "-vf",
        "scale=1280:960",
        "-c:v",
        "libx264",
        "-threads:v",
        "2",
        large.to_str().unwrap(),
    ]);
    let mut settings = video_options();
    settings.resolution = Some(480);
    settings.quality = 100;
    let high = convert(&large, &settings);
    let p = probe(Path::new(&high.path));
    assert_eq!(p["streams"][0]["width"], 640);
    assert_eq!(p["streams"][0]["height"], 480);
    settings.quality = 1;
    let small = convert(&large, &settings);
    assert!(small.bytes < high.bytes);
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let audio = copy("apple-aac-lc.m4a", &dir.path().join("sound.m4a"));
    let missing = dir.path().join("retry.mp4");
    let mut request = request(&[source, image, audio, missing.clone()], dir.path());
    request.video_options = Some(video_options());
    request.audio_options = Some(super::audio::audio_options("flac"));
    let manager = JobManager::default();
    let job = manager.prepare("mixed", request.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert!(
        before.files[..3]
            .iter()
            .all(|f| f.status == FileStatus::Succeeded),
        "{before:?}"
    );
    assert_eq!(before.files[3].status, FileStatus::Failed);
    copy("apple-h264.mp4", &missing);
    let retry = manager.prepare("mixed", request, true).unwrap();
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
fn video_cancel_during_encode_and_validation_reaps_worker_and_cleans_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    let long = dir.path().join("long.mp4");
    ffmpeg(&[
        "-stream_loop",
        "99",
        "-i",
        source.to_str().unwrap(),
        "-c",
        "copy",
        long.to_str().unwrap(),
    ]);
    for phase in ["Encoding MP4", "Checking MP4"] {
        let cancelled = AtomicBool::new(false);
        let mut reached = false;
        let result = thread::scope(|scope| {
            audio_backend().convert_video(
                &long,
                None,
                &video_options(),
                &cancelled,
                |_, current| {
                    if current == phase && !reached {
                        reached = true;
                        let signal = &cancelled;
                        scope.spawn(move || {
                            thread::sleep(Duration::from_millis(25));
                            signal.store(true, Ordering::Relaxed);
                        });
                    }
                },
            )
        });
        assert!(reached, "{phase} was not exercised: {result:?}");
        assert!(
            matches!(result, Err(ConversionError::Cancelled)),
            "{result:?}"
        );
        assert!(!dir.path().join("long (1).mp4").exists());
        no_partials(dir.path());
    }
}

#[test]
fn video_webm_opus_transcodes_and_mp4_subtitles_are_retained() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["video-vp9-opus.webm", "video-captions.mp4"] {
        let source = copy(name, &dir.path().join(name));
        let output = convert(&source, &video_options());
        let p = probe(Path::new(&output.path));
        assert_eq!(p["streams"][0]["codec_name"], "h264");
        if name.ends_with("webm") {
            assert_eq!(p["streams"][1]["codec_name"], "aac");
            assert_eq!(p["streams"][1]["sample_rate"], "48000");
            assert_eq!(p["streams"][1]["channels"], 2);
        } else {
            assert_eq!(p["streams"][1]["codec_name"], "mov_text");
            assert_eq!(p["streams"][1]["tags"]["language"], "eng");
            let captions = |path: &Path| {
                let o = audio_backend()
                    .command("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-select_streams",
                        "s:0",
                        "-show_packets",
                        "-show_data_hash",
                        "sha256",
                        "-show_entries",
                        "packet=pts_time,duration_time,data_hash",
                        "-of",
                        "json",
                    ])
                    .arg(path)
                    .output()
                    .unwrap();
                serde_json::from_slice::<Value>(&o.stdout).unwrap()
            };
            assert_eq!(captions(&source), captions(Path::new(&output.path)));
        }
    }
    let input = crate::inputs::inspect_with_audio(
        &fixture("video-srt.mkv"),
        &audio_backend(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(input
        .require_video_target(&video_options())
        .unwrap_err()
        .contains("subtitle"));
}
