use super::*;

fn tone(
    path: &Path,
    rate: u32,
    channels: u16,
    frames: usize,
    amplitude: f32,
    mask: Option<u32>,
) -> Vec<f32> {
    let samples: Vec<_> = (0..frames)
        .flat_map(|i| {
            (0..channels).map(move |c| {
                let frequency = if channels >= 6 && c == 3 {
                    40.0
                } else {
                    431.0 + f32::from(c) * 173.0
                };
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
        mask,
    );
    samples
}

fn floats(path: &str) -> Vec<f32> {
    pcm(Path::new(path))
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn inspect(path: &str) -> crate::audio_backend::AudioInfo {
    audio_backend()
        .inspect(Path::new(path), &AtomicBool::new(false))
        .unwrap()
}

fn probe(path: &str) -> serde_json::Value {
    let output = audio_backend()
        .command("ffprobe")
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
            path,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn aac_independent_apple_lc_and_adts_inputs_use_content_and_gapless_metadata() {
    for (name, profile, frames) in [
        ("apple-aac-lc.m4a", "LC", 48117),
        ("apple-aac-adts.aac", "LC", 51200),
        ("unsupported-aac.m4a", "LC", 5003),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("audio.png");
        fs::copy(fixture(name), &source).unwrap();
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(input.kind, "audio");
        assert_eq!(serde_json::to_value(&input).unwrap()["format"], "AAC");
        assert_eq!(input.audio.as_ref().unwrap().sample_format, "lossy");
        assert_eq!(
            probe(source.to_str().unwrap())["streams"][0]["profile"],
            profile
        );
        let output = convert(&source, &audio_options("wav"));
        let samples = floats(&output.path);
        assert_eq!(samples.len(), frames * 2, "{name}");
        if name.starts_with("apple-aac-") && !name.ends_with(".aac") {
            let mut error = 0.0f64;
            for (i, pair) in samples.as_chunks::<2>().0.iter().take(48117).enumerate() {
                for (c, (amplitude, frequency)) in
                    [(9000.0, 431.0), (7000.0, 997.0)].iter().enumerate()
                {
                    let reference = (amplitude
                        * (i as f64 * frequency * std::f64::consts::TAU / 48000.0).sin())
                    .trunc()
                        / 32768.0;
                    error += (f64::from(pair[c]) - reference).powi(2);
                }
            }
            assert!(error / (samples.len() as f64) < 0.001, "{name}: {error}");
        }
        assert_eq!(
            output.warnings.iter().any(|w| w.contains("Raw AAC")),
            name.ends_with(".aac")
        );
        no_partials(dir.path());
    }
}

#[test]
fn aac_every_rate_and_partial_frame_exports_keep_exact_lengths() {
    audio_backend().verify().unwrap();
    for &rate in crate::audio_backend::AAC_RATES {
        for channels in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("tone.wav");
            let original = tone(&source, rate, channels, rate as usize + 117, 0.25, None);
            let mut options = audio_options("m4a");
            options.aac.bitrate = (if rate >= 32000 {
                128
            } else if rate >= 16000 {
                64
            } else {
                32
            }) * channels;
            let encoded = convert(&source, &options);
            let info = inspect(&encoded.path);
            assert_eq!(info.codec, "aac");
            assert_eq!(info.sample_rate, rate);
            assert_eq!(info.channels, channels);
            assert_eq!(probe(&encoded.path)["streams"][0]["profile"], "LC");
            assert!(encoded.details.as_ref().unwrap().contains("kbps target"));
            let back = convert(Path::new(&encoded.path), &audio_options("wav"));
            let samples = floats(&back.path);
            assert_eq!(samples.len(), original.len(), "{rate}/{channels}");
            let mse = samples
                .iter()
                .zip(original)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                / samples.len() as f32;
            assert!(mse < 0.003, "{rate}/{channels}: {mse}");
        }
    }
    for frames in [1, 47, 1023, 1024, 1025, 2048] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("short.wav");
        tone(&source, 48000, 1, frames, 0.2, None);
        let encoded = convert(&source, &audio_options("m4a"));
        let back = convert(Path::new(&encoded.path), &audio_options("wav"));
        assert_eq!(floats(&back.path).len(), frames);
    }
}

#[test]
fn aac_standard_surround_layouts_preserve_each_channel_without_remapping() {
    for (channels, mask) in [
        (3, 0x7),
        (4, 0x107),
        (5, 0x37),
        (6, 0x3f),
        (7, 0x13f),
        (8, 0x63f),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("surround.wav");
        let original = tone(&source, 48000, channels, 48117, 0.25, Some(mask));
        let mut options = audio_options("m4a");
        options.aac.bitrate = 768;
        let encoded = convert(&source, &options);
        assert_eq!(
            inspect(&encoded.path).channel_layout,
            inspect(source.to_str().unwrap()).channel_layout
        );
        let back = convert(Path::new(&encoded.path), &audio_options("wav"));
        let samples = floats(&back.path);
        assert_eq!(samples.len(), original.len());
        for c in 0..channels as usize {
            let mse = samples
                .iter()
                .zip(&original)
                .skip(c)
                .step_by(channels as usize)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                / 48117.0;
            assert!(mse < 0.002, "{channels} channel {c}: {mse}");
        }
    }
}

#[test]
fn aac_incompatible_rates_bitrates_layouts_and_hidden_pcm_settings_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 192000, 1, 12017, 0.3, None);
    let info = inspect(source.to_str().unwrap());
    let mut options = audio_options("m4a");
    assert!(options.resolve(&info).unwrap_err().contains("sample rate"));
    options.sample_rate = Some(8000);
    assert!(options
        .resolve(&info)
        .unwrap_err()
        .contains("compatible bitrate"));
    options.aac.bitrate = 32;
    options.sample_format = "u8".into();
    let first = convert(&source, &options);
    options.sample_format = "f64".into();
    options.dither = false;
    let second = convert(&source, &options);
    assert_eq!(
        fs::read(&first.path).unwrap(),
        fs::read(&second.path).unwrap()
    );
    assert_eq!(inspect(&first.path).sample_rate, 8000);
    let side = dir.path().join("side.wav");
    tone(&side, 48000, 6, 10000, 0.2, Some(0x60f));
    let input = crate::inputs::inspect_with_audio(&side, &audio_backend(), &AtomicBool::new(false))
        .unwrap();
    let json = serde_json::to_value(&input).unwrap();
    assert!(!json["targets"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("m4a")));
    assert!(json["targetIssues"]["m4a"]
        .as_str()
        .unwrap()
        .contains("channel layout"));
    assert!(audio_backend()
        .convert(&side, None, &options, &AtomicBool::new(false))
        .is_err());
    no_partials(dir.path());
}

#[test]
fn aac_lossy_defaults_cross_conversions_and_mixed_batches_preserve_output_count() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("apple-aac-lc.m4a", &dir.path().join("aac.m4a"));
    for (target, format) in [
        ("wav", "f32"),
        ("flac", "s24"),
        ("alac", "s24"),
        ("mp3", "lossy"),
    ] {
        let output = convert(&source, &audio_options(target));
        assert_eq!(inspect(&output.path).sample_format, format);
        assert!(output
            .warnings
            .iter()
            .any(|w| w.contains(if target == "mp3" {
                "lossy generation"
            } else {
                "does not restore"
            })));
        if target != "mp3" {
            let mut settings = audio_options(target);
            settings.sample_format = "s16".into();
            assert_eq!(
                inspect(&convert(&source, &settings).path).sample_format,
                "s16"
            );
        }
    }
    let integer = dir.path().join("integer.wav");
    wav(&integer, 16, false, 48000, 1, &vec![0; 96000], None);
    let image = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[source, integer, image], dir.path());
    request.audio_options = Some(audio_options("m4a"));
    let manager = JobManager::default();
    let job = manager.prepare("aac-mixed", request, false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let snapshot = job.snapshot();
    assert_eq!(snapshot.files.len(), 3);
    for file in &snapshot.files {
        assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
    }
    assert!(snapshot.files[0]
        .output_path
        .as_ref()
        .unwrap()
        .ends_with(".m4a"));
    assert!(snapshot.files[2]
        .output_path
        .as_ref()
        .unwrap()
        .ends_with(".webp"));
    no_partials(dir.path());
}

