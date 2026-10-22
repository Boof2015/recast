use super::*;
use crate::animation::{inspect_gif, inspect_webp, Sequence};
fn sequence(path: &Path) -> Sequence {
    let mut file = fs::File::open(path).unwrap();
    if path.extension().unwrap() == "gif" {
        inspect_gif(&mut file).unwrap()
    } else {
        inspect_webp(&mut file).unwrap().unwrap()
    }
}
fn convert(source: &Path, settings: &ImageOptions) -> PathBuf {
    PathBuf::from(
        backend()
            .convert(source, None, settings, &AtomicBool::new(false))
            .unwrap()
            .path,
    )
}
#[test]
fn animation_targets_preserve_motion_and_explain_incompatible_formats() {
    for name in [
        "disposal.gif",
        "partial.webp",
        "millisecond.webp",
        "animated.webp",
    ] {
        let input = serde_json::to_value(crate::inputs::inspect(&fixture(name)).unwrap()).unwrap();
        assert_eq!(
            input["targets"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|target| !target.as_str().unwrap().starts_with("frame-"))
                .cloned()
                .collect::<Vec<_>>(),
            serde_json::json!(["webp", "gif"])
                .as_array()
                .unwrap()
                .clone(),
            "{name}: {input}"
        );
        assert_eq!(input["animated"], true);
        assert!(input["targetIssues"]["png"]
            .as_str()
            .unwrap()
            .contains("animated"));
    }
    for (name, targets) in [("fast.webp", vec!["webp"]), ("max-loops.gif", vec!["gif"])] {
        let input = serde_json::to_value(crate::inputs::inspect(&fixture(name)).unwrap()).unwrap();
        assert_eq!(
            input["targets"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|target| !target.as_str().unwrap().starts_with("frame-"))
                .cloned()
                .collect::<Vec<_>>(),
            serde_json::json!(targets).as_array().unwrap().clone(),
            "{input}"
        );
    }
}
#[test]
fn animation_frame_timing_loops_dimensions_and_sources_survive_conversion() {
    for name in [
        "disposal.gif",
        "forever.gif",
        "once.gif",
        "identical.gif",
        "partial.webp",
        "millisecond.webp",
        "fast.webp",
        "long-delay.webp",
        "max-loops.gif",
    ] {
        for target in ["gif", "webp"] {
            let dir = tempfile::tempdir().unwrap();
            let source = copy(name, &dir.path().join(name));
            let original = fs::read(&source).unwrap();
            let expected = sequence(&source);
            for resize in [100, 50] {
                let mut settings = options();
                settings.target = target.into();
                settings.resize = resize;
                settings.lossless = true;
                settings.metadata = false;
                if expected.target_issue(target).is_some() {
                    assert!(backend()
                        .convert(&source, None, &settings, &AtomicBool::new(false))
                        .is_err());
                    continue;
                }
                let output = convert(&source, &settings);
                let actual = sequence(&output);
                assert_eq!(actual.loops, expected.loops, "{name} -> {target}");
                assert_eq!(
                    actual.delays,
                    if target == "gif" {
                        expected.gif_delays()
                    } else {
                        expected.delays.clone()
                    },
                    "{name} -> {target}"
                );
                assert_eq!(actual.width, expected.width * u32::from(resize) / 100);
                assert_eq!(actual.height, expected.height * u32::from(resize) / 100);
                assert_eq!(fs::read(&source).unwrap(), original);
                no_partials(dir.path());
            }
        }
    }
}

