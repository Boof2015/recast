use super::*;
use crate::audio_backend::Mp3Mode;

fn tone(path: &Path, rate: u32, channels: u16, frames: usize, amplitude: f32) -> Vec<f32> {
    let samples: Vec<_> = (0..frames)
        .flat_map(|i| {
            (0..channels).map(move |c| {
                let frequency = if c == 0 { 431.0 } else { 997.0 };
                (i as f32 * frequency * std::f32::consts::TAU / rate as f32).sin() * amplitude
            })
        })
        .collect();
    wav(
        path,
        32,
        true,
        rate,
        channels,
        &samples
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>(),
        None,
    );
    samples
}

fn floats(path: &Path) -> Vec<f32> {
    pcm(path)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

fn inspect(path: &Path) -> crate::audio_backend::AudioInfo {
    audio_backend()
        .inspect(path, &AtomicBool::new(false))
        .unwrap()
}

#[test]
fn mp3_standalone_lame_fixtures_cover_crc_gapless_and_untagged_inputs() {
    for name in ["lame-vbr.mp3", "lame-crc.mp3", "lame-no-xing.mp3"] {
        let dir = tempfile::tempdir().unwrap();
        // Detection must use content, including a raw MPEG header with no ID3.
        let source = dir.path().join("audio.png");
        fs::copy(fixture(name), &source).unwrap();
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(input.kind, "audio");
        input.require_audio_target(&audio_options("wav")).unwrap();
        let output = convert(&source, &audio_options("wav"));
        let actual = floats(Path::new(&output.path));
        if name != "lame-no-xing.mp3" {
            assert_eq!(actual.len(), 44117 * 2, "{name}");
            let mut error = 0.0f64;
            for (i, samples) in actual.as_chunks::<2>().0.iter().enumerate() {
                for (c, (amplitude, frequency)) in
                    [(9000.0, 431.0), (7000.0, 997.0)].iter().enumerate()
                {
                    let reference = (amplitude
                        * (i as f64 * frequency * std::f64::consts::TAU / 44100.0).sin())
                    .trunc()
                        / 32768.0;
                    error += (f64::from(samples[c]) - reference).powi(2);
                }
            }
            assert!(error / (actual.len() as f64) < 0.0001, "{name}: {error}");
        } else {
            // No delay/padding metadata: retain all decoded samples rather than
            // guessing a trim. Both encoders' complete frames are still read.
            assert!(actual.len() > 44117 * 2);
            assert!(actual.len() < 47000 * 2);
        }
        no_partials(dir.path());
    }
}

#[test]
fn mp3_text_tags_unicode_and_same_format_conversion_respect_retention_and_names() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tagged.wav");
    tone(&source, 48000, 2, 48117, 0.3);
    let tagged = dir.path().join("tags.flac");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args([
            "-c:a",
            "flac",
            "-sample_fmt",
            "s32",
            "-bits_per_raw_sample",
            "24",
            "-metadata",
            "title=夜 = tone",
            "-metadata",
            "artist=Recast fixture",
            "-metadata",
            "album=Fixture album",
            "-metadata",
            "track=2/7",
            "-metadata",
            "disc=1/2",
            "-metadata",
            "replaygain_track_gain=-5 dB",
        ])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let kept = convert(&tagged, &audio_options("mp3"));
    let probe = |path: &str| -> serde_json::Value {
        let output = audio_backend()
            .command("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format_tags",
                "-of",
                "json",
                path,
            ])
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let tags = probe(&kept.path);
    assert_eq!(tags["format"]["tags"]["title"], "夜 = tone");
    assert_eq!(tags["format"]["tags"]["track"], "2/7");
    assert_eq!(tags["format"]["tags"]["disc"], "1/2");
    assert!(tags["format"]["tags"]["replaygain_track_gain"].is_null());
    let original = fs::read(&kept.path).unwrap();
    let mut options = audio_options("mp3");
    options.metadata = false;
    let again = convert(Path::new(&kept.path), &options);
    assert!(again.path.ends_with("tags (1).mp3"));
    let tags = probe(&again.path);
    for name in ["title", "artist", "album", "track", "disc"] {
        assert!(tags["format"]["tags"][name].is_null());
    }
    assert!(again
        .warnings
        .iter()
        .any(|warning| warning.contains("lossy generation")));
    assert_eq!(fs::read(&kept.path).unwrap(), original);
    no_partials(dir.path());
}

