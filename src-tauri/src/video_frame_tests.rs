use super::*;
use crate::video_frame::{FrameOptions, FrameSelection};

fn settings(source: &Path, selection: FrameSelection, target: &str) -> FrameOptions {
    FrameOptions {
        image: ImageOptions {
            target: target.into(),
            lossless: true,
            ..options()
        },
        frames: [(source.to_string_lossy().into_owned(), selection)].into(),
    }
}
fn preview(source: &Path, time: f64) -> (crate::video_frame::FramePreview, Vec<u8>) {
    audio_backend()
        .preview_selected_frame(source, time, &AtomicBool::new(false))
        .unwrap()
}
// Independent full sequential decode supplies both frame order and pixels.
// No seeking, trimming, FPS filter, or export implementation participates.
fn decoded(source: &Path) -> (Vec<i64>, Vec<u8>) {
    let probe = audio_backend()
        .command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp",
            "-of",
            "json",
        ])
        .arg(source)
        .output()
        .unwrap();
    assert!(probe.status.success());
    let data: Value = serde_json::from_slice(&probe.stdout).unwrap();
    let timestamps = data["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["best_effort_timestamp"].as_i64().unwrap())
        .collect();
    let output = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-threads", "2", "-filter_threads", "1", "-i"])
        .arg(source)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-threads:v",
            "1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (timestamps, output.stdout)
}

#[test]
fn selected_frames_match_sequential_decode_for_vfr_rotation_av1_and_high_rates() {
    let dir = tempfile::tempdir().unwrap();
    let fast = make_video(dir.path(), "fast.mp4", "240", None);
    for source in [
        fixture("apple-h264.mp4"),
        fixture("apple-vfr-rotated.mov"),
        fixture("video-av1.mp4"),
        fixture("video-vp9-opus.webm"),
        fixture("video-vp8-vorbis.webm"),
        fixture("video-av1-vfr.mkv"),
        fixture("video-hevc.mov"),
        fixture("video-mpeg4.avi"),
        fixture("video-mjpeg.avi"),
        fast,
    ] {
        let info = crate::video_animation::inspect(&probe(&source)).unwrap();
        let (timestamps, rgb) = decoded(&source);
        let stride = info.width as usize * info.height as usize * 3;
        assert_eq!(rgb.len(), timestamps.len() * stride);
        for time in [0.0, info.duration * 0.6, info.duration] {
            let (selected, pixels) = preview(&source, time);
            let index = timestamps
                .iter()
                .position(|p| p.to_string() == selected.selection.pts)
                .unwrap();
            assert_eq!(
                pixels,
                crate::video_animation::bmp(
                    &rgb[index * stride..(index + 1) * stride],
                    info.width,
                    info.height
                ),
                "{source:?} at {time}"
            );
            if time == 0.0 {
                assert_eq!(index, 0);
                assert!(selected.previous.is_none());
            }
            if time == info.duration {
                assert_eq!(index, timestamps.len() - 1);
                assert!(selected.next.is_none());
            }
            if let Some(next) = selected.next {
                assert_eq!(
                    preview(&source, next).0.selection.pts,
                    timestamps[index + 1].to_string()
                );
            }
            if let Some(previous) = selected.previous {
                assert_eq!(
                    preview(&source, previous).0.selection.pts,
                    timestamps[index - 1].to_string()
                );
            }
        }
    }
}

