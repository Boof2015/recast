use super::*;
use crate::audio_backend::AudioOptions;

fn audio_options(target: &str) -> AudioOptions {
    AudioOptions {
        target: target.into(),
        sample_rate: None,
        sample_format: "source".into(),
        dither: true,
        metadata: true,
        allow_clipping: false,
    }
}

fn wav(
    path: &Path,
    bits: u16,
    float: bool,
    rate: u32,
    channels: u16,
    samples: &[u8],
    mask: Option<u32>,
) {
    let mut fmt = Vec::new();
    fmt.extend_from_slice(
        &(if mask.is_some() {
            65534u16
        } else if float {
            3
        } else {
            1
        })
        .to_le_bytes(),
    );
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
    fmt.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    fmt.extend_from_slice(&bits.to_le_bytes());
    if let Some(mask) = mask {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(&mask.to_le_bytes());
        fmt.extend_from_slice(&(if float { 3u32 } else { 1 }).to_le_bytes());
        fmt.extend_from_slice(&[0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
    }
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(20 + fmt.len() as u32 + samples.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&fmt);
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    bytes.extend_from_slice(samples);
    fs::write(path, bytes).unwrap();
}

fn pcm(path: &Path) -> Vec<u8> {
    // Independently inspect RIFF chunks, rather than use the encoder as an oracle.
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if &bytes[offset..offset + 4] == b"data" {
            return bytes[offset + 8..offset + 8 + size].to_vec();
        }
        offset += 8 + size + (size % 2);
    }
    panic!("Missing WAV data");
}

fn convert(source: &Path, options: &AudioOptions) -> crate::image_backend::OutputFile {
    audio_backend()
        .convert(source, None, options, &AtomicBool::new(false))
        .unwrap()
}

#[test]
fn audio_integer_roundtrips_are_sample_exact_without_silent_depth_reduction() {
    audio_backend().verify().unwrap();
    for bits in [16, 24, 32] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        let mut data = Vec::new();
        for index in 0..4096i32 {
            let value = index.wrapping_mul(104729).wrapping_mul(7919);
            data.extend_from_slice(&value.to_le_bytes()[..bits as usize / 8]);
        }
        wav(&source, bits, false, 96000, 2, &data, None);
        let original = fs::read(&source).unwrap();
        if bits == 32 {
            let output = convert(&source, &audio_options("wav"));
            assert_eq!(pcm(Path::new(&output.path)), data);
            assert!(audio_backend()
                .convert(
                    &source,
                    None,
                    &audio_options("flac"),
                    &AtomicBool::new(false)
                )
                .is_err());
            continue;
        }
        let flac = convert(&source, &audio_options("flac"));
        let back = convert(Path::new(&flac.path), &audio_options("wav"));
        assert_eq!(pcm(Path::new(&back.path)), data, "{bits}-bit round trip");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(back.path.ends_with("source (1).wav"));
        assert!(flac.details.unwrap().ends_with("Dither off"));
        no_partials(dir.path());
    }
}

#[test]
fn audio_float_and_eight_bit_wav_preserve_samples_and_require_explicit_flac_depth() {
    for bits in [8, 32, 64] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("not an image.png");
        let samples: Vec<u8> = if bits == 8 {
            (0..=255u8).collect()
        } else if bits == 32 {
            [-1.25f32, -0.1, 0.0, 0.125, 1.25]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect()
        } else {
            [-1.25f64, -0.123456789, 0.0, 0.125, 1.25]
                .into_iter()
                .flat_map(f64::to_le_bytes)
                .collect()
        };
        wav(&source, bits, bits != 8, 44100, 1, &samples, None);
        let inspected =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        assert_eq!(inspected.kind, "audio");
        let output = convert(&source, &audio_options("wav"));
        assert_eq!(pcm(Path::new(&output.path)), samples);
        assert!(audio_options("flac")
            .resolve(inspected.audio.as_ref().unwrap())
            .is_err());
        no_partials(dir.path());
    }
}

