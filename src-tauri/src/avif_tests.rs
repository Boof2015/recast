use super::*;

fn convert(source: &Path, settings: &ImageOptions) -> PathBuf {
    PathBuf::from(
        backend()
            .convert(source, None, settings, &AtomicBool::new(false))
            .unwrap()
            .path,
    )
}

#[test]
fn avif_still_alpha_and_high_depth_inputs_are_recognized_by_content() {
    for name in [
        "rgb.avif",
        "rgba.avif",
        "rgba12.avif",
        "gray10.avif",
        "rotated.avif",
        "profiled.avif",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join("misleading.txt"));
        let input = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
        assert_eq!(input["format"], "AVIF", "{name}");
        assert!(input["conversionIssue"].is_null(), "{name}: {input}");
        assert_eq!(input["targets"].as_array().unwrap().len(), 6);
        assert!(crate::image_format::ImageFormat::Avif
            .validate_output(&source)
            .is_ok());
    }
}

#[test]
fn avif_lossless_and_alpha_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.target = "avif".into();
    settings.lossless = true;
    settings.quality = 5;
    let output = convert(&source, &settings);
    assert_eq!(identify(&output, "%m %w %h %n"), "AVIF 32 20 1");
    // Read every raw RGB/alpha sample through uncompressed BMP output. Turning
    // alpha off preserves hidden RGB and avoids the FX interpolator's implicit
    // alpha weighting at transparent pixels.
    fn pixels(path: &Path) -> Vec<u8> {
        let mut pixels = Vec::new();
        for alpha in ["off", "extract"] {
            let result = backend()
                .command()
                .arg(path)
                .args([
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
            assert_eq!(&result.stdout[..2], b"BM");
            assert_eq!(
                u32::from_le_bytes(result.stdout[10..14].try_into().unwrap()),
                54
            );
            assert_eq!(&result.stdout[28..30], &[24, 0]);
            pixels.extend_from_slice(&result.stdout[54..]);
        }
        pixels
    }
    let actual = pixels(&output);
    let expected = pixels(&source);
    assert!(
        actual == expected,
        "lossless pixels differ: {:?}",
        actual
            .iter()
            .zip(&expected)
            .enumerate()
            .find(|(_, (a, b))| a != b)
    );
    settings.lossless = false;
    settings.quality = 10;
    let lossy = convert(&source, &settings);
    let expected = pixels(&source);
    let actual = pixels(&lossy);
    assert_eq!(
        &actual[1920..],
        &expected[1920..],
        "lossy color must preserve alpha exactly"
    );
    assert_ne!(&actual[..1920], &expected[..1920]);
    settings.lossless = true;
    settings.quality = 95;
    assert_eq!(
        fs::read(convert(&source, &settings)).unwrap(),
        fs::read(output).unwrap(),
        "hidden quality must not affect lossless output"
    );
    no_partials(dir.path());
}

#[test]
fn avif_quality_is_independent_of_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgb.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.target = "avif".into();
    settings.quality = 15;
    settings.metadata = false;
    let low = convert(&source, &settings);
    settings.quality = 100;
    let high = convert(&source, &settings);
    settings.lossless = true;
    let lossless = convert(&source, &settings);
    assert_ne!(fs::read(&low).unwrap(), fs::read(&high).unwrap());
    // Matrix 6 is lossy RGB->YCbCr; matrix 0 preserves RGB exactly. Check the
    // actual nclx property, independently of ImageMagick's reported quality.
    let profile = |path: &Path| {
        let bytes = fs::read(path).unwrap();
        let start = bytes.windows(4).position(|v| v == b"nclx").unwrap();
        bytes[start..start + 11].to_vec()
    };
    assert_eq!(&profile(&high)[8..10], &[0, 6]);
    assert_eq!(&profile(&lossless)[8..10], &[0, 0]);
}

