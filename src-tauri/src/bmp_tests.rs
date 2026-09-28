use super::*;

// Inspect the actual RGB bytes independently of the worker's decoder. Recast
// exports bottom-up, 24-bit BMP rows with DWORD padding and a 54-byte header.
fn rgb(path: &Path, x: usize, y: usize) -> [u8; 3] {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..2], b"BM");
    assert_eq!(&bytes[10..18], &[54, 0, 0, 0, 40, 0, 0, 0]);
    assert_eq!(&bytes[26..34], &[1, 0, 24, 0, 0, 0, 0, 0]);
    let width = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize;
    let stride = (width * 3).div_ceil(4) * 4;
    assert_eq!(bytes.len(), 54 + height * stride);
    let start = 54 + (height - 1 - y) * stride + x * 3;
    [bytes[start + 2], bytes[start + 1], bytes[start]]
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
fn bmp_rgb_palette_rle_and_top_down_pixels_survive_exactly() {
    for name in [
        "rgb.bmp",
        "top-down.bmp",
        "core.bmp",
        "palette.bmp",
        "rle.bmp",
        "rle4.bmp",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join("misleading.png"));
        let inspected = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
        assert_eq!(inspected["format"], "BMP");
        let mut settings = options();
        settings.target = "bmp".into();
        let output = convert(&source, &settings);
        for y in 0..20 {
            for x in 0..32 {
                let expected = if ["palette.bmp", "rle.bmp", "rle4.bmp"].contains(&name) {
                    let index = if name == "rle4.bmp" {
                        (x + 3 * y) % 16
                    } else {
                        x + 3 * y
                    };
                    [index as u8, 255 - index as u8, (index * 5) as u8]
                } else {
                    [(x * 8) as u8, (y * 12) as u8, ((x + y) * 5) as u8]
                };
                assert_eq!(rgb(&output, x, y), expected, "{name} ({x}, {y})");
            }
        }
    }
}

#[test]
fn bmp_background_composites_full_and_partial_transparency() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.bmp", &dir.path().join("alpha.bmp"));
    let mut settings = options();
    settings.target = "bmp".into();
    for (background, expected) in [
        ("#ffffff", [255, 255, 255]),
        ("#000000", [0, 0, 0]),
        ("#287ec4", [40, 126, 196]),
    ] {
        settings.background = background.into();
        let output = convert(&source, &settings);
        assert_eq!(rgb(&output, 0, 0), expected);
        let half: Vec<_> = [8, 0, 5]
            .into_iter()
            .zip(expected)
            .map(|(source, background)| {
                ((source as f64 * 128.0 + background as f64 * 127.0) / 255.0).round() as u8
            })
            .collect();
        // Q16 compositing followed by 8-bit encoding can round one channel
        // level either way. Opaque and fully transparent samples remain exact.
        assert!(rgb(&output, 1, 0)
            .into_iter()
            .zip(half)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 1));
        assert_eq!(rgb(&output, 2, 0), [16, 0, 10]);
    }
}

#[test]
fn bmp_alpha_input_stays_transparent_in_png_and_lossless_webp() {
    for target in ["png", "webp"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy("rgba.bmp", &dir.path().join("alpha.bmp"));
        let mut settings = options();
        settings.target = target.into();
        settings.lossless = true;
        let output = convert(&source, &settings);
        let pixels =
            "%[pixel:p{0,0}]|%[pixel:p{1,0}]|%[pixel:p{2,0}]|%[pixel:p{3,0}]|%[pixel:p{16,9}]";
        assert_eq!(identify(&source, pixels), identify(&output, pixels));
        assert_eq!(identify(&output, "%[opaque]"), "False");
    }
}