#[test]
fn audio_clipping_requires_consent_and_reports_when_used() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("hot.wav");
    let samples: Vec<_> = [-1.5f32, -0.25, 0.0, 0.25, 1.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    wav(&source, 32, true, 48000, 1, &samples, None);
    let original = fs::read(&source).unwrap();
    let mut options = audio_options("wav");
    options.sample_format = "s16".into();
    options.dither = false;
    let error = audio_backend()
        .convert(&source, None, &options, &AtomicBool::new(false))
        .unwrap_err();
    assert!(matches!(error, ConversionError::Clipping));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    options.allow_clipping = true;
    let output = convert(&source, &options);
    assert_eq!(output.warnings.len(), 1);
    let values: Vec<_> = pcm(Path::new(&output.path))
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| i16::from_le_bytes(*s))
        .collect();
    assert_eq!(values, vec![i16::MIN, -8192, 0, 8192, i16::MAX]);
    assert_eq!(fs::read(source).unwrap(), original);
    no_partials(dir.path());
}

#[test]
fn audio_dither_is_applied_only_at_required_integer_quantization() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("silence.wav");
    wav(&source, 32, true, 48000, 1, &vec![0; 48000 * 4], None);
    let mut options = audio_options("wav");
    options.sample_format = "s16".into();
    let dithered = convert(&source, &options);
    assert!(pcm(Path::new(&dithered.path)).iter().any(|b| *b != 0));
    assert!(dithered.details.unwrap().ends_with("Dither applied"));
    options.dither = false;
    let plain = convert(&source, &options);
    assert!(pcm(Path::new(&plain.path)).iter().all(|b| *b == 0));
    let integer = dir.path().join("integer.wav");
    wav(&integer, 16, false, 48000, 1, &vec![0; 48000 * 2], None);
    options.sample_format = "s24".into();
    options.dither = true;
    let wider = convert(&integer, &options);
    assert!(pcm(Path::new(&wider.path)).iter().all(|b| *b == 0));
    assert!(wider.details.unwrap().ends_with("Dither off"));
}