// Read BMP bytes rather than use the worker's textual color formatting. The
// independent fixture generator supplies expected fully composed GIF canvases.
fn pixels(path: &Path, frame: usize) -> Vec<u8> {
    let frames = crate::inputs::inspect(path)
        .unwrap()
        .sequence
        .map(|s| s.delays.len())
        .unwrap_or(1);
    let mut channels = vec![];
    for alpha in ["off", "extract"] {
        let result = backend()
            .command()
            .arg(path)
            .args(["-alpha", "set", "-coalesce"])
            .args([
                "-clone",
                &frame.to_string(),
                "-delete",
                &format!("0-{}", frames - 1),
                "-alpha",
                alpha,
                "-type",
                "TrueColor",
                "-depth",
                "8",
                "-define",
                "bmp:format=bmp3",
                "BMP:-",
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = result.stdout;
        let width = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
        let height = u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize;
        let stride = (width * 3).div_ceil(4) * 4;
        assert_eq!(&bytes[..2], b"BM");
        let mut channel = vec![];
        for y in (0..height).rev() {
            for x in 0..width {
                let at = 54 + y * stride + x * 3;
                channel.extend([bytes[at + 2], bytes[at + 1], bytes[at]]);
            }
        }
        channels.push(channel);
    }
    channels[0]
        .as_chunks::<3>()
        .0
        .iter()
        .zip(channels[1].as_chunks::<3>().0.iter())
        .flat_map(|(rgb, a)| {
            if a[0] == 0 {
                [0, 0, 0, 0]
            } else {
                [rgb[0], rgb[1], rgb[2], a[0]]
            }
        })
        .collect()
}

#[test]
fn gif_disposal_previous_background_and_transparency_match_reference_canvases() {
    let expected: Vec<Vec<u8>> =
        serde_json::from_slice(&fs::read(fixture("disposal-frames.json")).unwrap()).unwrap();
    for target in ["gif", "webp"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy("disposal.gif", &dir.path().join("source.gif"));
        let mut settings = options();
        settings.target = target.into();
        settings.lossless = true;
        let output = convert(&source, &settings);
        for (i, expected) in expected.iter().enumerate() {
            assert_eq!(pixels(&source, i), *expected, "source frame {i}");
            assert_eq!(pixels(&output, i), *expected, "{target} frame {i}");
        }
    }
}

#[test]
fn partial_webp_blend_and_disposal_survive_lossless_reencoding() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("partial.webp", &dir.path().join("source.webp"));
    // An opaque first subframe must not turn the unused canvas opaque black.
    assert_eq!(&pixels(&source, 0)[..4], &[0, 0, 0, 0]);
    let mut settings = options();
    settings.lossless = true;
    let output = convert(&source, &settings);
    for frame in 0..4 {
        let expected = pixels(&source, frame);
        let actual = pixels(&output, frame);
        let differences: Vec<_> = expected
            .iter()
            .zip(&actual)
            .enumerate()
            .filter(|(i, (a, b))| a.abs_diff(**b) > if i % 4 == 3 { 0 } else { 1 })
            .take(8)
            .collect();
        assert!(differences.is_empty(), "frame {frame}: {differences:?}");
    }
}

#[test]
fn gif_color_controls_cutout_alpha_and_interlacing() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.target = "gif".into();
    let mut outputs = vec![];
    for colors in [32, 64, 128, 256] {
        for dither in [true, false] {
            settings.colors = colors;
            settings.dither = dither;
            let output = convert(&source, &settings);
            let unique: u16 = identify(&output, "%k").parse().unwrap();
            assert!(unique <= colors, "{unique} > {colors}");
            let alpha = identify(&output, "%[fx:p{0,0}.a] %[fx:p{1,0}.a] %[fx:p{2,0}.a]");
            assert_eq!(alpha, "0 1 1");
            outputs.push(fs::read(&output).unwrap());
            assert!(identify(&output, "%[profiles]").is_empty());
        }
    }
    assert_ne!(
        outputs[0], outputs[1],
        "Dithering should change a reduced palette image"
    );
    for name in ["still.gif", "interlaced.gif"] {
        let source = copy(name, &dir.path().join(name));
        settings.target = "png".into();
        let output = convert(&source, &settings);
        assert_eq!(
            identify(&output, "%w %h %[pixel:p{1,0}] %[pixel:p{2,0}]"),
            "32 20 srgba(255,0,0,1) srgba(0,255,0,1)"
        );
    }
}

#[test]
fn malformed_gif_blocks_and_oversized_animations_are_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let original = fs::read(fixture("disposal.gif")).unwrap();
    let mut cases = vec![];
    for cut in [1, 13, 20, original.len() - 1] {
        cases.push(original[..cut].to_vec());
    }
    let mut concatenated = original.clone();
    concatenated.extend(&original);
    cases.push(concatenated);
    let gce = original
        .windows(3)
        .position(|b| b == b"\x21\xf9\x04")
        .unwrap();
    for flag in [0x20, 0x10, 0x02] {
        let mut data = original.clone();
        data[gce + 3] |= flag;
        cases.push(data);
    }
    let mut large = original.clone();
    large[6..10].copy_from_slice(&[0x70, 0x17, 0x70, 0x17]);
    cases.push(large); // 4 × 6000²
    for (index, bytes) in cases.iter().enumerate() {
        let path = dir.path().join(format!("bad{index}.gif"));
        fs::write(&path, bytes).unwrap();
        assert!(
            crate::inputs::require_supported(&path).is_err(),
            "case {index}"
        );
        for target in ["gif", "webp", "png"] {
            let mut settings = options();
            settings.target = target.into();
            assert!(backend()
                .convert(&path, None, &settings, &AtomicBool::new(false))
                .is_err());
        }
    }
    no_partials(dir.path());
}

