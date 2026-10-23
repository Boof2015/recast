use super::*;
use crate::video_backend::VideoOptions;
use serde_json::Value;
use std::io::Write;

#[path = "av1_tests.rs"]
mod av1;
#[path = "audio_extract_tests.rs"]
mod extraction;
#[path = "video_frame_tests.rs"]
mod frames;
#[path = "video_animation_tests.rs"]
mod gif;
#[path = "mkv_tests.rs"]
mod mkv;
#[path = "mov_tests.rs"]
mod mov;
#[path = "webm_tests.rs"]
mod webm;

fn video_options() -> VideoOptions {
    VideoOptions {
        target: "mp4".into(),
        quality: 85,
        resolution: None,
        frame_rate: None,
        metadata: true,
        acceleration: crate::video_backend::Acceleration::Software,
        size_limit: None,
        auto_resolution: false,
        copy_streams: false,
    }
}

#[test]
#[ignore = "manual recording check; set RECAST_VIDEO_SOURCE, RECAST_VIDEO_OUTPUT, and optionally RECAST_VIDEO_OPTIONS JSON"]
fn video_real_file_check() {
    use sha2::{Digest, Sha256};

    let source = PathBuf::from(std::env::var_os("RECAST_VIDEO_SOURCE").expect("source path"));
    let folder = PathBuf::from(std::env::var_os("RECAST_VIDEO_OUTPUT").expect("output folder"));
    let options = std::env::var("RECAST_VIDEO_OPTIONS")
        .map(|json| serde_json::from_str::<VideoOptions>(&json).expect("video options JSON"))
        .unwrap_or_else(|_| video_options());
    fs::create_dir_all(&folder).unwrap();
    let before = Sha256::digest(fs::read(&source).unwrap());
    let mut phases = Vec::new();
    let started = Instant::now();
    let result = audio_backend().convert_video(
        &source,
        Some(&folder),
        &options,
        &AtomicBool::new(false),
        |_, phase| {
            if phases.last().is_none_or(|previous| previous != phase) {
                eprintln!("{:.1}s {phase}", started.elapsed().as_secs_f64());
                phases.push(phase.to_string());
            }
        },
    );
    let report = match &result {
        Ok(output) => serde_json::json!({
            "ok": true, "path": output.path, "bytes": output.bytes,
            "details": output.details, "warnings": output.warnings,
            "probe": probe(Path::new(&output.path)),
        }),
        Err(error) => serde_json::json!({ "ok": false, "error": format!("{error:?}") }),
    };
    fs::write(
        folder.join("result.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "result": report, "phases": phases, "seconds": started.elapsed().as_secs_f64(),
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(Sha256::digest(fs::read(&source).unwrap()), before);
    no_partials(&folder);
    let output = result.expect("recording conversion failed; see result.json");
    if let Ok(required) = std::env::var("RECAST_REQUIRE_HARDWARE") {
        assert!(output.details.as_deref().unwrap_or("").contains(&required));
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

fn hardware_video(source: &Path, destination: &Path) -> PathBuf {
    // The tiny independent fixtures are below NVENC's minimum dimensions.
    // Enlarge their pictures while retaining VFR timestamps, rotation and tracks.
    let mut args = vec![
        "-noautorotate",
        "-i",
        source.to_str().unwrap(),
        "-map",
        "0",
        "-c",
        "copy",
        "-vf",
        "scale=iw*16:ih*16,setsar=1",
        "-pix_fmt",
        "yuv420p",
        "-threads:v",
        "2",
        "-enc_time_base:v",
        "demux",
        "-fps_mode",
        "passthrough",
    ];
    if destination
        .extension()
        .is_some_and(|extension| extension == "webm")
    {
        args.extend([
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "good",
            "-cpu-used",
            "8",
            "-lag-in-frames",
            "0",
        ]);
    } else {
        args.extend(["-c:v", "libx264", "-bf", "0"]);
    }
    args.push(destination.to_str().unwrap());
    ffmpeg(&args);
    let info = audio_backend()
        .inspect_video(destination, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert!(info.width >= 320 && info.height >= 192);
    let original = audio_backend()
        .inspect_video(source, &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(info.rotation, original.rotation);
    assert_eq!(info.audio_tracks, original.audio_tracks);
    assert_eq!(info.subtitle_tracks, original.subtitle_tracks);
    destination.canonicalize().unwrap()
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

fn decoded_audio(path: &Path, track: usize) -> Vec<u8> {
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            &format!("0:a:{track}"),
            "-c:a",
            "pcm_f64le",
            "-f",
            "f64le",
            "-",
        ])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(!result.stdout.is_empty());
    result.stdout
}

#[test]
fn mp4_restores_matroska_aac_clocks_without_trimming_samples() {
    let dir = tempfile::tempdir().unwrap();
    for (rate, offset) in [("48000", "0"), ("44100", "0.25"), ("8000", "2.25")] {
        let source = dir.path().join(format!("recording-{rate}-{offset}.mkv"));
        ffmpeg(&[
            "-i",
            fixture("apple-h264.mp4").to_str().unwrap(),
            "-i",
            fixture("apple-aac-lc.m4a").to_str().unwrap(),
            "-itsoffset",
            offset,
            "-i",
            fixture("apple-aac-lc.m4a").to_str().unwrap(),
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:a",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-ar:a:0",
            "48000",
            "-ar:a:1",
            rate,
            source.to_str().unwrap(),
        ]);
        let before = fs::read(&source).unwrap();
        for metadata in [true, false] {
            let output = convert(
                &source,
                &VideoOptions {
                    metadata,
                    ..video_options()
                },
            );
            let data = probe(Path::new(&output.path));
            assert_eq!(data["streams"][1]["sample_rate"], "48000");
            assert_eq!(data["streams"][2]["sample_rate"], rate);
            for track in 0..2 {
                assert_eq!(
                    decoded_audio(&source, track),
                    decoded_audio(Path::new(&output.path), track),
                    "AAC samples changed at {rate} Hz, offset {offset}, track {track}"
                );
            }
        }
        assert_eq!(fs::read(&source).unwrap(), before);
    }
    no_partials(dir.path());
}

#[test]
fn mp4_refuses_to_collapse_gaps_in_copied_matroska_aac() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("gapped.mkv");
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
        "aac",
        "-bsf:a",
        "setts=ts=TS+gte(N\\,10)*0.1/TB",
        source.to_str().unwrap(),
    ]);
    let result = audio_backend().convert_video(
        &source,
        None,
        &video_options(),
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(
        matches!(result, Err(ConversionError::Failed(ref error))
            if error.contains("timing gaps") && error.contains("MP4")),
        "{result:?}"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

fn size_options(limit: u64) -> VideoOptions {
    VideoOptions {
        size_limit: Some(limit),
        auto_resolution: true,
        // Hidden Quality-mode values must not affect target-size encoding.
        quality: 0,
        ..video_options()
    }
}

#[test]
fn video_size_software_keeps_complete_cfr_vfr_rotation_audio_and_subtitles() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-vp9-opus.webm",
        "video-captions.mp4",
    ] {
        let source = copy(name, &dir.path().join(name));
        let before = fs::read(&source).unwrap();
        let settings = size_options(300_000);
        let mut phases = Vec::new();
        let output = audio_backend()
            .convert_video(
                &source,
                None,
                &settings,
                &AtomicBool::new(false),
                |_, phase| {
                    phases.push(phase.to_string());
                },
            )
            .unwrap();
        assert!(output.bytes <= 300_000, "{name}: {output:?}");
        assert_eq!(fs::read(&source).unwrap(), before);
        assert!(phases.iter().any(|p| p.starts_with("Analyzing size")));
        assert!(phases.iter().any(|p| p.contains("pass 2 of 2")));
        assert!(phases.iter().any(|p| p == "Checking MP4"));
        let original = probe(&source);
        let actual = probe(Path::new(&output.path));
        assert_eq!(
            original["streams"].as_array().unwrap().len(),
            actual["streams"].as_array().unwrap().len()
        );
        assert!(output.details.as_ref().unwrap().contains("0.30 MB limit"));
        if let Ok(folder) = std::env::var("RECAST_SIZE_EVIDENCE") {
            fs::create_dir_all(&folder).unwrap();
            fs::copy(&output.path, Path::new(&folder).join(format!("{name}.mp4"))).unwrap();
        }
        no_partials(dir.path());
    }
}

#[test]
fn video_size_auto_resolution_and_fixed_choices_stay_under_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture("apple-h264.mp4");
    let large = dir.path().join("larger.mp4");
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
    let mut settings = size_options(100_000);
    let automatic = convert(&large, &settings);
    let actual = probe(Path::new(&automatic.path));
    assert!(actual["streams"][0]["height"].as_u64().unwrap() < 960);
    assert!(automatic.bytes <= 100_000);
    settings.auto_resolution = false;
    settings.resolution = Some(480);
    settings.frame_rate = Some(24);
    let fixed = convert(&large, &settings);
    let actual = probe(Path::new(&fixed.path));
    assert_eq!(actual["streams"][0]["height"], 480);
    assert_eq!(actual["streams"][0]["avg_frame_rate"], "24/1");
    assert!(fixed.bytes <= 100_000);
    no_partials(dir.path());
}

#[test]
fn video_size_preserves_multiple_audio_tracks_offsets_and_languages() {
    let dir = tempfile::tempdir().unwrap();
    let silent = make_video(dir.path(), "silent.mp4", "30", None);
    let audio = fixture("apple-aac-lc.m4a");
    for offset in ["0.005", "0.25", "2.25"] {
        let source = dir.path().join(format!("two-{offset}.mp4"));
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
        let output = audio_backend()
            .convert_video(
                &source,
                None,
                &size_options(100_000),
                &AtomicBool::new(false),
                |_, phase| {
                    if phase == "Checking MP4" {
                        if let Ok(folder) = std::env::var("RECAST_SIZE_EVIDENCE") {
                            fs::create_dir_all(&folder).unwrap();
                            fs::copy(
                                &source,
                                Path::new(&folder).join(format!("offset-{offset}-source.mp4")),
                            )
                            .unwrap();
                            let output = fs::read_dir(dir.path())
                                .unwrap()
                                .map(Result::unwrap)
                                .map(|p| p.path())
                                .find(|p| {
                                    p.file_name()
                                        .unwrap()
                                        .to_string_lossy()
                                        .starts_with(".recast-")
                                })
                                .unwrap();
                            fs::copy(
                                output,
                                Path::new(&folder).join(format!("offset-{offset}-output.mp4")),
                            )
                            .unwrap();
                        }
                    }
                },
            )
            .unwrap();
        assert!(output.bytes <= 100_000);
        let actual = probe(Path::new(&output.path));
        assert_eq!(actual["streams"].as_array().unwrap().len(), 3);
        assert_eq!(actual["streams"][1]["tags"]["language"], "eng");
        assert_eq!(actual["streams"][2]["tags"]["language"], "fra");
        assert_eq!(actual["streams"][2]["disposition"]["default"], 0);
        assert_eq!(
            actual["streams"][2]["sample_rate"],
            probe(&source)["streams"][2]["sample_rate"]
        );
        no_partials(dir.path());
    }
}

#[test]
fn video_size_cancel_at_each_pass_and_validation_leaves_no_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    for prefix in ["Analyzing size", "Encoding", "Checking MP4"] {
        let cancel = AtomicBool::new(false);
        let result = audio_backend().convert_video(
            &source,
            None,
            &size_options(100_000),
            &cancel,
            |_, phase| {
                if phase.starts_with(prefix) {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        assert!(
            matches!(
                result,
                Err(crate::image_backend::ConversionError::Cancelled)
            ),
            "{prefix}: {result:?}"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn video_size_rejects_impossible_limits_and_rechecks_bytes_before_publication() {
    let mut invalid = size_options(99_999);
    assert!(invalid.validate().is_err());
    invalid.size_limit = Some(u64::MAX);
    assert!(invalid.validate().is_err());
    invalid.size_limit = None;
    assert!(invalid.validate().is_err());
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    let mut appended = false;
    let result = audio_backend().convert_video(
        &source,
        None,
        &size_options(100_000),
        &AtomicBool::new(false),
        |_, phase| {
            if phase == "Checking MP4" && !appended {
                let path = fs::read_dir(dir.path())
                    .unwrap()
                    .map(Result::unwrap)
                    .map(|p| p.path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with(".recast-")
                    })
                    .unwrap();
                // Model output growth after the early size check. The final gate must
                // still refuse publication even if the complete MP4 decodes correctly.
                let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
                f.write_all(&vec![0; 100_001]).unwrap();
                appended = true;
            }
        },
    );
    assert!(appended);
    assert!(result.is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn video_size_automatic_reports_encoder_and_keeps_each_output_under_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = size_options(2_000_000);
    settings.acceleration = crate::video_backend::Acceleration::Automatic;
    let small = make_video(dir.path(), "small.mp4", "30", Some("copy"));
    let source = hardware_video(&small, &dir.path().join("source.mp4"));
    let output = convert(&source, &settings);
    assert!(output.bytes <= 2_000_000);
    let details = output.details.unwrap();
    assert!(details.contains("2.00 MB limit"));
    if let Ok(required) = std::env::var("RECAST_REQUIRE_HARDWARE") {
        assert!(
            details.contains(&required),
            "Expected {required}: {details}"
        );
    }
    no_partials(dir.path());
}

#[test]
fn video_size_is_per_file_and_mixed_batch_retry_retains_successes() {
    let dir = tempfile::tempdir().unwrap();
    let first = make_video(dir.path(), "first.mp4", "30", None);
    let second = make_video(dir.path(), "second.mp4", "60", None);
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let missing = dir.path().join("later.mp4");
    let mut request = request(&[first, second, image, missing.clone()], dir.path());
    request.video_options = Some(size_options(100_000));
    let manager = JobManager::default();
    let job = manager
        .prepare("size-batch", request.clone(), false)
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
    }
    assert_eq!(before.files[3].status, FileStatus::Failed);
    copy("apple-h264.mp4", &missing);
    let retry = manager.prepare("size-batch", request, true).unwrap();
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
    assert!(after.files[3].output_bytes.unwrap() <= 100_000);
    no_partials(dir.path());
}

#[test]
fn video_size_tiny_budget_for_a_long_clip_fails_without_encoding_or_publication() {
    let dir = tempfile::tempdir().unwrap();
    let source = fixture("apple-h264.mp4");
    let long = dir.path().join("long.mp4");
    ffmpeg(&[
        "-stream_loop",
        "59",
        "-i",
        source.to_str().unwrap(),
        "-c",
        "copy",
        long.to_str().unwrap(),
    ]);
    let mut encoded = false;
    let result = audio_backend().convert_video(
        &long,
        None,
        &size_options(100_000),
        &AtomicBool::new(false),
        |_, phase| {
            if phase.starts_with("Encoding") || phase.starts_with("Analyzing size") {
                encoded = true;
            }
        },
    );
    assert!(
        matches!(result, Err(crate::image_backend::ConversionError::Failed(message)) if message.contains("size limit is too small"))
    );
    assert!(!encoded);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
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
        assert!(output.warnings.is_empty());
        assert_eq!(
            output.details.unwrap().contains("encoded to AAC"),
            codec != "copy"
        );
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
    for (mode, phase) in [
        (crate::video_backend::Acceleration::Software, "Encoding ·"),
        (crate::video_backend::Acceleration::Software, "Checking MP4"),
        (crate::video_backend::Acceleration::Automatic, "Encoding ·"),
        (
            crate::video_backend::Acceleration::Automatic,
            "Checking MP4",
        ),
    ] {
        let mut options = video_options();
        options.acceleration = mode;
        let cancelled = AtomicBool::new(false);
        let mut reached = false;
        let result = thread::scope(|scope| {
            audio_backend().convert_video(&long, None, &options, &cancelled, |_, current| {
                if current.starts_with(phase) && !reached {
                    reached = true;
                    let signal = &cancelled;
                    scope.spawn(move || {
                        thread::sleep(Duration::from_millis(25));
                        signal.store(true, Ordering::Relaxed);
                    });
                }
            })
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

#[test]
fn video_automatic_encodes_and_reports_the_actual_engine() {
    let dir = tempfile::tempdir().unwrap();
    let mut options = video_options();
    options.acceleration = crate::video_backend::Acceleration::Automatic;
    for name in [
        "apple-h264.mp4",
        "apple-vfr-rotated.mov",
        "video-captions.mp4",
        "video-vp9-opus.webm",
    ] {
        let source = hardware_video(&fixture(name), &dir.path().join(name));
        let output = convert(&source, &options);
        let details = output.details.as_deref().unwrap();
        eprintln!(
            "Automatic {name}: {details}; warnings: {:?}",
            output.warnings
        );
        if let Some(folder) = std::env::var_os("RECAST_HARDWARE_EVIDENCE") {
            fs::create_dir_all(&folder).unwrap();
            fs::copy(
                &output.path,
                PathBuf::from(folder).join(format!("{name}.mp4")),
            )
            .unwrap();
        }
        if let Ok(required) = std::env::var("RECAST_REQUIRE_HARDWARE") {
            assert!(
                details.contains(&required),
                "Expected hardware {required}: {output:?}"
            );
        }
        assert!(
            details.contains("Software")
                || ["VideoToolbox", "NVENC", "Quick Sync", "AMF", "VA-API"]
                    .iter()
                    .any(|e| details.contains(e))
        );
        assert_eq!(
            probe(Path::new(&output.path))["streams"][0]["codec_name"],
            "h264"
        );
    }
    no_partials(dir.path());
}

#[test]
fn video_automatic_quality_extremes_resize_and_explicit_fps() {
    let dir = tempfile::tempdir().unwrap();
    let source = hardware_video(&fixture("apple-h264.mp4"), &dir.path().join("source.mp4"));
    let mut options = video_options();
    options.acceleration = crate::video_backend::Acceleration::Automatic;
    options.resolution = Some(480);
    options.frame_rate = Some(24);
    for quality in [1, 100] {
        options.quality = quality;
        let output = convert(&source, &options);
        eprintln!("Automatic quality {quality}: {output:?}");
        let data = probe(Path::new(&output.path));
        assert_eq!(data["streams"][0]["height"], 480);
        assert_eq!(data["streams"][0]["avg_frame_rate"], "24/1");
        if let Ok(required) = std::env::var("RECAST_REQUIRE_HARDWARE") {
            assert!(
                output.details.as_ref().unwrap().contains(&required),
                "Expected hardware {required}: {output:?}"
            );
        }
    }
    no_partials(dir.path());
}

#[test]
fn video_automatic_small_input_keeps_dimensions_with_optional_fallback() {
    // Small real inputs must still convert, even on hardware whose minimum
    // dimensions exceed theirs. Required-hardware checks use the larger fixtures.
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("small.mp4"));
    let output = convert(
        &source,
        &VideoOptions {
            acceleration: crate::video_backend::Acceleration::Automatic,
            ..video_options()
        },
    );
    let data = probe(Path::new(&output.path));
    assert_eq!(data["streams"][0]["width"], 64);
    assert_eq!(data["streams"][0]["height"], 48);
    no_partials(dir.path());
}

#[test]
fn video_automatic_cancellation_during_hardware_selection_does_not_retry() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    let cancelled = AtomicBool::new(false);
    let mut options = video_options();
    options.acceleration = crate::video_backend::Acceleration::Automatic;
    let result = audio_backend().convert_video(&source, None, &options, &cancelled, |_, phase| {
        if phase == "Checking hardware" {
            cancelled.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(ConversionError::Cancelled)));
    assert!(!dir.path().join("source (1).mp4").exists());
    no_partials(dir.path());
}