#[test]
fn mp3_active_encoder_cancellation_reaps_work_and_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("long.wav");
    wav(
        &source,
        16,
        false,
        48000,
        2,
        &vec![0; 16 * 1024 * 1024],
        None,
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let worker = thread::spawn(move || {
        audio_backend().convert(&source, None, &audio_options("mp3"), &signal)
    });
    let started = Instant::now();
    let mut saw_output = false;
    while started.elapsed() < Duration::from_secs(30) && !worker.is_finished() {
        if fs::read_dir(dir.path()).unwrap().any(|file| {
            file.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".recast-")
        }) {
            saw_output = true;
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    cancel.store(true, Ordering::Relaxed);
    let result = worker.join().unwrap();
    assert!(saw_output, "Worker did not reach encoding: {result:?}");
    assert!(
        matches!(result, Err(ConversionError::Cancelled)),
        "{result:?}"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    no_partials(dir.path());
}

#[test]
fn mp3_padding_across_final_packets_preserves_short_and_partial_frames() {
    // The 12117/12000 case loses 26 samples of discard padding with the
    // unpatched FFmpeg 9.0.2 muxer. Verify other short/frame-boundary cases too.
    for (rate, frames) in [
        (12000, 12117),
        (44100, 1),
        (44100, 47),
        (44100, 576),
        (44100, 1152),
        (8000, 117),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("short.wav");
        tone(&source, rate, 1, frames, 0.3);
        let encoded = convert(&source, &audio_options("mp3"));
        let output = convert(Path::new(&encoded.path), &audio_options("wav"));
        assert_eq!(
            floats(Path::new(&output.path)).len(),
            frames,
            "{rate} Hz, {frames} frames"
        );
    }
}

#[test]
fn mp3_vbr_and_cbr_preserve_every_supported_rate_channel_count_and_gapless_length() {
    audio_backend().verify().unwrap();
    for (index, &rate) in crate::audio_backend::MP3_RATES.iter().enumerate() {
        for vbr in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("tone.wav");
            let channels = if index % 2 == 0 { 1 } else { 2 };
            let samples = tone(&source, rate, channels, rate as usize + 117, 0.35);
            let original = fs::read(&source).unwrap();
            let mut options = audio_options("mp3");
            options.mp3.mode = if vbr { Mp3Mode::Vbr } else { Mp3Mode::Cbr };
            options.mp3.bitrate = if rate < 16000 {
                64
            } else if rate < 32000 {
                160
            } else {
                320
            };
            let output = convert(&source, &options);
            assert!(output.path.ends_with("tone.mp3"));
            let info = inspect(Path::new(&output.path));
            assert_eq!(info.codec, "mp3");
            assert_eq!(info.sample_format, "lossy");
            assert_eq!((info.sample_rate, info.channels), (rate, channels));
            let decoded = convert(Path::new(&output.path), &audio_options("wav"));
            assert_eq!(inspect(Path::new(&decoded.path)).sample_format, "f32");
            let actual = floats(Path::new(&decoded.path));
            assert_eq!(actual.len(), samples.len(), "{rate} Hz {vbr}");
            // A synthetic-tone integrity check, not a perceptual quality claim.
            // A swapped, silent, or padded channel must not pass the round trip.
            for channel in 0..channels as usize {
                let energy: f64 = samples
                    .iter()
                    .skip(channel)
                    .step_by(channels as usize)
                    .map(|v| f64::from(*v).powi(2))
                    .sum();
                let error: f64 = samples
                    .iter()
                    .zip(&actual)
                    .skip(channel)
                    .step_by(channels as usize)
                    .map(|(a, b)| f64::from(a - b).powi(2))
                    .sum();
                assert!(
                    10.0 * (energy / error).log10() > 25.0,
                    "{rate} Hz {vbr}: energy {energy} error {error}"
                );
            }
            if !vbr {
                let probe = audio_backend()
                    .command("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-show_entries",
                        "stream=bit_rate",
                        "-of",
                        "json",
                    ])
                    .arg(&output.path)
                    .output()
                    .unwrap();
                let json: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
                assert_eq!(
                    json["streams"][0]["bit_rate"]
                        .as_str()
                        .unwrap()
                        .parse::<u32>()
                        .unwrap(),
                    u32::from(options.mp3.bitrate) * 1000
                );
            }
            assert_eq!(fs::read(&source).unwrap(), original);
            no_partials(dir.path());
        }
    }
}

#[test]
fn mp3_quality_controls_change_encoding_without_pcm_quantization_or_silent_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 44100, 2, 44117, 0.3);
    let mut options = audio_options("mp3");
    options.mp3.quality = 0;
    let highest = convert(&source, &options);
    options.mp3.quality = 8;
    let smaller = convert(&source, &options);
    assert!(highest.bytes > smaller.bytes);
    options.sample_format = "u8".into();
    options.dither = false;
    let remembered = convert(&source, &options);
    assert_eq!(
        fs::read(&smaller.path).unwrap(),
        fs::read(&remembered.path).unwrap()
    );
    options.mp3.quality = 10;
    assert!(options.validate().is_err());

    let high_rate = dir.path().join("high-rate.wav");
    tone(&high_rate, 96000, 1, 96117, 0.3);
    let mut options = audio_options("mp3");
    assert!(options
        .resolve(&inspect(&high_rate))
        .unwrap_err()
        .contains("sample rate"));
    options.sample_rate = Some(48000);
    let resampled = convert(&high_rate, &options);
    assert_eq!(inspect(Path::new(&resampled.path)).sample_rate, 48000);
    let low_rate = dir.path().join("low-rate.wav");
    tone(&low_rate, 8000, 1, 8117, 0.3);
    options.sample_rate = None;
    options.mp3.mode = Mp3Mode::Cbr;
    options.mp3.bitrate = 192;
    assert!(options
        .resolve(&inspect(&low_rate))
        .unwrap_err()
        .contains("192 kbps"));
    let surround = dir.path().join("surround.wav");
    wav(
        &surround,
        16,
        false,
        48000,
        6,
        &vec![0; 4800 * 6 * 2],
        Some(0x3f),
    );
    let input =
        crate::inputs::inspect_with_audio(&surround, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    let json = serde_json::to_value(input).unwrap();
    assert!(!json["targets"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("mp3")));
    assert!(json["targetIssues"]["mp3"]
        .as_str()
        .unwrap()
        .contains("downmix"));
    no_partials(dir.path());
}