#[test]
fn webp_frame_bounds_lengths_and_missing_frames_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let original = fs::read(fixture("partial.webp")).unwrap();
    let start = original.windows(4).position(|b| b == b"ANMF").unwrap();
    for (index, offset, value) in [
        (0, start + 4, 0xff),
        (1, start + 8, 0xff),
        (2, start + 23, 0xfc),
    ] {
        let mut data = original.clone();
        data[offset] = value;
        let path = dir.path().join(format!("bad{index}.webp"));
        fs::write(&path, data).unwrap();
        assert!(
            crate::inputs::require_supported(&path).is_err(),
            "case {index}"
        );
    }
}

#[test]
fn animated_and_still_files_share_one_batch_without_combining_or_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy("disposal.gif", &dir.path().join("shared.gif"));
    let b = copy("rgba.png", &dir.path().join("shared.png"));
    fs::write(dir.path().join("shared.webp"), b"existing").unwrap();
    let manager = JobManager::default();
    let job = manager
        .prepare("animation", request(&[a, b], dir.path()), false)
        .unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    for file in job.snapshot().files {
        assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
    }
    assert_eq!(
        fs::read(dir.path().join("shared.webp")).unwrap(),
        b"existing"
    );
    assert!(sequence(&dir.path().join("shared (1).webp")).animated());
    assert_eq!(identify(&dir.path().join("shared (2).webp"), "%n"), "1");
    no_partials(dir.path());
}

#[test]
fn corrupt_lzw_payload_fails_without_publishing_a_partial_gif() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = fs::read(fixture("still.gif")).unwrap();
    let image = 25 + bytes[25..].iter().position(|&b| b == 0x2c).unwrap();
    bytes[image + 12..image + 15].fill(0xff);
    let source = dir.path().join("broken.gif");
    fs::write(&source, &bytes).unwrap();
    // Inspection validates the container, while the worker must reject bad LZW.
    assert!(crate::inputs::require_supported(&source).is_ok());
    for format in crate::image_format::ImageFormat::ALL {
        let mut settings = options();
        settings.target = format.id().into();
        assert!(backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .is_err());
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(fs::read(source).unwrap(), bytes);
}

#[test]
fn frame_budget_is_checked_before_decoding_and_output_timing_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fs::read(fixture("still.gif")).unwrap();
    let mut many = bytes[..25].to_vec();
    for _ in 0..1000 {
        many.extend_from_slice(&bytes[25..bytes.len() - 1]);
    }
    many.push(0x3b);
    let source = dir.path().join("many.gif");
    fs::write(&source, &many).unwrap();
    assert_eq!(sequence(&source).delays.len(), 1000);
    many.pop();
    many.extend_from_slice(&bytes[25..]);
    fs::write(&source, many).unwrap();
    assert!(crate::inputs::require_supported(&source).is_err());
    let source = copy("disposal.gif", &dir.path().join("source.gif"));
    let mut settings = options();
    settings.lossless = true;
    let output = convert(&source, &settings);
    let mut corrupt = fs::read(&output).unwrap();
    let frame = corrupt.windows(4).position(|b| b == b"ANMF").unwrap();
    corrupt[frame + 20] += 1; // duration, not compressed pixels
    fs::write(&output, corrupt).unwrap();
    assert!(sequence(&source)
        .finish_output(&output, "webp", 100)
        .unwrap_err()
        .contains("timing"));
}

