use super::*;
use crate::audio_extract::{selected_track, VideoAudioOptions, TARGETS};
use crate::jobs::tests::audio::audio_options;

fn extract(source: &Path, target: &str, track: Option<u32>) -> crate::image_backend::OutputFile {
    audio_backend()
        .extract_audio(
            source,
            None,
            &audio_options(target),
            track,
            &AtomicBool::new(false),
        )
        .unwrap()
}

fn decode(path: &Path, map: &str) -> Vec<u8> {
    let result = audio_backend()
        .command("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "1",
            "-i",
            path.to_str().unwrap(),
            "-map",
            map,
            "-c:a",
            "pcm_f64le",
            "-f",
            "f64le",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

fn multitrack(dir: &Path, name: &str, second_level: f64) -> PathBuf {
    let silent = make_video(dir, "silent.mp4", "30", None);
    let a = dir.join("first.f64");
    let b = dir.join("second.f64");
    fs::write(&a, 0.25f64.to_le_bytes().repeat(12007)).unwrap();
    fs::write(&b, second_level.to_le_bytes().repeat(48017 * 2)).unwrap();
    let source = dir.join(name);
    ffmpeg(&[
        "-i",
        silent.to_str().unwrap(),
        "-f",
        "f64le",
        "-ar",
        "16000",
        "-ac",
        "1",
        "-i",
        a.to_str().unwrap(),
        "-itsoffset",
        "2.25",
        "-f",
        "f64le",
        "-ar",
        "48000",
        "-ac",
        "2",
        "-i",
        b.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:a",
        "-c:v",
        "copy",
        "-c:a",
        "pcm_f64le",
        "-disposition:a:0",
        "0",
        "-disposition:a:1",
        "default",
        "-metadata",
        "title=Video title",
        "-metadata:s:a:0",
        "title=Main",
        "-metadata:s:a:0",
        "language=eng",
        "-metadata:s:a:1",
        "title=Commentary",
        "-metadata:s:a:1",
        "language=jpn",
        source.to_str().unwrap(),
    ]);
    source.canonicalize().unwrap()
}

#[test]
fn extraction_uses_default_or_explicit_track_without_mixing_padding_or_video_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let source = multitrack(dir.path(), "选择.mkv", -0.5);
    let original = fs::read(&source).unwrap();
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(input.selected_audio_track, Some(2));
    assert_eq!(input.audio_tracks.len(), 2);
    assert!(input.audio_tracks[1].label.contains("Commentary"));
    assert!(input.audio_tracks[1].label.contains("jpn"));
    let json = serde_json::to_value(&input).unwrap();
    for target in TARGETS {
        assert!(json["targets"].as_array().unwrap().contains(&target.into()));
    }
    for (track, expected, rate, channels) in [
        (None, (-0.5f64).to_le_bytes().repeat(48017 * 2), 48000, 2),
        (Some(1), 0.25f64.to_le_bytes().repeat(12007), 16000, 1),
    ] {
        let output = extract(&source, "wav", track);
        let path = Path::new(&output.path);
        assert_eq!(decode(path, "0:a:0"), expected);
        let data = probe(path);
        assert_eq!(data["streams"].as_array().unwrap().len(), 1);
        assert_eq!(data["streams"][0]["channels"], channels);
        assert_eq!(data["streams"][0]["sample_rate"], rate.to_string());
        assert_eq!(
            data["format"]["tags"]["title"],
            if track.is_some() {
                "Main"
            } else {
                "Commentary"
            }
        );
        assert!(output.details.unwrap().contains(if track.is_some() {
            "Track 1"
        } else {
            "Track 2"
        }));
    }
    assert_eq!(fs::read(&source).unwrap(), original);
    no_partials(dir.path());
}

#[test]
fn extraction_first_track_fallback_and_invalid_track_do_not_silently_select_another() {
    let dir = tempfile::tempdir().unwrap();
    let source = multitrack(dir.path(), "tracks.mkv", -0.5);
    let mut data = probe(&source);
    data["streams"][2]["disposition"]["default"] = 0.into();
    assert_eq!(selected_track(&data, None).unwrap().index, 1);
    assert!(selected_track(&data, Some(0))
        .unwrap_err()
        .contains("no longer available"));
    assert!(selected_track(&data, Some(9)).is_err());
    assert!(audio_backend()
        .extract_audio(
            &source,
            None,
            &audio_options("wav"),
            Some(0),
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(!dir.path().join("tracks.wav").exists());
    let mut options = audio_options("wav");
    options.metadata = false;
    let output = audio_backend()
        .extract_audio(&source, None, &options, Some(1), &AtomicBool::new(false))
        .unwrap();
    assert!(probe(Path::new(&output.path))["format"]["tags"]["title"].is_null());
    no_partials(dir.path());
}

#[test]
fn extraction_converts_aac_opus_vorbis_to_every_audio_destination() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "apple-h264.mp4",
        "video-vp9-opus.webm",
        "video-vp8-vorbis.webm",
    ] {
        let source = if name == "apple-h264.mp4" {
            let source = dir.path().join("aac.mp4");
            ffmpeg(&[
                "-i",
                fixture(name).to_str().unwrap(),
                "-i",
                fixture("apple-aac-lc.m4a").to_str().unwrap(),
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-c",
                "copy",
                source.to_str().unwrap(),
            ]);
            source
        } else {
            copy(name, &dir.path().join(name))
        };
        let original = decode(&source, "0:a:0");
        assert!(!original.is_empty());
        if let Some(folder) = std::env::var_os("RECAST_EXTRACTION_EVIDENCE") {
            let sources = Path::new(&folder).join("sources");
            fs::create_dir_all(&sources).unwrap();
            fs::copy(&source, sources.join(name)).unwrap();
        }
        for target in TARGETS {
            let output = extract(&source, target, None);
            let path = Path::new(&output.path);
            let data = probe(path);
            assert_eq!(
                data["streams"].as_array().unwrap().len(),
                1,
                "{name} → {target}"
            );
            assert_eq!(data["streams"][0]["codec_type"], "audio");
            let pcm = decode(path, "0:a:0");
            let source_info = probe(&source);
            let stream = source_info["streams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["codec_type"] == "audio")
                .unwrap();
            let channels = stream["channels"].as_u64().unwrap() as usize;
            let source_rate: f64 = stream["sample_rate"].as_str().unwrap().parse().unwrap();
            if target == "opus" {
                let expected =
                    original.len() as f64 / (8 * channels) as f64 * 48000.0 / source_rate;
                assert!(
                    (pcm.len() as f64 / (8 * channels) as f64 - expected).abs() <= 1.0,
                    "{name} → {target}"
                );
            } else {
                assert_eq!(pcm.len(), original.len(), "{name} → {target}");
            }
            if target == "wav" {
                assert_eq!(pcm, original, "{name}");
            }
            if let Some(folder) = std::env::var_os("RECAST_EXTRACTION_EVIDENCE") {
                fs::create_dir_all(&folder).unwrap();
                fs::copy(
                    path,
                    Path::new(&folder).join(format!(
                        "{name}-{target}.{}",
                        path.extension().unwrap().to_str().unwrap()
                    )),
                )
                .unwrap();
            }
        }
    }
    no_partials(dir.path());
}

#[test]
fn extraction_support_does_not_depend_on_video_pixels_or_unselected_audio() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hdr.mkv");
    ffmpeg(&[
        "-i",
        fixture("video-av1-10bit.mkv").to_str().unwrap(),
        "-i",
        fixture("apple-aac-lc.m4a").to_str().unwrap(),
        "-i",
        fixture("apple-aac-hev2.m4a").to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:a",
        "-c",
        "copy",
        "-disposition:a:0",
        "0",
        "-disposition:a:1",
        "default",
        source.to_str().unwrap(),
    ]);
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert!(input.require_video_target(&video_options()).is_err());
    assert_eq!(input.selected_audio_track, Some(2));
    assert!(input.audio_tracks[1]
        .issue
        .as_ref()
        .unwrap()
        .contains("AAC-LC"));
    assert!(input.audio_tracks[1].targets.is_empty());
    assert!(audio_backend()
        .extract_audio(
            &source,
            None,
            &audio_options("wav"),
            None,
            &AtomicBool::new(false)
        )
        .is_err());
    let output = extract(&source, "wav", Some(1));
    assert_eq!(
        decode(Path::new(&output.path), "0:a:0"),
        decode(&source, "0:1")
    );
    no_partials(dir.path());
}

#[test]
fn extraction_silent_video_and_cancel_publish_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let silent = make_video(dir.path(), "silent.mp4", "30", None);
    let input =
        crate::inputs::inspect_with_audio(&silent, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    let json = serde_json::to_value(&input).unwrap();
    assert_eq!(json["hasAudio"], false);
    for target in TARGETS {
        assert!(!json["targets"].as_array().unwrap().contains(&target.into()));
        assert!(json["targetIssues"][target]
            .as_str()
            .unwrap()
            .contains("no audio track"));
    }
    assert!(audio_backend()
        .extract_audio(
            &silent,
            None,
            &audio_options("mp3"),
            None,
            &AtomicBool::new(false)
        )
        .is_err());
    let source = copy("apple-h264.mp4", &dir.path().join("source.mp4"));
    assert!(matches!(
        audio_backend().extract_audio(
            &source,
            None,
            &audio_options("wav"),
            None,
            &AtomicBool::new(true)
        ),
        Err(ConversionError::Cancelled)
    ));
    assert!(!dir.path().join("source.wav").exists());
    assert!(!dir.path().join("silent.mp3").exists());
    no_partials(dir.path());
}

#[test]
fn extraction_reads_pcm_flac_alac_and_mp3_from_video_containers() {
    let dir = tempfile::tempdir().unwrap();
    let pcm_source = multitrack(dir.path(), "pcm.mkv", -0.5);
    for (name, container, encoder) in [
        ("apple-24.m4a", "mov", "copy"),
        ("lame-vbr.mp3", "mkv", "copy"),
        ("pcm.mkv", "mkv", "flac"),
        ("pcm.mkv", "mov", "pcm_s16le"),
    ] {
        let audio = if name == "pcm.mkv" {
            pcm_source.clone()
        } else {
            fixture(name)
        };
        let source = dir.path().join(format!("{encoder}.{container}"));
        ffmpeg(&[
            "-i",
            fixture("apple-h264.mp4").to_str().unwrap(),
            "-i",
            audio.to_str().unwrap(),
            // Put audio before video to catch accidental ordinal mapping.
            "-map",
            "1:a:0",
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-c:a",
            encoder,
            source.to_str().unwrap(),
        ]);
        let output = extract(&source, "wav", Some(0));
        assert_eq!(
            decode(Path::new(&output.path), "0:a:0"),
            decode(&source, "0:0"),
            "{name} / {encoder}"
        );
    }
}

#[test]
fn extraction_retains_surround_channels_and_blocks_mp3_without_downmixing() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("surround.f64");
    let frame: Vec<_> = [0.125f64, 0.25, 0.375, 0.5, 0.625, 0.75]
        .into_iter()
        .flat_map(f64::to_le_bytes)
        .collect();
    fs::write(&raw, frame.repeat(12007)).unwrap();
    let source = dir.path().join("surround.mkv");
    ffmpeg(&[
        "-i",
        fixture("apple-h264.mp4").to_str().unwrap(),
        "-f",
        "f64le",
        "-ar",
        "48000",
        "-ac",
        "6",
        "-channel_layout",
        "5.1",
        "-i",
        raw.to_str().unwrap(),
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "copy",
        "-c:a",
        "flac",
        source.to_str().unwrap(),
    ]);
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    let track = &input.audio_tracks[0];
    assert!(!track.targets.contains(&"mp3".into()));
    assert!(track.target_issues["mp3"].contains("downmix"));
    assert!(audio_backend()
        .extract_audio(
            &source,
            None,
            &audio_options("mp3"),
            None,
            &AtomicBool::new(false)
        )
        .is_err());
    let output = extract(&source, "wav", None);
    assert_eq!(
        decode(Path::new(&output.path), "0:a:0"),
        frame.repeat(12007)
    );
    assert_eq!(
        probe(Path::new(&output.path))["streams"][0]["channel_layout"],
        "5.1"
    );
}

