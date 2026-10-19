use super::aac::{floats, inspect, probe, tone};
use super::*;

#[test]
fn vorbis_independent_xiph_inputs_preserve_length_alignment_and_lossy_defaults() {
    for (name, frames) in [("xiph-vorbis.ogg", 44117), ("xiph-vorbis-short.ogg", 117)] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join("audio.png"));
        let input =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(input.kind, "audio");
        assert_eq!(serde_json::to_value(&input).unwrap()["format"], "VORBIS");
        assert_eq!(input.audio.unwrap().sample_format, "lossy");
        let output = convert(&source, &audio_options("wav"));
        assert_eq!(inspect(&output.path).sample_format, "f32");
        let samples = floats(&output.path);
        assert_eq!(samples.len(), frames * 2);
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
                        let reference = (7000.0
                            * (i as f64 * f * std::f64::consts::TAU / 44100.0).sin())
                        .trunc()
                            / 32768.0;
                        (f64::from(pair[c]) - reference).powi(2)
                    })
                    .sum::<f64>()
            })
            .sum::<f64>()
            / samples.len() as f64;
        assert!(mse < 0.002, "{name}: {mse}");
        assert!(output
            .warnings
            .iter()
            .any(|w| w.contains("does not restore")));
        for target in ["flac", "alac"] {
            let lossless = convert(&source, &audio_options(target));
            assert_eq!(inspect(&lossless.path).sample_format, "s24");
        }
        no_partials(dir.path());
    }
}

#[test]
fn vorbis_supported_rates_and_single_page_timing_keep_exact_audio_lengths() {
    audio_backend().verify().unwrap();
    for rate in [
        7350, 8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000, 64000, 88200, 96000,
        176400, 192000,
    ] {
        for channels in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("tone.wav");
            let original = tone(&source, rate, channels, rate as usize + 117, 0.2, None);
            let output = convert(&source, &audio_options("ogg"));
            assert_eq!(inspect(&output.path).sample_rate, rate);
            assert_eq!(inspect(&output.path).codec, "vorbis");
            assert!(output.path.ends_with(".ogg"));
            let back = convert(Path::new(&output.path), &audio_options("wav"));
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
    // Without the checked demuxer patch, 44117 becomes 43989, while 117
    // becomes 128 and a one-frame stream decodes hundreds of padding samples.
    for frames in [1, 117, 127, 128, 129, 647, 1024, 44117, 88117] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("short.wav");
        tone(&source, 44100, 2, frames, 0.2, None);
        let output = convert(&source, &audio_options("ogg"));
        let back = convert(Path::new(&output.path), &audio_options("wav"));
        assert_eq!(floats(&back.path).len(), frames * 2, "{frames}");
    }
}

#[test]
fn vorbis_quality_rate_choices_and_remembered_pcm_settings_are_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("tone.wav");
    tone(&source, 44100, 2, 44117, 0.2, None);
    let mut options = audio_options("ogg");
    options.sample_format = "u8".into();
    let mut sizes = Vec::new();
    for quality in -1..=10 {
        options.vorbis.quality = quality;
        let output = convert(&source, &options);
        assert!(output
            .details
            .unwrap()
            .contains(&format!("quality {quality}")));
        sizes.push(output.bytes);
    }
    assert!(sizes[11] > sizes[0]);
    for quality in [-2, 11] {
        options.vorbis.quality = quality;
        assert!(options.resolve(&inspect(source.to_str().unwrap())).is_err());
    }
    options.vorbis.quality = 5;
    tone(&source, 384000, 1, 38400, 0.2, None);
    assert!(options.resolve(&inspect(source.to_str().unwrap())).is_err());
    options.sample_rate = Some(48000);
    let output = convert(&source, &options);
    assert_eq!(inspect(&output.path).sample_rate, 48000);
    let back = convert(Path::new(&output.path), &audio_options("wav"));
    assert_eq!(floats(&back.path).len(), 4800);
    no_partials(dir.path());
}

#[test]
fn vorbis_surround_preserves_channel_order_and_blocks_incompatible_layouts() {
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
        let original = tone(&source, 48000, channels, 48117, 0.2, Some(mask));
        let output = convert(&source, &audio_options("ogg"));
        assert_eq!(
            inspect(&output.path).channel_layout,
            inspect(source.to_str().unwrap()).channel_layout
        );
        let back = convert(Path::new(&output.path), &audio_options("wav"));
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
            .contains(&serde_json::json!("ogg")));
        assert!(audio_backend()
            .convert(
                &source,
                None,
                &audio_options("ogg"),
                &AtomicBool::new(false)
            )
            .is_err());
    }
}

