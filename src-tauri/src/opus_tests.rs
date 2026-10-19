use super::aac::{floats, inspect, probe, tone};
use super::*;

#[test]
fn opus_independent_packets_header_gain_and_continued_tags_decode_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("reference-opus.opus", &dir.path().join("audio.png"));
    let input =
        crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(input.kind, "audio");
    assert_eq!(serde_json::to_value(&input).unwrap()["format"], "OPUS");
    // The header's original-rate hint must not replace the playback rate.
    assert_eq!(input.audio.unwrap().sample_rate, 48000);
    let back = convert(&source, &audio_options("wav"));
    let samples = floats(&back.path);
    assert_eq!(samples.len(), 48117 * 2);
    let gain = 10.0f64.powf(3.0 / 20.0);
    let mse = samples
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(i, pair)| {
            [431.0, 997.0]
                .iter()
                .enumerate()
                .map(|(c, f)| {
                    let expected =
                        0.2 * gain * (i as f64 * f * std::f64::consts::TAU / 48000.0).sin();
                    (f64::from(pair[c]) - expected).powi(2)
                })
                .sum::<f64>()
        })
        .sum::<f64>()
        / samples.len() as f64;
    assert!(mse < 0.0001, "{mse}");
    assert!(back.warnings.iter().any(|w| w.contains("does not restore")));
    no_partials(dir.path());
}

#[test]
fn opus_cross_conversions_and_mixed_batch_produce_one_file_per_input() {
    for name in [
        "apple-aac-lc.m4a",
        "reference-opus.opus",
        "apple-16.m4a",
        "lame-vbr.mp3",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join(name));
        let opus = convert(&source, &audio_options("opus"));
        for target in ["mp3", "m4a", "wav", "flac", "alac"] {
            let mut options = audio_options(target);
            // The full-scale ALAC fixture can acquire overshoot when encoded.
            options.allow_clipping = true;
            convert(Path::new(&opus.path), &options);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let first = copy("reference-opus.opus", &dir.path().join("first.opus"));
    let second = copy("apple-aac-lc.m4a", &dir.path().join("second.m4a"));
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[first, second, picture], dir.path());
    request.audio_options = Some(audio_options("opus"));
    let manager = JobManager::default();
    let job = manager.prepare("opus-mixed", request, false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let result = job.snapshot();
    assert_eq!(result.files.len(), 3);
    for file in result.files {
        assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
    }
    no_partials(dir.path());
}

#[test]
fn opus_rates_short_clips_and_gapless_roundtrips_keep_full_audio() {
    audio_backend().verify().unwrap();
    for rate in [8000, 24000, 44100, 48000, 96000, 192000] {
        for channels in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("tone.wav");
            tone(&source, rate, channels, rate as usize, 0.25, None);
            let mut options = audio_options("opus");
            options.sample_rate = Some(7350); // A remembered AAC setting is irrelevant.
            options.sample_format = "u8".into();
            let encoded = convert(&source, &options);
            assert_eq!(inspect(&encoded.path).sample_rate, 48000);
            assert_eq!(inspect(&encoded.path).codec, "opus");
            assert!(encoded.path.ends_with(".opus"));
            assert!(encoded.details.unwrap().contains("128 kbps VBR target"));
            let back = convert(Path::new(&encoded.path), &audio_options("wav"));
            assert_eq!(floats(&back.path).len(), 48000 * channels as usize);
            assert_eq!(inspect(&back.path).sample_format, "f32");
        }
    }
    for frames in [1, 47, 117, 647, 648, 649, 959, 960, 961, 48117] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("short.wav");
        let original = tone(&source, 48000, 2, frames, 0.2, None);
        let encoded = convert(&source, &audio_options("opus"));
        let back = convert(Path::new(&encoded.path), &audio_options("wav"));
        let samples = floats(&back.path);
        assert_eq!(samples.len(), original.len());
        let mse = samples
            .iter()
            .zip(original)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            / samples.len() as f32;
        assert!(mse < 0.003, "{frames}: {mse}");
    }
}

#[test]
fn opus_surround_preserves_each_channel_and_rejects_incompatible_layouts() {
    for (channels, mask) in [
        (3, 0x7),
        (4, 0x33),
        (5, 0x37),
        (6, 0x3f),
        (7, 0x70f),
        (8, 0x63f),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("surround.wav");
        let original = tone(&source, 48000, channels, 48117, 0.25, Some(mask));
        let mut options = audio_options("opus");
        options.opus.bitrate = 512;
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
            assert!(mse < 0.003, "{channels}/{c}: {mse}");
        }
    }
    for (channels, mask) in [(4, 0x107), (6, 0x60f), (7, 0x13f), (8, 0xff)] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("layout.wav");
        tone(&source, 48000, channels, 4800, 0.2, Some(mask));
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        let json = serde_json::to_value(input).unwrap();
        assert!(!json["targets"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("opus")));
        assert!(audio_backend()
            .convert(
                &source,
                None,
                &audio_options("opus"),
                &AtomicBool::new(false)
            )
            .is_err());
    }
}