#[test]
fn avif_sequences_hdr_and_additional_images_never_publish() {
    let dir = tempfile::tempdir().unwrap();
    let mut cases = vec![
        (fs::read(fixture("animated.avif")).unwrap(), "Animated"),
        (fs::read(fixture("hdr.avif")).unwrap(), "HDR"),
        (fs::read(fixture("collection.avif")).unwrap(), "multi-image"),
    ];
    // Remove the alpha relationship while keeping two valid AV1 image items.
    // This turns the alpha image into a second independent picture.
    let mut collection = fs::read(fixture("rgba.avif")).unwrap();
    let reference = collection.windows(4).position(|v| v == b"auxl").unwrap();
    collection[reference..reference + 4].copy_from_slice(b"cdsc");
    cases.push((collection, "multi-image"));
    // A movie box must be caught even when the AVIS compatible brand is forged.
    let mut disguised = fs::read(fixture("animated.avif")).unwrap();
    for i in 0..disguised.len() - 4 {
        if &disguised[i..i + 4] == b"avis" || &disguised[i..i + 4] == b"msf1" {
            disguised[i..i + 4].copy_from_slice(b"avif");
        }
    }
    cases.push((disguised, "Animated"));
    for (index, (bytes, expected)) in cases.into_iter().enumerate() {
        let source = dir.path().join(format!("unsupported-{index}.avif"));
        fs::write(&source, bytes).unwrap();
        let input = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
        assert!(
            input["conversionIssue"]
                .as_str()
                .unwrap()
                .contains(expected),
            "{input}"
        );
        assert!(input["targets"].as_array().unwrap().is_empty());
        for target in crate::image_format::ImageFormat::ALL {
            let mut settings = options();
            settings.target = target.id().into();
            assert!(backend()
                .convert(&source, None, &settings, &AtomicBool::new(false))
                .is_err());
        }
    }
    no_partials(dir.path());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 5);
}

#[test]
fn avif_malformed_box_lengths_and_extents_are_rejected() {
    let original = fs::read(fixture("rgb.avif")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut cases = vec![
        original[..16].to_vec(),
        original[..original.len() - 1].to_vec(),
    ];
    let mut large = original.clone();
    large[..4].copy_from_slice(&u32::MAX.to_be_bytes());
    cases.push(large);
    let mut small = original.clone();
    small[..4].copy_from_slice(&4u32.to_be_bytes());
    cases.push(small);
    let mut extents = original.clone();
    let iloc = extents.windows(4).position(|v| v == b"iloc").unwrap();
    // Corrupt the field-size nibbles without changing the outer box bounds.
    extents[iloc + 8] = 0xff;
    cases.push(extents);
    let mut doubled = original.clone();
    doubled.extend_from_slice(&original);
    cases.push(doubled);
    for bytes in cases {
        let path = dir.path().join("malformed.avif");
        fs::write(&path, bytes).unwrap();
        assert!(crate::avif::inspect(&mut fs::File::open(&path).unwrap()).is_err());
        assert!(crate::image_format::ImageFormat::Avif
            .validate_output(&path)
            .is_err());
        assert!(backend()
            .convert(&path, None, &options(), &AtomicBool::new(false))
            .is_err());
        no_partials(dir.path());
    }
}

#[test]
fn avif_preserves_supported_precision_and_exports_standard_tiff_depths() {
    let dir = tempfile::tempdir().unwrap();
    let mut settings = options();
    settings.target = "avif".into();
    settings.lossless = true;
    settings.metadata = false;
    for (name, expected) in [
        ("rgba.png", 8),
        ("gray10.avif", 10),
        ("rgba12.avif", 12),
        ("rgba16.tiff", 12),
    ] {
        let source = copy(name, &dir.path().join(name));
        let output = convert(&source, &settings);
        let bytes = fs::read(&output).unwrap();
        let av1c = bytes.windows(4).position(|value| value == b"av1C").unwrap();
        let encoded = bytes[av1c + 6];
        let depth = if encoded & 0x40 == 0 {
            8
        } else if encoded & 0x20 == 0 {
            10
        } else {
            12
        };
        if name == "gray10.avif" {
            // Expanding grayscale to sRGB can require additional precision.
            assert!((10..=12).contains(&depth), "{name}: {depth}");
        } else {
            assert_eq!(depth, expected, "{name}");
        }
    }
    for name in ["gray10.avif", "rgba12.avif"] {
        for target in ["png", "tiff"] {
            settings.target = target.into();
            let source = dir.path().join(name);
            let output = convert(&source, &settings);
            assert_eq!(identify(&output, "%z"), "16", "{name} -> {target}");
            for pixel in ["%[fx:p{1,0}.r]", "%[fx:p{10,8}.g]", "%[fx:p{2,1}.a]"] {
                let expected: f64 = identify(&source, pixel).parse().unwrap();
                let actual: f64 = identify(&output, pixel).parse().unwrap();
                assert!(
                    (actual - expected).abs() < 0.00003,
                    "{name} -> {target}: {expected} != {actual}"
                );
            }
        }
    }
}

#[test]
fn avif_profiles_transform_color_before_metadata_is_kept_or_stripped() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("profiled.avif", &dir.path().join("profiled.avif"));
    let mut settings = options();
    settings.lossless = true;
    for target in ["png", "avif"] {
        settings.target = target.into();
        settings.metadata = true;
        let retained = convert(&source, &settings);
        settings.metadata = false;
        let stripped = convert(&source, &settings);
        let pixel =
            "%[fx:round(255*p{10,8}.r)] %[fx:round(255*p{10,8}.g)] %[fx:round(255*p{10,8}.b)]";
        assert_ne!(identify(&source, pixel), identify(&retained, pixel));
        assert_eq!(identify(&retained, pixel), identify(&stripped, pixel));
        assert!(identify(&retained, "%[profiles]").contains("icc"));
        assert!(identify(&stripped, "%[profiles]").is_empty());
    }
}