#[test]
fn aac_metadata_same_container_names_and_bitrate_targets_are_respected() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 48000, 2, 144117, 0.3, None);
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
            "track=2/7",
            "-metadata",
            "replaygain_track_gain=-5 dB",
        ])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(result.status.success());
    let mut settings = audio_options("m4a");
    settings.aac.bitrate = 64;
    let small = convert(&tagged, &settings);
    settings.aac.bitrate = 256;
    let large = convert(&tagged, &settings);
    assert!(large.bytes > small.bytes, "{} {}", large.bytes, small.bytes);
    let tags = probe(&large.path);
    assert_eq!(tags["format"]["tags"]["title"], "夜 = tone");
    assert_eq!(tags["format"]["tags"]["track"], "2/7");
    assert!(tags["format"]["tags"]["replaygain_track_gain"].is_null());
    let original = fs::read(&large.path).unwrap();
    settings.metadata = false;
    let again = convert(Path::new(&large.path), &settings);
    assert_ne!(again.path, large.path);
    assert_eq!(fs::read(&large.path).unwrap(), original);
    assert!(probe(&again.path)["format"]["tags"]["title"].is_null());
    let lossless = convert(Path::new(&large.path), &audio_options("alac"));
    assert!(lossless.path.ends_with(".m4a"));
    assert_ne!(lossless.path, large.path);
    assert_eq!(inspect(&lossless.path).codec, "alac");
    no_partials(dir.path());
}