#[test]
fn opus_bitrate_choices_are_explicit_and_all_supported_targets_encode() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 48000, 2, 48117, 0.2, None);
    let mut options = audio_options("opus");
    let mut small = 0;
    let mut large = 0;
    for &bitrate in crate::audio_backend::OPUS_BITRATES {
        options.opus.bitrate = bitrate;
        let output = convert(&source, &options);
        if bitrate == 16 {
            small = output.bytes;
        }
        if bitrate == 512 {
            large = output.bytes;
        }
    }
    assert!(large > small * 2);
    options.opus.bitrate = 15;
    assert!(options.resolve(&inspect(source.to_str().unwrap())).is_err());
    tone(&source, 48000, 1, 4800, 0.2, None);
    options.opus.bitrate = 320;
    assert!(options.resolve(&inspect(source.to_str().unwrap())).is_err());
    options.opus.bitrate = 256;
    convert(&source, &options);
    no_partials(dir.path());
}

#[test]
fn opus_metadata_and_same_format_outputs_preserve_originals() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 48000, 2, 48117, 0.2, None);
    let tagged = dir.path().join("tags.flac");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args([
            "-c:a",
            "flac",
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
    let output = convert(&tagged, &audio_options("opus"));
    let tags = probe(&output.path);
    assert_eq!(tags["streams"][0]["tags"]["title"], "夜 = tone");
    assert!(tags["streams"][0]["tags"]["replaygain_track_gain"].is_null());
    let original = fs::read(&output.path).unwrap();
    let mut settings = audio_options("opus");
    settings.metadata = false;
    let again = convert(Path::new(&output.path), &settings);
    assert_ne!(again.path, output.path);
    assert_eq!(fs::read(&output.path).unwrap(), original);
    assert!(probe(&again.path)["streams"][0]["tags"]["title"].is_null());
    assert!(again
        .warnings
        .iter()
        .any(|w| w.contains("another lossy generation")));
    let flac = convert(Path::new(&output.path), &audio_options("flac"));
    assert_eq!(inspect(&flac.path).sample_format, "s24");
    let alac = convert(Path::new(&output.path), &audio_options("alac"));
    assert_eq!(inspect(&alac.path).sample_format, "s24");
    no_partials(dir.path());
}

fn pages(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut position = 0;
    let mut result = Vec::new();
    while position < bytes.len() {
        let header = position + 27 + bytes[position + 26] as usize;
        let size: usize = bytes[position + 27..header]
            .iter()
            .map(|b| *b as usize)
            .sum();
        result.push(position..header + size);
        position = header + size;
    }
    result
}

fn checksum(page: &mut [u8]) {
    page[22..26].fill(0);
    let sum = crate::ogg::crc(page);
    page[22..26].copy_from_slice(&sum.to_le_bytes());
}

#[test]
fn opus_corrupt_truncated_chained_and_unmapped_inputs_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 48000, 2, 144117, 0.2, None);
    let encoded = convert(&source, &audio_options("opus"));
    let bytes = fs::read(&encoded.path).unwrap();
    let ranges = pages(&bytes);
    let mut cases = vec![
        bytes[..bytes.len() - 1].to_vec(),
        bytes[..ranges.last().unwrap().start].to_vec(),
        [bytes.clone(), bytes.clone()].concat(),
    ];
    let mut corrupt = bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    cases.push(corrupt);
    let mut missing = bytes.clone();
    missing.drain(ranges[2].clone());
    cases.push(missing);
    let mut mapping = bytes.clone();
    mapping[28 + 18] = 255;
    checksum(&mut mapping[ranges[0].clone()]);
    cases.push(mapping);
    let mut timing = bytes.clone();
    let last = ranges.last().unwrap().clone();
    timing[last.start + 6..last.start + 14].copy_from_slice(&u64::MAX.to_le_bytes());
    checksum(&mut timing[last]);
    cases.push(timing);
    for (i, data) in cases.iter().enumerate() {
        let bad = dir.path().join(format!("bad-{i}.opus"));
        fs::write(&bad, data).unwrap();
        assert!(
            audio_backend()
                .convert(&bad, None, &audio_options("wav"), &AtomicBool::new(false))
                .is_err(),
            "{i}"
        );
        assert!(!bad.with_extension("wav").exists());
    }
    no_partials(dir.path());
}

#[test]
fn opus_mixed_batch_overload_and_retry_keep_completed_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hot.wav");
    tone(&source, 48000, 2, 48117, 1.4, None);
    let output = convert(&source, &audio_options("opus"));
    assert!(output.warnings.iter().any(|w| w.contains("full scale")));
    let float = convert(Path::new(&output.path), &audio_options("wav"));
    assert!(floats(&float.path).iter().any(|v| v.abs() > 1.2));
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[picture, PathBuf::from(&output.path)], dir.path());
    request.audio_options = Some(audio_options("flac"));
    let manager = JobManager::default();
    let job = manager.prepare("opus-hot", request.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert_eq!(before.files[0].status, FileStatus::Succeeded);
    assert!(before.files[1].can_allow_clipping);
    request.audio_options.as_mut().unwrap().allow_clipping = true;
    let job = manager.prepare("opus-hot", request, true).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let after = job.snapshot();
    assert_eq!(before.files[0].output_path, after.files[0].output_path);
    assert_eq!(after.files[1].status, FileStatus::Succeeded);
    assert!(after.files[1]
        .warnings
        .iter()
        .any(|w| w.contains("Clipping occurred")));
    no_partials(dir.path());
}

#[test]
fn opus_active_encoder_cancellation_cleans_up() {
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
        audio_backend().convert(&source, None, &audio_options("opus"), &signal)
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
}