#[test]
fn avif_alpha_uses_the_selected_background_for_jpeg_and_bmp() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.avif", &dir.path().join("source.avif"));
    let png = fixture("rgba.png");
    let mut settings = options();
    for target in ["jpeg", "bmp"] {
        settings.target = target.into();
        settings.quality = 100;
        settings.background = "#27518a".into();
        let avif_output = convert(&source, &settings);
        let png_output = backend()
            .convert(&png, Some(dir.path()), &settings, &AtomicBool::new(false))
            .unwrap();
        let pixel = "%[fx:round(255*p{0,0}.r)] %[fx:round(255*p{0,0}.g)] %[fx:round(255*p{0,0}.b)] %[fx:round(255*p{0,0}.a)]";
        assert_eq!(
            identify(&avif_output, pixel),
            identify(Path::new(&png_output.path), pixel)
        );
        assert_eq!(identify(&avif_output, "%[opaque]"), "True");
    }
}

#[test]
fn avif_grid_tiles_form_one_image_for_every_destination() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("grid.avif", &dir.path().join("grid.avif"));
    let input = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
    assert!(input["conversionIssue"].is_null(), "{input}");
    for target in crate::image_format::ImageFormat::ALL {
        let mut settings = options();
        settings.target = target.id().into();
        settings.lossless = true;
        settings.quality = 100;
        let output = convert(&source, &settings);
        assert_eq!(identify(&output, "%w %h %n"), "128 64 1");
        for (x, expected) in [(10, [200, 10, 20]), (90, [20, 30, 200])] {
            let pixel = format!("%[fx:round(255*p{{{x},10}}.r)] %[fx:round(255*p{{{x},10}}.g)] %[fx:round(255*p{{{x},10}}.b)]");
            let actual: Vec<i32> = identify(&output, &pixel)
                .split_whitespace()
                .map(|n| n.parse().unwrap())
                .collect();
            let tolerance = if target == crate::image_format::ImageFormat::Jpeg {
                2
            } else {
                0
            };
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| (a - b).abs() <= tolerance),
                "{target:?}: {actual:?}"
            );
        }
    }
    no_partials(dir.path());
}

#[test]
fn avif_corrupt_payload_fails_without_publication() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = fs::read(fixture("rgb.avif")).unwrap();
    let data = bytes.windows(4).position(|value| value == b"mdat").unwrap() + 4;
    bytes[data..].fill(0);
    let source = dir.path().join("corrupt.avif");
    fs::write(&source, &bytes).unwrap();
    // Bounded container inspection deliberately does not duplicate AV1 decoding.
    assert!(crate::avif::inspect(&mut fs::File::open(&source).unwrap()).is_ok());
    for target in crate::image_format::ImageFormat::ALL {
        let mut settings = options();
        settings.target = target.id().into();
        assert!(backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .is_err());
    }
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    no_partials(dir.path());
}

#[test]
fn avif_twelve_bit_alpha_spans_the_full_normalized_range() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("alpha12.avif", &dir.path().join("known-samples.avif"));
    let mut settings = options();
    settings.target = "avif".into();
    settings.lossless = true;
    let roundtrip = convert(&source, &settings);
    for path in [&source, &roundtrip] {
        for (x, expected) in [(0, 0.0), (1, 2048.0 / 4095.0), (2, 1.0)] {
            for channel in ["r", "a"] {
                let actual: f64 = identify(path, &format!("%[fx:p{{{x},0}}.{channel}]"))
                    .parse()
                    .unwrap();
                assert!(
                    (actual - expected).abs() < 0.00001,
                    "12-bit {channel} sample {x}: {actual} != {expected}"
                );
            }
        }
    }
}