#[test]
fn extraction_mixed_batch_clipping_retry_freezes_track_and_independent_group_settings() {
    let dir = tempfile::tempdir().unwrap();
    let source = multitrack(dir.path(), "clipped.mkv", 1.25);
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let audio = copy("apple-aac-lc.m4a", &dir.path().join("sound.m4a"));
    let mut req = request(&[image, audio, source.clone()], dir.path());
    req.audio_options = Some(audio_options("wav"));
    let mut selected = audio_options("flac");
    selected.sample_format = "s24".into();
    req.video_audio_options = Some(VideoAudioOptions {
        audio: selected,
        tracks: [(source.to_string_lossy().into_owned(), 2)].into(),
    });
    let manager = JobManager::default();
    let job = manager.prepare("extraction", req.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert_eq!(before.files[0].status, FileStatus::Succeeded, "{before:?}");
    assert!(before.files[0]
        .output_path
        .as_ref()
        .unwrap()
        .ends_with(".webp"));
    assert!(before.files[1]
        .output_path
        .as_ref()
        .unwrap()
        .ends_with(".wav"));
    assert!(before.files[2].can_allow_clipping, "{before:?}");
    let edited = req.video_audio_options.as_mut().unwrap();
    edited.audio.allow_clipping = true;
    edited.audio.target = "mp3".into();
    edited
        .tracks
        .insert(source.to_string_lossy().into_owned(), 1);
    let job = manager.prepare("extraction", req, true).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let after = job.snapshot();
    assert!(
        after
            .files
            .iter()
            .all(|f| f.status == FileStatus::Succeeded),
        "{after:?}"
    );
    for i in 0..2 {
        assert_eq!(before.files[i].output_path, after.files[i].output_path);
    }
    let output = after.files[2].output_path.as_ref().unwrap();
    assert!(output.ends_with(".flac"));
    assert!(after.files[2].details.as_ref().unwrap().contains("Track 2"));
    let pcm = decode(Path::new(output), "0:a:0");
    assert_eq!(pcm.len(), 48017 * 2 * 8);
    assert!(f64::from_le_bytes(pcm[..8].try_into().unwrap()) > 0.99);
    no_partials(dir.path());
}

#[test]
#[ignore = "Optional real-file check: set RECAST_EXTRACT_SOURCE and RECAST_EXTRACT_OUTPUT"]
fn extraction_real_file() {
    let source = PathBuf::from(std::env::var_os("RECAST_EXTRACT_SOURCE").expect("source"));
    let folder = PathBuf::from(std::env::var_os("RECAST_EXTRACT_OUTPUT").expect("output folder"));
    fs::create_dir_all(&folder).unwrap();
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(input.kind, "video");
    let output = audio_backend()
        .extract_audio(
            &source,
            Some(&folder),
            &audio_options("wav"),
            input.selected_audio_track,
            &AtomicBool::new(false),
        )
        .unwrap();
    println!(
        "{}",
        serde_json::json!({"input": input, "output": output.path, "details": output.details})
    );
}