#[test]
fn vorbis_metadata_names_cross_conversions_and_mixed_batches_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("xiph-vorbis.ogg", &dir.path().join("source.ogg"));
    let tagged = dir.path().join("tags.ogg");
    let status = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args([
            "-c:a",
            "copy",
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
    assert!(status.status.success());
    let output = convert(&tagged, &audio_options("ogg"));
    let tags = probe(&output.path);
    assert_eq!(tags["streams"][0]["tags"]["title"], "夜 = tone");
    assert_eq!(tags["streams"][0]["tags"]["track"], "2/7");
    assert!(tags["streams"][0]["tags"]["replaygain_track_gain"].is_null());
    let original = fs::read(&output.path).unwrap();
    let mut options = audio_options("ogg");
    options.metadata = false;
    let again = convert(Path::new(&output.path), &options);
    assert_ne!(again.path, output.path);
    assert_eq!(fs::read(&output.path).unwrap(), original);
    assert!(probe(&again.path)["streams"][0]["tags"]["title"].is_null());
    assert!(again
        .warnings
        .iter()
        .any(|w| w.contains("another lossy generation")));
    for target in ["wav", "flac", "alac", "mp3", "m4a", "opus"] {
        convert(&source, &audio_options(target));
    }
    for name in [
        "lame-vbr.mp3",
        "apple-aac-lc.m4a",
        "reference-opus.opus",
        "apple-16.m4a",
    ] {
        let input = copy(name, &dir.path().join(name));
        convert(&input, &audio_options("ogg"));
    }
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[source, tagged, picture], dir.path());
    request.audio_options = Some(audio_options("ogg"));
    let manager = JobManager::default();
    let job = manager.prepare("vorbis-mixed", request, false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let result = job.snapshot();
    assert_eq!(result.files.len(), 3);
    for file in result.files {
        assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
    }
    no_partials(dir.path());
}

#[test]
fn vorbis_corrupt_chained_and_truncated_ogg_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fs::read(fixture("xiph-vorbis.ogg")).unwrap();
    let mut cases = vec![
        bytes[..bytes.len() - 1].to_vec(),
        bytes[..bytes.len() / 2].to_vec(),
        [bytes.clone(), bytes.clone()].concat(),
    ];
    let mut corrupt = bytes.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    cases.push(corrupt);
    let mut offset = 0;
    let mut last = 0;
    while offset < bytes.len() {
        last = offset;
        let count = bytes[offset + 26] as usize;
        offset += 27
            + count
            + bytes[offset + 27..offset + 27 + count]
                .iter()
                .map(|b| *b as usize)
                .sum::<usize>();
    }
    for frames in [9999999u64, i64::MAX as u64, u64::MAX - 1] {
        let mut wrong_length = bytes.clone();
        wrong_length[last + 6..last + 14].copy_from_slice(&frames.to_le_bytes());
        wrong_length[last + 22..last + 26].fill(0);
        let sum = crate::ogg::crc(&wrong_length[last..]);
        wrong_length[last + 22..last + 26].copy_from_slice(&sum.to_le_bytes());
        cases.push(wrong_length);
    }
    let mut missing = bytes.clone();
    missing[last + 18..last + 22].copy_from_slice(&20u32.to_le_bytes());
    missing[last + 22..last + 26].fill(0);
    let sum = crate::ogg::crc(&missing[last..]);
    missing[last + 22..last + 26].copy_from_slice(&sum.to_le_bytes());
    cases.push(missing);
    for (i, data) in cases.iter().enumerate() {
        let bad = dir.path().join(format!("bad-{i}.ogg"));
        fs::write(&bad, data).unwrap();
        for rate in [None, Some(48000)] {
            let mut options = audio_options("wav");
            options.sample_rate = rate;
            assert!(
                audio_backend()
                    .convert(&bad, None, &options, &AtomicBool::new(false))
                    .is_err(),
                "{i}"
            );
        }
        assert!(!bad.with_extension("wav").exists());
    }
    no_partials(dir.path());
}

#[test]
fn vorbis_overload_retry_and_active_cancellation_keep_batch_protections() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hot.wav");
    tone(&source, 48000, 2, 48117, 1.4, None);
    let output = convert(&source, &audio_options("ogg"));
    assert!(output.warnings.iter().any(|w| w.contains("full scale")));
    let float = convert(Path::new(&output.path), &audio_options("wav"));
    assert!(floats(&float.path).iter().any(|v| v.abs() > 1.2));
    let picture = copy("rgb.png", &dir.path().join("picture.png"));
    let mut request = request(&[picture, PathBuf::from(&output.path)], dir.path());
    request.audio_options = Some(audio_options("flac"));
    let manager = JobManager::default();
    let job = manager
        .prepare("vorbis-hot", request.clone(), false)
        .unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert_eq!(before.files[0].status, FileStatus::Succeeded);
    assert!(before.files[1].can_allow_clipping);
    request.audio_options.as_mut().unwrap().allow_clipping = true;
    let job = manager.prepare("vorbis-hot", request, true).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let after = job.snapshot();
    assert_eq!(after.files[0].output_path, before.files[0].output_path);
    assert_eq!(after.files[1].status, FileStatus::Succeeded);
    let long = dir.path().join("long.wav");
    wav(&long, 16, false, 48000, 2, &vec![0; 16 * 1024 * 1024], None);
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let worker =
        thread::spawn(move || audio_backend().convert(&long, None, &audio_options("ogg"), &signal));
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
    no_partials(dir.path());
}