#[test]
fn aac_truncated_adts_and_m4a_and_multiple_tracks_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["apple-aac-lc.m4a", "apple-aac-adts.aac"] {
        let data = fs::read(fixture(name)).unwrap();
        for cut in [data.len() / 2, data.len() - 37] {
            let bad = dir.path().join(format!("bad-{cut}-{name}"));
            fs::write(&bad, &data[..cut]).unwrap();
            assert!(
                audio_backend()
                    .convert(&bad, None, &audio_options("wav"), &AtomicBool::new(false))
                    .is_err(),
                "{name}/{cut}"
            );
            assert!(!bad.with_extension("wav").exists());
        }
    }
    let data = fs::read(fixture("apple-aac-adts.aac")).unwrap();
    let mut changed = data.clone();
    let frame = ((usize::from(data[3] & 3)) << 11)
        | (usize::from(data[4]) << 3)
        | usize::from(data[5] >> 5);
    changed[frame + 2] ^= 4;
    let bad = dir.path().join("changed.aac");
    fs::write(&bad, changed).unwrap();
    assert!(crate::aac::validate_adts(&bad, &AtomicBool::new(false)).is_err());
    let multiple = dir.path().join("multiple.m4a");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(fixture("apple-aac-lc.m4a"))
        .args(["-map", "0:a", "-map", "0:a", "-c:a", "copy", "-f", "ipod"])
        .arg(&multiple)
        .output()
        .unwrap();
    assert!(result.status.success());
    let error = audio_backend()
        .convert(
            &multiple,
            None,
            &audio_options("wav"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
    assert!(matches!(error,ConversionError::Failed(ref s) if s.contains("one audio stream")));
    no_partials(dir.path());
}

#[test]
fn aac_float_overload_keeps_gain_and_integer_retry_preserves_successes() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hot.wav");
    tone(&source, 48000, 2, 48117, 1.4, None);
    let encoded = convert(&source, &audio_options("m4a"));
    assert!(encoded.warnings.iter().any(|s| s.contains("full scale")));
    let float = convert(Path::new(&encoded.path), &audio_options("wav"));
    assert!(floats(&float.path).iter().any(|s| s.abs() > 1.2));
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[picture, PathBuf::from(&encoded.path)], dir.path());
    request.audio_options = Some(audio_options("flac"));
    let manager = JobManager::default();
    let job = manager.prepare("aac-hot", request.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let snapshot = job.snapshot();
    assert_eq!(snapshot.files[0].status, FileStatus::Succeeded);
    assert!(snapshot.files[1].can_allow_clipping);
    request.audio_options.as_mut().unwrap().allow_clipping = true;
    let retry = manager.prepare("aac-hot", request, true).unwrap();
    run_job(&retry, &backend(), &audio_backend(), |_| {});
    let result = retry.snapshot();
    assert_eq!(result.files[0].output_path, snapshot.files[0].output_path);
    assert_eq!(result.files[1].status, FileStatus::Succeeded);
    assert!(result.files[1]
        .warnings
        .iter()
        .any(|w| w.contains("Clipping occurred")));
    no_partials(dir.path());
}

#[test]
fn aac_active_encoder_cancellation_cleans_up() {
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
        audio_backend().convert(&source, None, &audio_options("m4a"), &signal)
    });
    let started = Instant::now();
    let mut saw_output = false;
    while started.elapsed() < Duration::from_secs(30) && !worker.is_finished() {
        if fs::read_dir(dir.path()).unwrap().any(|f| {
            f.unwrap()
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
    assert!(saw_output, "{result:?}");
    assert!(matches!(result, Err(ConversionError::Cancelled)));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    no_partials(dir.path());
}

#[test]
fn aac_he_profiles_are_blocked_until_decoder_timing_is_reliable() {
    for name in ["apple-aac-he.m4a", "apple-aac-hev2.m4a"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join(name));
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        let json = serde_json::to_value(input).unwrap();
        assert!(json["targets"].as_array().unwrap().is_empty());
        assert!(json["conversionIssue"]
            .as_str()
            .unwrap()
            .contains("timing reliably"));
        let error = audio_backend()
            .convert(
                &source,
                None,
                &audio_options("wav"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(matches!(error, ConversionError::Failed(ref text) if text.contains("HE-AAC")));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