#[test]
fn animated_webp_applies_orientation_and_color_profiles_before_metadata_retention() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("profiled-animation.webp", &dir.path().join("profiled.webp"));
    let mut settings = options();
    settings.lossless = true;
    let retained = convert(&source, &settings);
    settings.metadata = false;
    let stripped = convert(&source, &settings);
    assert_eq!(sequence(&retained).delays, vec![33, 33, 34, 0]);
    assert_eq!(sequence(&retained).width, 20);
    assert_eq!(sequence(&retained).height, 32);
    let profiles = identify(&retained, "%[profiles]\n");
    assert!(
        profiles
            .lines()
            .all(|line| line.contains("icc") && line.contains("xmp")),
        "{profiles}"
    );
    assert!(identify(&stripped, "%[profiles]").is_empty());
    for frame in 0..4 {
        assert_eq!(pixels(&retained, frame), pixels(&stripped, frame));
    }
    // The frame payload is the existing photo.webp, interpreted in the linear
    // profile, then oriented. A still conversion of that same tagged payload
    // provides a separate color-management path for the expected first frame.
    let tagged = dir.path().join("tagged.webp");
    let profile = dir.path().join("linear.icc");
    let bytes = fs::read(&source).unwrap();
    let start = bytes.windows(4).position(|b| b == b"ICCP").unwrap();
    let size = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()) as usize;
    fs::write(&profile, &bytes[start + 8..start + 8 + size]).unwrap();
    let result = backend()
        .command()
        .arg(fixture("photo.webp"))
        .args(["-strip", "-profile"])
        .arg(&profile)
        .args(["-rotate", "90", "-define", "webp:lossless=true"])
        .arg(&tagged)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let expected = convert(&tagged, &settings);
    let sample = "%[pixel:p{8,7}] %[pixel:p{12,19}]";
    let result = backend()
        .command()
        .arg(&retained)
        .args(["-delete", "1-3", "-define", "webp:lossless=true"])
        .arg(dir.path().join("first.webp"))
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        identify(&dir.path().join("first.webp"), sample),
        identify(&expected, sample)
    );
}

#[test]
fn still_frames_preserve_composed_gif_canvases_and_webp_blend_alpha() {
    let expected: Vec<Vec<u8>> =
        serde_json::from_slice(&fs::read(fixture("disposal-frames.json")).unwrap()).unwrap();
    for name in ["disposal.gif", "partial.webp"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join(name));
        let original = fs::read(&source).unwrap();
        let input = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
        for target in crate::image_format::ImageFormat::ALL {
            assert!(input["targets"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(format!("frame-{}", target.id()))));
        }
        for (index, _) in sequence(&source).delays.iter().enumerate() {
            let (preview, png) = backend()
                .preview_animation_frame(&source, 0.0, Some(index), &AtomicBool::new(false))
                .unwrap();
            let preview_path = dir.path().join("preview.png");
            fs::write(&preview_path, png).unwrap();
            let settings = ImageOptions {
                target: "png".into(),
                ..options()
            };
            let result = backend()
                .convert_with_frame(
                    &source,
                    None,
                    &settings,
                    Some(&preview.frame.selection),
                    true,
                    &AtomicBool::new(false),
                )
                .unwrap();
            if let Some(folder) = std::env::var_os("RECAST_IMAGE_FRAME_EVIDENCE") {
                fs::create_dir_all(&folder).unwrap();
                fs::copy(
                    &preview_path,
                    Path::new(&folder).join(format!("{name}-{index}-preview.png")),
                )
                .unwrap();
                fs::copy(
                    &result.path,
                    Path::new(&folder).join(format!("{name}-{index}-export.png")),
                )
                .unwrap();
            }
            let actual = pixels(Path::new(&result.path), 0);
            assert_eq!(
                actual,
                pixels(&preview_path, 0),
                "{name} frame {index}: preview/export"
            );
            assert_eq!(
                actual,
                if name == "disposal.gif" {
                    expected[index].clone()
                } else {
                    pixels(&source, index)
                },
                "{name} frame {index}: canvas"
            );
            assert_eq!(preview.index, index);
            assert_eq!(preview.count, 4);
            assert_eq!(preview.previous_frame, index.checked_sub(1));
            assert_eq!(preview.next_frame, (index < 3).then_some(index + 1));
        }
        assert_eq!(fs::read(&source).unwrap(), original);
        no_partials(dir.path());
    }
}