#[test]
fn mp3_lossy_defaults_and_mixed_pcm_group_preserve_each_sources_appropriate_format() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 44100, 2, 44117, 0.3);
    let mp3 = convert(&source, &audio_options("mp3"));
    for target in ["wav", "flac", "alac"] {
        let output = convert(Path::new(&mp3.path), &audio_options(target));
        let format = if target == "wav" { "f32" } else { "s24" };
        assert_eq!(inspect(Path::new(&output.path)).sample_format, format);
        assert!(output
            .warnings
            .iter()
            .any(|warning| warning.contains("does not restore")));
        assert!(output.details.unwrap().ends_with(if target == "wav" {
            "Dither off"
        } else {
            "Dither applied"
        }));
        let mut options = audio_options(target);
        options.sample_format = "s16".into();
        assert_eq!(
            inspect(Path::new(&convert(Path::new(&mp3.path), &options).path)).sample_format,
            "s16"
        );
    }
    let integer = dir.path().join("integer.wav");
    wav(&integer, 16, false, 48000, 1, &vec![0; 48117 * 2], None);
    let mut request = request(&[PathBuf::from(&mp3.path), integer], dir.path());
    request.image_options = None;
    request.audio_options = Some(audio_options("flac"));
    let manager = JobManager::default();
    let job = manager.prepare("mp3-mixed", request, false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    for (file, format) in job.snapshot().files.iter().zip(["s24", "s16"]) {
        assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
        assert_eq!(
            inspect(Path::new(file.output_path.as_ref().unwrap())).sample_format,
            format
        );
    }
    no_partials(dir.path());
}

#[test]
fn mp3_damaged_frames_and_incomplete_streams_do_not_publish() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 44100, 2, 44117, 0.3);
    let output = convert(&source, &audio_options("mp3"));
    let bytes = fs::read(&output.path).unwrap();
    let mut joined = bytes.clone();
    joined.extend_from_slice(&bytes);
    for (name, damaged) in [
        ("partial-frame.mp3", bytes[..bytes.len() - 100].to_vec()),
        ("missing-frames.mp3", bytes[..bytes.len() / 2].to_vec()),
        ("joined.mp3", joined),
    ] {
        let bad = dir.path().join(name);
        fs::write(&bad, damaged).unwrap();
        let error =
            audio_backend().convert(&bad, None, &audio_options("wav"), &AtomicBool::new(false));
        assert!(error.is_err(), "{name}");
        assert!(!bad.with_extension("wav").exists());
    }
    no_partials(dir.path());
}

#[test]
fn mp3_float_peaks_warn_without_normalization_and_integer_conversion_requires_consent() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hot.wav");
    tone(&source, 48000, 2, 48117, 1.4);
    let encoded = convert(&source, &audio_options("mp3"));
    assert!(encoded
        .warnings
        .iter()
        .any(|warning| warning.contains("full scale")));
    let float = convert(Path::new(&encoded.path), &audio_options("wav"));
    assert!(floats(Path::new(&float.path))
        .iter()
        .any(|sample| sample.abs() > 1.2));
    let options = audio_options("flac");
    assert!(matches!(
        audio_backend().convert(
            Path::new(&encoded.path),
            None,
            &options,
            &AtomicBool::new(false)
        ),
        Err(ConversionError::Clipping)
    ));
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[picture, PathBuf::from(&encoded.path)], dir.path());
    request.audio_options = Some(options);
    let manager = JobManager::default();
    let job = manager
        .prepare("mp3-clipping", request.clone(), false)
        .unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let snapshot = job.snapshot();
    assert_eq!(snapshot.files[0].status, FileStatus::Succeeded);
    assert!(snapshot.files[1].can_allow_clipping);
    request.audio_options.as_mut().unwrap().allow_clipping = true;
    let retry = manager.prepare("mp3-clipping", request, true).unwrap();
    run_job(&retry, &backend(), &audio_backend(), |_| {});
    let result = retry.snapshot();
    assert_eq!(result.files[0].output_path, snapshot.files[0].output_path);
    assert_eq!(result.files[1].status, FileStatus::Succeeded);
    assert!(result.files[1]
        .warnings
        .iter()
        .any(|warning| warning.contains("Clipping occurred")));
    no_partials(dir.path());
}