#[test]
fn audio_resampling_retains_duration_and_recognized_surround_layout() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("surround.wav");
    let samples: Vec<u8> = (0..4800)
        .flat_map(|i| (0..6).flat_map(move |c| ((i * (c + 1)) as i16).to_le_bytes()))
        .collect();
    wav(&source, 16, false, 48000, 6, &samples, Some(0x3f));
    let info = audio_backend()
        .inspect(&source, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(info.channel_layout.as_deref(), Some("5.1"));
    let mut options = audio_options("wav");
    options.sample_rate = Some(44100);
    let output = convert(&source, &options);
    let result = audio_backend()
        .inspect(Path::new(&output.path), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(result.sample_rate, 44100);
    assert_eq!(result.channel_layout, info.channel_layout);
    assert_eq!(pcm(Path::new(&output.path)).len(), 4410 * 6 * 2);
    assert!(output.details.unwrap().ends_with("Dither applied"));
    options.sample_rate = None;
    options.target = "flac".into();
    let flac = convert(&source, &options);
    let back = convert(Path::new(&flac.path), &audio_options("wav"));
    assert_eq!(pcm(Path::new(&back.path)), samples);
}

#[test]
fn audio_mixed_batch_and_clipping_retry_keep_successful_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let image = copy("rgba.png", &dir.path().join("picture.png"));
    let audio = dir.path().join("audio.wav");
    wav(
        &audio,
        32,
        true,
        48000,
        1,
        &1.5f32.to_le_bytes().repeat(4800),
        None,
    );
    let mut request = request(&[image, audio], dir.path());
    let mut settings = audio_options("flac");
    settings.sample_format = "s24".into();
    request.audio_options = Some(settings);
    let manager = JobManager::default();
    let job = manager.prepare("mixed", request.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    assert_eq!(job.snapshot().files[0].status, FileStatus::Succeeded);
    assert_eq!(job.snapshot().files[1].status, FileStatus::Failed);
    assert!(job.snapshot().files[1].can_allow_clipping);
    request.audio_options.as_mut().unwrap().allow_clipping = true;
    let retry = manager.prepare("mixed", request, true).unwrap();
    run_job(&retry, &backend(), &audio_backend(), |_| {});
    assert!(
        retry
            .snapshot()
            .files
            .iter()
            .all(|f| f.status == FileStatus::Succeeded),
        "{:?}",
        retry.snapshot()
    );
    assert_eq!(retry.snapshot().files[1].warnings.len(), 1);
    assert!(!retry.snapshot().files[1].can_allow_clipping);
    assert!(!dir.path().join("picture (1).webp").exists());
    no_partials(dir.path());
}

#[test]
fn audio_invalid_samples_truncation_and_cancel_do_not_publish_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("nan.wav");
    wav(&source, 32, true, 48000, 1, &f32::NAN.to_le_bytes(), None);
    let mut options = audio_options("wav");
    options.sample_format = "s24".into();
    options.allow_clipping = true;
    assert!(audio_backend()
        .convert(&source, None, &options, &AtomicBool::new(false))
        .is_err());
    let truncated = dir.path().join("truncated.wav");
    wav(&truncated, 16, false, 48000, 1, &vec![1; 9600], None);
    let mut data = fs::read(&truncated).unwrap();
    data.truncate(data.len() - 500);
    fs::write(&truncated, data).unwrap();
    assert!(audio_backend()
        .convert(
            &truncated,
            None,
            &audio_options("flac"),
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(matches!(
        audio_backend().convert(&source, None, &options, &AtomicBool::new(true)),
        Err(ConversionError::Cancelled)
    ));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    no_partials(dir.path());
}

#[test]
fn audio_sample_rates_remain_individual_in_one_batch() {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = vec![];
    for (rate, bits) in [(44100, 16), (96000, 24)] {
        let path = dir.path().join(format!("{rate}.wav"));
        wav(
            &path,
            bits,
            false,
            rate,
            1,
            &vec![0; 1200 * bits as usize / 8],
            None,
        );
        paths.push(path);
    }
    let mut request = request(&paths, dir.path());
    request.image_options = None;
    request.audio_options = Some(audio_options("flac"));
    let manager = JobManager::default();
    let job = manager.prepare("audio", request, false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    for (index, rate) in [44100, 96000].into_iter().enumerate() {
        let result = &job.snapshot().files[index];
        assert_eq!(result.status, FileStatus::Succeeded, "{:?}", result.error);
        let info = audio_backend()
            .inspect(
                Path::new(result.output_path.as_ref().unwrap()),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(info.sample_rate, rate);
    }
}

#[test]
fn audio_retains_ordinary_tags_when_requested_and_drops_stale_loudness_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    wav(&source, 16, false, 48000, 1, &vec![0; 9600], None);
    let tagged = dir.path().join("tagged.flac");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args([
            "-c:a",
            "flac",
            "-metadata",
            "title=Title = with spaces",
            "-metadata",
            "artist=Recast fixture",
            "-metadata",
            "replaygain_track_peak=1.7",
        ])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let probe = |path: &Path| -> serde_json::Value {
        let result = audio_backend()
            .command("ffprobe")
            .args(["-v", "error", "-show_entries", "format_tags", "-of", "json"])
            .arg(path)
            .output()
            .unwrap();
        serde_json::from_slice(&result.stdout).unwrap()
    };
    let mut options = audio_options("flac");
    options.sample_rate = Some(44100);
    let kept = convert(&tagged, &options);
    let tags = probe(Path::new(&kept.path));
    assert_eq!(tags["format"]["tags"]["title"], "Title = with spaces");
    assert_eq!(tags["format"]["tags"]["artist"], "Recast fixture");
    assert!(tags["format"]["tags"]["replaygain_track_peak"].is_null());
    options.metadata = false;
    let stripped = convert(&tagged, &options);
    assert!(probe(Path::new(&stripped.path))["format"]["tags"]["title"].is_null());
}

#[test]
fn audio_cancellation_stops_the_worker_and_cleans_its_temporary_output() {
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
        audio_backend().convert(&source, None, &audio_options("flac"), &signal)
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
fn alac_apple_encoded_fixtures_decode_to_exact_independent_samples() {
    for bits in [16, 24] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("apple.m4a");
        fs::copy(fixture(&format!("apple-{bits}.m4a")), &source).unwrap();
        let info = audio_backend()
            .inspect(&source, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(info.codec, "alac");
        assert_eq!(info.sample_rate, 48000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_format, format!("s{bits}"));
        let expected: Vec<u8> = (0..5003 * 2i32)
            .flat_map(|i| {
                i.wrapping_mul(104729).wrapping_mul(7919).to_le_bytes()[..bits / 8].to_vec()
            })
            .collect();
        let output = convert(&source, &audio_options("wav"));
        assert_eq!(pcm(Path::new(&output.path)), expected);
        no_partials(dir.path());
    }
}

#[test]
fn alac_unsupported_depth_and_aac_are_explained_before_conversion() {
    for (name, message) in [
        ("apple-32.m4a", "16-bit and 24-bit ALAC"),
        ("unsupported-aac.m4a", "AAC and other codecs"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join(name);
        fs::copy(fixture(name), &source).unwrap();
        let inspected =
            crate::inputs::inspect_with_audio(&source, &audio_backend(), &AtomicBool::new(false))
                .unwrap();
        let inspected = serde_json::to_value(inspected).unwrap();
        assert!(inspected["targets"].as_array().unwrap().is_empty());
        assert!(inspected["conversionIssue"]
            .as_str()
            .unwrap()
            .contains(message));
        let error = audio_backend()
            .convert(
                &source,
                None,
                &audio_options("wav"),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert!(matches!(error, ConversionError::Failed(ref text) if text.contains(message)));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        no_partials(dir.path());
    }
}

#[test]
fn alac_roundtrips_preserve_pcm_samples_rates_and_m4a_names() {
    audio_backend().verify().unwrap();
    for (bits, rate, channels) in [(16, 44100, 1), (24, 96000, 2), (24, 192000, 2)] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.wav");
        let data: Vec<u8> = (0..5003i32 * channels as i32)
            .flat_map(|i| {
                i.wrapping_mul(104729).wrapping_mul(7919).to_le_bytes()[..bits as usize / 8]
                    .to_vec()
            })
            .collect();
        wav(&source, bits, false, rate, channels, &data, None);
        let original = fs::read(&source).unwrap();
        let alac = convert(&source, &audio_options("alac"));
        assert!(alac.path.ends_with("source.m4a"));
        let info = crate::inputs::inspect_with_audio(
            Path::new(&alac.path),
            &audio_backend(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(info.kind, "audio");
        assert_eq!(info.audio.as_ref().unwrap().codec, "alac");
        assert_eq!(info.audio.as_ref().unwrap().sample_rate, rate);
        assert_eq!(
            info.audio.as_ref().unwrap().sample_format,
            format!("s{bits}")
        );
        let same = convert(Path::new(&alac.path), &audio_options("alac"));
        assert!(same.path.ends_with("source (1).m4a"));
        let flac = convert(Path::new(&same.path), &audio_options("flac"));
        let alac_again = convert(Path::new(&flac.path), &audio_options("alac"));
        let back = convert(Path::new(&alac_again.path), &audio_options("wav"));
        assert_eq!(pcm(Path::new(&back.path)), data);
        assert_eq!(fs::read(&source).unwrap(), original);
        no_partials(dir.path());
    }
}

#[test]
fn alac_supported_surround_layouts_preserve_every_channel() {
    for (channels, mask) in [
        (3, 0x7),
        (4, 0x107),
        (5, 0x37),
        (6, 0x3f),
        (7, 0x13f),
        (8, 0xff),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("surround.wav");
        let data: Vec<u8> = (0..5003)
            .flat_map(|i| (0..channels).flat_map(move |c| ((i * (c + 1)) as i16).to_le_bytes()))
            .collect();
        wav(&source, 16, false, 48000, channels, &data, Some(mask));
        let before = audio_backend()
            .inspect(&source, &AtomicBool::new(false))
            .unwrap();
        let alac = convert(&source, &audio_options("alac"));
        let after = audio_backend()
            .inspect(Path::new(&alac.path), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(after.channel_layout, before.channel_layout);
        let back = convert(Path::new(&alac.path), &audio_options("wav"));
        assert_eq!(pcm(Path::new(&back.path)), data, "{channels} channels");
    }
}

#[test]
fn alac_incompatible_depth_rate_and_layout_require_deliberate_choices() {
    let dir = tempfile::tempdir().unwrap();
    for (bits, float) in [(8, false), (32, false), (32, true), (64, true)] {
        let path = dir.path().join(format!("source-{bits}-{float}.wav"));
        wav(
            &path,
            bits,
            float,
            48000,
            1,
            &vec![0; 4800 * bits as usize / 8],
            None,
        );
        let error = audio_backend()
            .convert(&path, None, &audio_options("alac"), &AtomicBool::new(false))
            .unwrap_err();
        assert!(
            matches!(error, ConversionError::Failed(message) if message.contains("Choose one explicitly"))
        );
    }
    let path = dir.path().join("high-rate.wav");
    wav(&path, 16, false, 768000, 1, &vec![0; 9600], None);
    let info = audio_backend()
        .inspect(&path, &AtomicBool::new(false))
        .unwrap();
    assert!(audio_options("alac")
        .resolve(&info)
        .unwrap_err()
        .contains("384000"));
    let mut settings = audio_options("alac");
    settings.sample_rate = Some(192000);
    assert!(settings.resolve(&info).is_ok());
    let side = dir.path().join("side-surround.wav");
    wav(
        &side,
        16,
        false,
        48000,
        6,
        &vec![0; 4800 * 6 * 2],
        Some(0x60f),
    );
    let inspected =
        crate::inputs::inspect_with_audio(&side, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    let json = serde_json::to_value(&inspected).unwrap();
    assert!(!json["targets"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("alac")));
    assert!(json["targetIssues"]["alac"]
        .as_str()
        .unwrap()
        .contains("channel layout"));
    assert!(inspected
        .require_audio_target(&audio_options("alac"))
        .is_err());
    assert!(audio_backend()
        .convert(&side, None, &audio_options("alac"), &AtomicBool::new(false))
        .is_err());
    assert!(!dir.path().join("side-surround.m4a").exists());
    no_partials(dir.path());
}

#[test]
fn alac_generic_mp4_content_is_audio_even_with_an_unrelated_extension() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    let data: Vec<u8> = (0..4800i16).flat_map(i16::to_le_bytes).collect();
    wav(&source, 16, false, 48000, 1, &data, None);
    let output = convert(&source, &audio_options("alac"));
    let mut bytes = fs::read(&output.path).unwrap();
    assert_eq!(&bytes[4..8], b"ftyp");
    bytes[8..12].copy_from_slice(b"isom");
    let disguised = dir.path().join("actually audio.png");
    fs::write(&disguised, bytes).unwrap();
    let info =
        crate::inputs::inspect_with_audio(&disguised, &audio_backend(), &AtomicBool::new(false))
            .unwrap();
    assert_eq!(info.kind, "audio");
    assert_eq!(serde_json::to_value(&info).unwrap()["format"], "ALAC");
    let mut request = request(&[disguised], dir.path());
    request.image_options = None;
    request.audio_options = Some(audio_options("wav"));
    let job = JobManager::default()
        .prepare("alac", request, false)
        .unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let snapshot = job.snapshot();
    assert_eq!(
        snapshot.files[0].status,
        FileStatus::Succeeded,
        "{:?}",
        snapshot
    );
    assert_eq!(
        pcm(Path::new(snapshot.files[0].output_path.as_ref().unwrap())),
        data
    );
}

#[test]
fn alac_metadata_is_retained_or_stripped_through_m4a() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    wav(&source, 16, false, 48000, 1, &vec![0; 9600], None);
    let tagged = dir.path().join("tagged.m4a");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args([
            "-c:a",
            "alac",
            "-metadata",
            "title=ALAC title",
            "-metadata",
            "artist=Recast fixture",
            "-metadata",
            "album=Album",
            "-metadata",
            "track=3/10",
            "-metadata",
            "disc=1/2",
            "-f",
            "ipod",
        ])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for keep in [true, false] {
        let mut options = audio_options("alac");
        options.metadata = keep;
        let out = convert(&tagged, &options);
        let probe = audio_backend()
            .command("ffprobe")
            .args(["-v", "error", "-show_entries", "format_tags", "-of", "json"])
            .arg(&out.path)
            .output()
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        for (tag, value) in [
            ("title", "ALAC title"),
            ("artist", "Recast fixture"),
            ("album", "Album"),
            ("track", "3/10"),
            ("disc", "1/2"),
        ] {
            if keep {
                assert_eq!(json["format"]["tags"][tag], value);
            } else {
                assert!(json["format"]["tags"][tag].is_null());
            }
        }
    }
}

#[test]
fn alac_multiple_tracks_and_truncated_payloads_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.wav");
    let data: Vec<_> = (0..10000i32)
        .flat_map(|n| (n.wrapping_mul(104729) as i16).to_le_bytes())
        .collect();
    wav(&source, 16, false, 48000, 1, &data, None);
    let two = dir.path().join("two.m4a");
    let result = audio_backend()
        .command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&source)
        .args(["-map", "0:a", "-map", "0:a", "-c:a", "alac", "-f", "ipod"])
        .arg(&two)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let issue = audio_backend()
        .inspect(&two, &AtomicBool::new(false))
        .unwrap_err();
    assert!(
        matches!(issue, ConversionError::Failed(message) if message.contains("one audio stream"))
    );
    assert!(audio_backend()
        .convert(&two, None, &audio_options("wav"), &AtomicBool::new(false))
        .is_err());
    let good = convert(&source, &audio_options("alac"));
    let mut bytes = fs::read(&good.path).unwrap();
    let mdat = bytes.windows(4).position(|b| b == b"mdat").unwrap();
    bytes.truncate(mdat + (bytes.len() - mdat) / 2);
    let corrupt = dir.path().join("truncated.m4a");
    fs::write(&corrupt, bytes).unwrap();
    assert!(audio_backend()
        .convert(
            &corrupt,
            None,
            &audio_options("wav"),
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(!dir.path().join("two.wav").exists());
    assert!(!dir.path().join("truncated.wav").exists());
    no_partials(dir.path());
}

#[test]
fn alac_float_conversion_obeys_clipping_and_resampling_choices() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("float.wav");
    let data: Vec<_> = [-1.5f32, -0.25, 0.0, 0.25, 1.5]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    wav(&source, 32, true, 48000, 1, &data.repeat(960), None);
    let mut options = audio_options("alac");
    options.sample_format = "s16".into();
    options.dither = false;
    assert!(matches!(
        audio_backend().convert(&source, None, &options, &AtomicBool::new(false)),
        Err(ConversionError::Clipping)
    ));
    options.allow_clipping = true;
    let output = convert(&source, &options);
    assert_eq!(output.warnings.len(), 1);
    let back = convert(Path::new(&output.path), &audio_options("wav"));
    let expected: Vec<_> = [i16::MIN, -8192, 0, 8192, i16::MAX]
        .into_iter()
        .flat_map(i16::to_le_bytes)
        .collect();
    assert_eq!(pcm(Path::new(&back.path)), expected.repeat(960));
    options.sample_rate = Some(44100);
    options.dither = true;
    let resampled = convert(&source, &options);
    let back = convert(Path::new(&resampled.path), &audio_options("wav"));
    assert_eq!(pcm(Path::new(&back.path)).len(), 4410 * 2);
    assert!(resampled.details.unwrap().ends_with("Dither applied"));
    no_partials(dir.path());
}