#[test]
fn still_frames_support_all_formats_background_resize_and_explicit_mode() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("partial.webp", &dir.path().join("source.webp"));
    let (selected, _) = backend()
        .preview_animation_frame(&source, 0.05, None, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(selected.index, 1);
    for target in crate::image_format::ImageFormat::ALL {
        let settings = ImageOptions {
            target: target.id().into(),
            lossless: true,
            background: "#123456".into(),
            ..options()
        };
        let output = backend()
            .convert_with_frame(
                &source,
                None,
                &settings,
                Some(&selected.frame.selection),
                true,
                &AtomicBool::new(false),
            )
            .unwrap();
        let path = Path::new(&output.path);
        assert_eq!(identify(path, "%w %h %n"), "40 24 1");
        assert!(!crate::inputs::inspect(path)
            .unwrap()
            .sequence
            .is_some_and(|s| s.animated()));
        assert!(target.validate_output(path).is_ok());
        if target == crate::image_format::ImageFormat::Bmp {
            assert_eq!(&pixels(path, 0)[..4], &[0x12, 0x34, 0x56, 255]);
        }
        if [
            crate::image_format::ImageFormat::Png,
            crate::image_format::ImageFormat::Tiff,
            crate::image_format::ImageFormat::WebP,
        ]
        .contains(&target)
        {
            let actual = pixels(path, 0);
            let expected = pixels(&source, 1);
            let max = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            // WebP quantizes the coalesced Q16 RGB to 8 bits; alpha stays exact.
            let allowed = actual.iter().zip(&expected).enumerate().all(|(i, (a, b))| {
                a.abs_diff(*b)
                    <= if target == crate::image_format::ImageFormat::WebP && i % 4 != 3 {
                        1
                    } else {
                        0
                    }
            });
            assert!(allowed, "{target:?}: maximum channel difference {max}");
        }
    }
    let settings = ImageOptions {
        target: "png".into(),
        resize: 50,
        ..options()
    };
    let output = backend()
        .convert_with_frame(
            &source,
            None,
            &settings,
            Some(&selected.frame.selection),
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(identify(Path::new(&output.path), "%w %h"), "20 12");
    assert!(backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .is_err());
    let motion = convert(
        &source,
        &ImageOptions {
            target: "gif".into(),
            ..options()
        },
    );
    assert_eq!(sequence(&motion).delays.len(), 4);
    no_partials(dir.path());
}

#[test]
fn still_frames_preserve_color_orientation_and_metadata_choices() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("profiled-animation.webp", &dir.path().join("profiled.webp"));
    let (preview, png) = backend()
        .preview_animation_frame(&source, 0.04, None, &AtomicBool::new(false))
        .unwrap();
    let reference = dir.path().join("preview.png");
    fs::write(&reference, png).unwrap();
    for metadata in [true, false] {
        let settings = ImageOptions {
            target: "png".into(),
            metadata,
            ..options()
        };
        let output = backend()
            .convert_with_frame(
                &source,
                None,
                &settings,
                Some(&preview.frame.selection),
                true,
                &AtomicBool::new(false),
            )
            .unwrap();
        let path = Path::new(&output.path);
        assert_eq!(identify(path, "%w %h"), "20 32");
        assert_eq!(pixels(path, 0), pixels(&reference, 0));
        let profiles = identify(path, "%[profiles]");
        if metadata {
            assert!(
                profiles.contains("icc") && profiles.contains("xmp"),
                "{profiles}"
            );
        } else {
            assert!(profiles.is_empty(), "{profiles}");
        }
    }
}

