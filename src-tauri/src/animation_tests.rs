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
            input["targets"],
            serde_json::json!(["webp", "gif"]),
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
        assert_eq!(input["targets"], serde_json::json!(targets), "{input}");
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
    let frames = sequence(path).delays.len();
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
    run_job(&job, &backend(), |_| {});
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