#[test]
fn bmp_output_ignores_quality_lossless_and_metadata_and_pads_rows() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("odd-width.png");
    let result = backend()
        .command()
        .arg(fixture("rgb.png"))
        .args(["-crop", "3x2+0+0", "+repage"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut settings = options();
    settings.target = "bmp".into();
    settings.quality = 1;
    let low = convert(&source, &settings);
    settings.quality = 100;
    settings.lossless = true;
    settings.metadata = false;
    let high = convert(&source, &settings);
    assert_eq!(fs::read(&low).unwrap(), fs::read(&high).unwrap());
    let bytes = fs::read(&low).unwrap();
    assert_eq!(bytes.len(), 78);
    assert_eq!(&bytes[63..66], &[0, 0, 0]);
    assert_eq!(&bytes[75..78], &[0, 0, 0]);
    rgb(&low, 2, 1);
    assert_eq!(identify(&low, "%w %h %[profiles]"), "3 2 ");
}

#[test]
fn bmp_embedded_profile_is_transformed_before_export() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("profiled.bmp", &dir.path().join("linear.bmp"));
    let before = fs::read(&source).unwrap();
    let mut settings = options();
    settings.target = "bmp".into();
    let bitmap = convert(&source, &settings);
    assert!(rgb(&bitmap, 15, 9)[0] > 160); // Original linear-red sample is 120.
    settings.target = "png".into();
    let png = convert(&source, &settings);
    let pixels = "%[pixel:p{15,9}]|%[pixel:p{7,0}]";
    assert_eq!(identify(&bitmap, pixels), identify(&png, pixels));
    assert!(identify(&png, "%[profiles]").contains("icc"));
    assert!(identify(&bitmap, "%[profiles]").is_empty());
    assert_eq!(fs::read(&source).unwrap(), before);
}

#[test]
fn bmp_rejects_truncation_bad_offsets_and_embedded_image_wrappers() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("invalid.bmp");
    let original = fs::read(fixture("rgb.bmp")).unwrap();
    let mut bad = original.clone();
    bad.pop();
    fs::write(&source, &bad).unwrap();
    assert!(crate::inputs::require_supported(&source).is_err());
    // Even a forged, matching file size cannot hide short pixel data.
    let shortened = bad.len() as u32 - 1;
    bad[2..6].copy_from_slice(&shortened.to_le_bytes());
    bad.pop();
    fs::write(&source, &bad).unwrap();
    assert!(crate::inputs::require_supported(&source).is_err());
    for (offset, value) in [(10, 0u32), (14, 9), (18, 0), (22, 0)] {
        let mut bad = original.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        fs::write(&source, bad).unwrap();
        assert!(crate::inputs::require_supported(&source).is_err());
    }
    for compression in [4u32, 5] {
        let mut wrapped = original.clone();
        wrapped[30..34].copy_from_slice(&compression.to_le_bytes());
        fs::write(&source, wrapped).unwrap();
        assert!(crate::inputs::require_supported(&source)
            .unwrap_err()
            .contains("embedded"));
    }
    let mut array = original;
    array[..2].copy_from_slice(b"BA");
    fs::write(&source, array).unwrap();
    assert!(crate::inputs::inspect(&source)
        .unwrap_err()
        .contains("bitmap arrays"));
}

#[test]
fn bmp_sequences_never_publish_even_with_a_forged_file_size() {
    for forge_size in [false, true] {
        for target in crate::image_format::ImageFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("sequence.bmp");
            let mut bytes = fs::read(fixture("rgb.bmp")).unwrap();
            bytes.extend_from_slice(&bytes.clone());
            if forge_size {
                let length = bytes.len() as u32;
                bytes[2..6].copy_from_slice(&length.to_le_bytes());
            }
            fs::write(&source, &bytes).unwrap();
            let mut settings = options();
            settings.target = target.id().into();
            assert!(
                backend()
                    .convert(&source, None, &settings, &AtomicBool::new(false))
                    .is_err(),
                "{target:?}, forged: {forge_size}"
            );
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            assert_eq!(fs::read(source).unwrap(), bytes);
            no_partials(dir.path());
        }
    }
}