#[test]
fn still_frames_zero_and_millisecond_delays_keep_every_frame_accessible() {
    let dir = tempfile::tempdir().unwrap();
    let zero = dir.path().join("zero.gif");
    let mut bytes = fs::read(fixture("disposal.gif")).unwrap();
    for i in 0..bytes.len() - 7 {
        if bytes[i..i + 3] == [0x21, 0xf9, 4] {
            bytes[i + 4] = 0;
            bytes[i + 5] = 0;
        }
    }
    fs::write(&zero, bytes).unwrap();
    for source in [
        zero,
        fixture("fast.webp"),
        fixture("millisecond.webp"),
        fixture("long-delay.webp"),
    ] {
        let delays = sequence(&source).delays;
        let mut elapsed = 0u64;
        for (index, delay) in delays.iter().enumerate() {
            let (preview, _) = backend()
                .preview_animation_frame(
                    &source,
                    elapsed as f64 / 1000.0,
                    Some(index),
                    &AtomicBool::new(false),
                )
                .unwrap();
            assert_eq!(preview.index, index);
            assert_eq!(preview.frame.selection.time, elapsed as f64 / 1000.0);
            assert_eq!(
                crate::image_frame::resolve(&sequence(&source), Some(&preview.frame.selection))
                    .unwrap(),
                index
            );
            elapsed += u64::from(*delay);
        }
        let (last, _) = backend()
            .preview_animation_frame(
                &source,
                elapsed as f64 / 1000.0,
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(last.index, delays.len() - 1);
        assert!(backend()
            .preview_animation_frame(&source, 0.0, Some(delays.len()), &AtomicBool::new(false))
            .is_err());
        assert!(backend()
            .preview_animation_frame(&source, -1.0, None, &AtomicBool::new(false))
            .is_err());
    }
}

#[test]
fn still_frames_changed_sources_invalid_ids_and_cancellation_do_not_publish() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("disposal.gif", &dir.path().join("source.gif"));
    let (preview, _) = backend()
        .preview_animation_frame(&source, 0.08, None, &AtomicBool::new(false))
        .unwrap();
    let settings = ImageOptions {
        target: "png".into(),
        ..options()
    };
    let result = backend().convert_with_frame(
        &source,
        None,
        &settings,
        Some(&preview.frame.selection),
        true,
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(ConversionError::Cancelled)));
    assert!(matches!(
        backend().preview_animation_frame(&source, 0.0, None, &AtomicBool::new(true)),
        Err(ConversionError::Cancelled)
    ));
    let mut invalid = preview.frame.selection.clone();
    invalid.pts = "1000".into();
    assert!(backend()
        .convert_with_frame(
            &source,
            None,
            &settings,
            Some(&invalid),
            true,
            &AtomicBool::new(false)
        )
        .is_err());
    let mut invalid = preview.frame.selection.clone();
    invalid.time = f64::NAN;
    assert!(backend()
        .convert_with_frame(
            &source,
            None,
            &settings,
            Some(&invalid),
            true,
            &AtomicBool::new(false)
        )
        .is_err());
    // Same valid container, changed timestamp/size fingerprint: no silent re-selection.
    fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
        .unwrap();
    assert!(
        matches!(backend().convert_with_frame(&source,None,&settings,Some(&preview.frame.selection),true,&AtomicBool::new(false)),Err(ConversionError::Failed(s)) if s.contains("changed"))
    );
    assert!(!dir.path().join("source.png").exists());
    no_partials(dir.path());
}

#[test]
fn still_frames_mixed_batch_retry_freezes_selection_and_preserves_successes() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy("disposal.gif", &dir.path().join("a.gif"));
    let b = copy("disposal.gif", &dir.path().join("b.gif"));
    let still = copy("rgba.png", &dir.path().join("ordinary.png"));
    let (selected, _) = backend()
        .preview_animation_frame(&b, 0.0, Some(2), &AtomicBool::new(false))
        .unwrap();
    let mut req = request(&[a.clone(), b.clone(), still], dir.path());
    req.image_options = None;
    req.image_frame_options = Some(crate::image_frame::ImageFrameOptions {
        image: ImageOptions {
            target: "png".into(),
            ..options()
        },
        frames: [(b.to_string_lossy().into_owned(), selected.frame.selection)].into(),
    });
    let held = dir.path().join("held.gif");
    fs::rename(&b, &held).unwrap();
    let manager = JobManager::default();
    let job = manager.prepare("image-frames", req.clone(), false).unwrap();
    run_job(&job, &backend(), &audio_backend(), |_| {});
    let before = job.snapshot();
    assert_eq!(before.files[0].status, FileStatus::Succeeded, "{before:?}");
    assert_eq!(before.files[1].status, FileStatus::Failed);
    assert_eq!(before.files[2].status, FileStatus::Succeeded);
    assert_eq!(
        pixels(Path::new(before.files[0].output_path.as_ref().unwrap()), 0),
        pixels(&a, 0)
    );
    fs::rename(held, &b).unwrap();
    req.image_frame_options.as_mut().unwrap().frames.clear();
    req.image_frame_options.as_mut().unwrap().image.target = "jpeg".into();
    let retry = manager.prepare("image-frames", req, true).unwrap();
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
    let output = after.files[1].output_path.as_ref().unwrap();
    assert!(output.ends_with("b.png"));
    assert_eq!(pixels(Path::new(output), 0), pixels(&b, 2));
    no_partials(dir.path());
}