#[test]
fn selected_frames_export_every_image_format_and_preserve_names_originals_and_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-vfr-rotated.mov", &dir.path().join("clip [1] %.mov"));
    let original = fs::read(&source).unwrap();
    let (frame, pixels) = preview(&source, 0.48);
    let reference = dir.path().join("reference.bmp");
    fs::write(&reference, pixels).unwrap();
    for target in crate::video_frame::TARGETS {
        let options = settings(&source, frame.selection.clone(), target);
        let result = audio_backend()
            .convert_frame(
                &backend(),
                &source,
                None,
                &options,
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap();
        assert!(Path::new(&result.path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("clip [1] %"));
        assert_eq!(identify(Path::new(&result.path), "%w %h"), "48 64");
        if ["png", "tiff", "bmp"].contains(&target) {
            // ImageMagick's decoded-pixel signature includes every pixel.
            assert_eq!(
                identify(&reference, "%#"),
                identify(Path::new(&result.path), "%#"),
                "{target}"
            );
        }
    }
    let mut half = settings(&source, frame.selection.clone(), "png");
    half.image.resize = 50;
    let result = audio_backend()
        .convert_frame(
            &backend(),
            &source,
            None,
            &half,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
    assert_eq!(identify(Path::new(&result.path), "%w %h"), "24 32");
    assert!(result.path.ends_with("clip [1] % (1).png"));
    assert_eq!(fs::read(&source).unwrap(), original);
    no_partials(dir.path());
}

#[test]
fn selected_frames_handle_nonzero_origin_stream_index_and_sparse_frames() {
    let dir = tempfile::tempdir().unwrap();
    let sparse = make_video(dir.path(), "sparse.mp4", "1/3", None);
    let source = dir.path().join("offset.mkv");
    ffmpeg(&[
        "-i",
        fixture("apple-aac-lc.m4a").to_str().unwrap(),
        "-i",
        sparse.to_str().unwrap(),
        "-map",
        "0:a",
        "-map",
        "1:v",
        "-c",
        "copy",
        "-output_ts_offset",
        "7",
        source.to_str().unwrap(),
    ]);
    let info = crate::video_animation::inspect(&probe(&source)).unwrap();
    assert_eq!(info.stream_index, 1);
    let (points, rgb) = decoded(&source);
    let (first, _) = preview(&source, 0.0);
    let (next, pixels) = preview(&source, first.next.unwrap());
    assert_eq!(first.selection.pts, points[0].to_string());
    assert_eq!(next.selection.pts, points[1].to_string());
    assert_eq!(
        pixels,
        crate::video_animation::bmp(&rgb[64 * 48 * 3..2 * 64 * 48 * 3], 64, 48)
    );
    assert_eq!(
        preview(&source, 72.2).0.selection.pts,
        points[24].to_string()
    );
}

#[test]
fn selected_frames_changed_sources_bad_selections_and_cancellation_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("video.mp4"));
    let (selected, _) = preview(&source, 0.7);
    let options = settings(&source, selected.selection, "png");
    for phase in ["Reading selected frame", "Saving image"] {
        let cancel = AtomicBool::new(false);
        let result =
            audio_backend().convert_frame(&backend(), &source, None, &options, &cancel, |_, p| {
                if p == phase {
                    cancel.store(true, Ordering::Relaxed);
                }
            });
        assert!(matches!(result, Err(ConversionError::Cancelled)));
        assert!(!dir.path().join("video.png").exists());
    }
    assert!(matches!(
        audio_backend().preview_selected_frame(&source, 0.0, &AtomicBool::new(true)),
        Err(ConversionError::Cancelled)
    ));
    for time in [-1.0, 999.0, f64::NAN] {
        assert!(audio_backend()
            .preview_selected_frame(&source, time, &AtomicBool::new(false))
            .is_err());
    }
    let mut bad = options.clone();
    bad.frames.values_mut().next().unwrap().pts = i64::MAX.to_string();
    assert!(bad.validate().is_err());
    fs::OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    let result = audio_backend().convert_frame(
        &backend(),
        &source,
        None,
        &options,
        &AtomicBool::new(false),
        |_, _| {},
    );
    assert!(matches!(result, Err(ConversionError::Failed(s)) if s.contains("changed")));
    assert!(!dir.path().join("video.png").exists());
    no_partials(dir.path());
}

#[test]
fn selected_frames_batch_retry_freezes_selection_and_mixes_image_groups() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-h264.mp4", &dir.path().join("video.mp4"));
    let missing = dir.path().join("later.mp4");
    let still = copy("rgba.png", &dir.path().join("still.png"));
    let (selected, pixels) = preview(&source, 1.3);
    let reference = dir.path().join("ref.bmp");
    fs::write(&reference, pixels).unwrap();
    let mut req = request(&[source.clone(), missing.clone(), still], dir.path());
    req.video_frame_options = Some(settings(&source, selected.selection, "png"));
    let manager = JobManager::default();
    let job = manager.prepare("frames", req.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert_eq!(before.files[0].status, FileStatus::Succeeded, "{before:?}");
    assert_eq!(before.files[1].status, FileStatus::Failed);
    assert_eq!(before.files[2].status, FileStatus::Succeeded);
    assert_eq!(
        identify(&reference, "%#"),
        identify(
            Path::new(before.files[0].output_path.as_ref().unwrap()),
            "%#"
        )
    );
    copy("apple-h264.mp4", &missing);
    req.video_frame_options.as_mut().unwrap().image.target = "jpeg".into();
    let retry = manager.prepare("frames", req, true).unwrap();
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
    assert!(after.files[1]
        .output_path
        .as_ref()
        .unwrap()
        .ends_with("later.png"));
    let (_, first) = preview(&missing, 0.0);
    fs::write(&reference, first).unwrap();
    assert_eq!(
        identify(&reference, "%#"),
        identify(
            Path::new(after.files[1].output_path.as_ref().unwrap()),
            "%#"
        )
    );
    no_partials(dir.path());
}

#[test]
#[ignore = "manual real-file verification; requires RECAST_FRAME_SOURCE and RECAST_FRAME_EVIDENCE"]
fn selected_frame_real_source() {
    let source = PathBuf::from(std::env::var_os("RECAST_FRAME_SOURCE").unwrap());
    let folder = PathBuf::from(std::env::var_os("RECAST_FRAME_EVIDENCE").unwrap());
    fs::create_dir_all(&folder).unwrap();
    let (frame, pixels) = preview(&source, 37.25);
    fs::write(folder.join("preview.bmp"), pixels).unwrap();
    fs::write(
        folder.join("frame.json"),
        serde_json::to_vec_pretty(&frame).unwrap(),
    )
    .unwrap();
    let result = audio_backend()
        .convert_frame(
            &backend(),
            &source,
            Some(&folder),
            &settings(&source, frame.selection, "png"),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
    assert_eq!(identify(Path::new(&result.path), "%w %h"), "3840 1632");
}
