use super::*;

fn convert(source: &Path, settings: &ImageOptions) -> PathBuf {
    PathBuf::from(
        backend()
            .convert(source, None, settings, &AtomicBool::new(false))
            .unwrap()
            .path,
    )
}

// Independent inspection of classic TIFF entries emitted by Recast. All source
// fixtures use our own assembler; expected pixels come from its simple patterns.
fn values(bytes: &[u8], tag: u16) -> Vec<u32> {
    assert_eq!(&bytes[..4], b"II\x2a\0");
    let u16_at = |n| u16::from_le_bytes(bytes[n..n + 2].try_into().unwrap());
    let u32_at = |n| u32::from_le_bytes(bytes[n..n + 4].try_into().unwrap());
    let ifd = u32_at(4) as usize;
    let count = u16_at(ifd) as usize;
    assert_eq!(
        u32_at(ifd + 2 + count * 12),
        0,
        "output has more than one page"
    );
    for n in 0..count {
        let start = ifd + 2 + n * 12;
        if u16_at(start) != tag {
            continue;
        }
        let width = match u16_at(start + 2) {
            3 => 2,
            4 => 4,
            value => panic!("Unexpected type {value}"),
        };
        let count = u32_at(start + 4) as usize;
        let offset = if count * width <= 4 {
            start + 8
        } else {
            u32_at(start + 8) as usize
        };
        return (0..count)
            .map(|n| {
                if width == 2 {
                    u32::from(u16_at(offset + n * width))
                } else {
                    u32_at(offset + n * width)
                }
            })
            .collect();
    }
    vec![]
}

fn bmp_pixel(path: &Path, x: usize, y: usize) -> [u8; 3] {
    let bytes = fs::read(path).unwrap();
    let width = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize;
    let offset = 54 + (height - 1 - y) * (width * 3).div_ceil(4) * 4 + x * 3;
    [bytes[offset + 2], bytes[offset + 1], bytes[offset]]
}

#[test]
fn tiff_byte_orders_storage_and_compression_preserve_known_pixels() {
    for name in [
        "rgb-le.tiff",
        "rgb-be.tiff",
        "bigtiff-le.tiff",
        "bigtiff-be.tiff",
        "deflate.tiff",
        "packbits.tiff",
        "tiled.tiff",
        "planar.tiff",
        "palette.tiff",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join("misleading.jpg"));
        let info =
            serde_json::to_value(crate::inputs::require_supported(&source).unwrap()).unwrap();
        assert_eq!(info["format"], "TIFF");
        assert!(info["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "tiff"));
        let mut settings = options();
        settings.target = "bmp".into();
        let output = convert(&source, &settings);
        for y in 0..20 {
            for x in 0..32 {
                let expected = if name == "palette.tiff" {
                    let i = x + 3 * y;
                    [i as u8, (255 - i) as u8, (i * 5 % 256) as u8]
                } else {
                    [(x * 8) as u8, (y * 12) as u8, ((x + y) * 5) as u8]
                };
                assert_eq!(bmp_pixel(&output, x, y), expected, "{name} ({x}, {y})");
            }
        }
    }
}

#[test]
fn tiff_output_is_single_page_deflate_and_ignores_lossy_settings() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.tiff", &dir.path().join("source.tif"));
    let mut settings = options();
    settings.target = "tiff".into();
    settings.quality = 1;
    settings.lossless = false;
    let first = convert(&source, &settings);
    settings.quality = 100;
    settings.lossless = true;
    let second = convert(&source, &settings);
    for output in [first, second] {
        let bytes = fs::read(&output).unwrap();
        assert!(matches!(values(&bytes, 259).as_slice(), [8] | [32946]));
        assert_eq!(values(&bytes, 338), [2]); // straight, not premultiplied alpha
        assert_eq!(values(&bytes, 258), [8, 8, 8, 8]);
        assert_eq!(identify(&output, "%m %w %h %n"), "TIFF 32 20 1");
        for x in [0, 1, 2, 15, 31] {
            let pixel = format!("%[pixel:p{{{x},7}}]");
            assert_eq!(identify(&output, &pixel), identify(&source, &pixel));
        }
    }
    assert_eq!(
        fs::read(source).unwrap(),
        fs::read(fixture("rgba.tiff")).unwrap()
    );
    no_partials(dir.path());
}

#[test]
fn tiff_sixteen_bit_samples_survive_tiff_and_png_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("gray16.tiff", &dir.path().join("gray16.tiff"));
    for target in ["tiff", "png"] {
        let mut settings = options();
        settings.target = target.into();
        let output = convert(&source, &settings);
        if target == "tiff" {
            assert!(values(&fs::read(&output).unwrap(), 258)
                .iter()
                .all(|v| *v == 16));
        }
        for (x, y) in [(0, 0), (1, 0), (2, 9), (31, 19)] {
            let pixel = format!("%[fx:round(65535*p{{{x},{y}}}.r)]");
            let actual: u32 = identify(&output, &pixel).parse().unwrap();
            assert_eq!(actual, (x * 1901 + y * 97) % 65536, "{target} ({x}, {y})");
        }
    }
    let source = copy("rgba16.tiff", &dir.path().join("rgba16.tiff"));
    for target in ["tiff", "png"] {
        let mut settings = options();
        settings.target = target.into();
        let output = convert(&source, &settings);
        // Pixel-expression sampling can discard hidden RGB when interpolating
        // alpha=0. Read an uncompressed export's actual 16-bit samples instead.
        let raw = output.with_extension("raw.tiff");
        let exported = backend()
            .command()
            .arg(&output)
            .args(["-compress", "None", "-define", "tiff:rows-per-strip=64"])
            .arg(&raw)
            .output()
            .unwrap();
        assert!(
            exported.status.success(),
            "{}",
            String::from_utf8_lossy(&exported.stderr)
        );
        let bytes = fs::read(raw).unwrap();
        assert_eq!(values(&bytes, 258), [16, 16, 16, 16]);
        assert_eq!(values(&bytes, 338), [2]);
        let strip = values(&bytes, 273)[0] as usize;
        for (x, y) in [(0, 7), (1, 9), (2, 13), (31, 19)] {
            let offset = strip + (y * 32 + x) as usize * 8;
            let actual: Vec<_> = bytes[offset..offset + 8]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u32::from(u16::from_le_bytes(*b)))
                .collect();
            assert_eq!(
                actual,
                [
                    x * 1901,
                    y * 3101,
                    (x + y) * 997,
                    [0, 32768, 65535][x as usize % 3]
                ],
                "{target} ({x}, {y})"
            );
        }
    }
}

#[test]
fn tiff_lzw_jpeg_and_fax_codecs_work_with_the_bundled_libraries() {
    for (compression, code) in [("LZW", 5), ("JPEG", 7), ("Group4", 4)] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("compressed.tiff");
        let mut command = backend().command();
        command.arg(fixture("rgb-le.tiff"));
        if compression == "Group4" {
            command.args(["-threshold", "50%", "-type", "Bilevel"]);
        }
        let result = command
            .args([
                "-compress",
                compression,
                "-quality",
                "100",
                "-sampling-factor",
                "1x1",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(values(&fs::read(&source).unwrap(), 259), [code]);
        crate::inputs::require_supported(&source).unwrap();
        let mut settings = options();
        settings.target = "bmp".into();
        let output = convert(&source, &settings);
        for (x, y) in [(0, 0), (12, 7), (31, 19)] {
            let actual = bmp_pixel(&output, x, y);
            if compression == "Group4" {
                assert!(actual == [0, 0, 0] || actual == [255, 255, 255]);
            } else {
                let expected = [x * 8, y * 12, (x + y) * 5];
                let tolerance = if compression == "JPEG" { 3 } else { 0 };
                assert!(
                    actual
                        .iter()
                        .zip(expected)
                        .all(|(a, b)| (i32::from(*a) - b as i32).abs() <= tolerance),
                    "{compression}: {actual:?}"
                );
            }
        }
    }
}

#[test]
fn xmp_is_retained_or_stripped_across_metadata_capable_formats() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rotated.tiff", &dir.path().join("metadata.tiff"));
    for target in ["png", "jpeg", "webp", "tiff"] {
        let mut settings = options();
        settings.target = target.into();
        let retained = convert(&source, &settings);
        assert!(
            identify(&retained, "%[profiles]").contains("xmp"),
            "{target}"
        );
        // Also exercise reading XMP from the other containers, not only TIFF.
        let roundtrip = convert(&retained, &settings);
        assert!(
            identify(&roundtrip, "%[profiles]").contains("xmp"),
            "{target}"
        );
        settings.metadata = false;
        let stripped = convert(&retained, &settings);
        assert!(identify(&stripped, "%[profiles]").is_empty(), "{target}");
    }
}

#[test]
fn tiff_alpha_is_preserved_or_composited_for_the_target() {
    for name in ["rgba.tiff", "associated.tiff"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join(name));
        let mut settings = options();
        settings.lossless = true;
        for target in ["tiff", "png", "webp"] {
            settings.target = target.into();
            let output = convert(&source, &settings);
            for x in [0, 1, 2] {
                let pixel = format!("%[fx:round(255*p{{{x},0}}.r)] %[fx:round(255*p{{{x},0}}.g)] %[fx:round(255*p{{{x},0}}.b)] %[fx:round(255*p{{{x},0}}.a)]");
                // Associated alpha loses hidden RGB at alpha=0 in its source,
                // but must retain the correctly unassociated decoded channels.
                let actual: Vec<i32> = identify(&output, &pixel)
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                let expected: Vec<i32> = identify(&source, &pixel)
                    .split_whitespace()
                    .map(|v| v.parse().unwrap())
                    .collect();
                assert_eq!(actual[3], expected[3]);
                // Unpremultiplying 8-bit samples creates fractional channels.
                // PNG/WebP/TIFF quantizers can differ by one 8-bit level.
                let tolerance = i32::from(name == "associated.tiff");
                assert!(
                    actual[..3]
                        .iter()
                        .zip(&expected[..3])
                        .all(|(a, b)| (a - b).abs() <= tolerance),
                    "{name} -> {target}: {actual:?} != {expected:?}"
                );
            }
        }
        for target in ["jpeg", "bmp"] {
            settings.target = target.into();
            settings.background = "#287ec4".into();
            settings.quality = 100;
            let output = convert(&source, &settings);
            assert_eq!(identify(&output, "%[opaque]"), "True");
            if target == "bmp" {
                assert_eq!(bmp_pixel(&output, 0, 0), [40, 126, 196]);
            }
        }
    }
}

#[test]
fn tiff_orientation_metadata_and_profiles_are_handled_before_export() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rotated.tiff", &dir.path().join("rotated.tiff"));
    for target in crate::image_format::ImageFormat::ALL {
        let mut settings = options();
        settings.target = target.id().into();
        settings.resize = 50;
        let output = convert(&source, &settings);
        assert_eq!(identify(&output, "%w %h"), "10 16");
        if settings.target == "tiff" {
            assert_eq!(identify(&output, "%[tiff:artist]"), "Recast test artist");
            let profiles = identify(&output, "%[profiles]");
            assert!(profiles.contains("xmp") && profiles.contains("iptc"));
            assert_ne!(values(&fs::read(&output).unwrap(), 274), [6]);
            settings.metadata = false;
            let stripped = convert(&source, &settings);
            assert!(identify(&stripped, "%[tiff:artist]").is_empty());
            assert!(identify(&stripped, "%[profiles]").is_empty());
        }
    }
    let source = copy("profiled.tiff", &dir.path().join("profiled.tiff"));
    let mut settings = options();
    settings.target = "tiff".into();
    let retained = convert(&source, &settings);
    settings.metadata = false;
    let stripped = convert(&source, &settings);
    let sample = "%[pixel:p{12,8}]";
    assert_ne!(identify(&source, sample), identify(&retained, sample));
    assert_eq!(identify(&retained, sample), identify(&stripped, sample));
    assert!(identify(&retained, "%[profiles]").contains("icc"));
    assert!(identify(&stripped, "%[profiles]").is_empty());
}

fn entry(bytes: &[u8], tag: u16) -> usize {
    let count = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
    (0..count)
        .map(|n| 10 + n * 12)
        .find(|n| u16::from_le_bytes(bytes[*n..*n + 2].try_into().unwrap()) == tag)
        .unwrap()
}

#[test]
fn tiff_multipage_subimages_and_unsupported_samples_never_publish() {
    let base = fs::read(fixture("rgb-le.tiff")).unwrap();
    let mut variants = vec![(fs::read(fixture("multipage.tiff")).unwrap(), "Multipage")];
    for tag in [330u16, 37724, 50706] {
        let mut bytes = base.clone();
        let at = entry(&bytes, 339);
        bytes[at..at + 2].copy_from_slice(&tag.to_le_bytes());
        variants.push((
            bytes,
            if tag == 50706 {
                "DNG"
            } else {
                "subdirectories or layers"
            },
        ));
    }
    let mut floating = base.clone();
    let at = entry(&floating, 339);
    let offset = u32::from_le_bytes(floating[at + 8..at + 12].try_into().unwrap()) as usize;
    floating[offset..offset + 6].copy_from_slice(&[3, 0, 3, 0, 3, 0]);
    variants.push((floating, "unsigned channels"));
    let mut compression = base.clone();
    let at = entry(&compression, 259);
    compression[at + 8..at + 10].copy_from_slice(&50000u16.to_le_bytes());
    variants.push((compression, "compression"));
    for (bytes, message) in variants {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("unsupported.tiff");
        fs::write(&source, &bytes).unwrap();
        let info = serde_json::to_value(crate::inputs::inspect(&source).unwrap()).unwrap();
        assert_eq!(info["targets"], serde_json::json!([]));
        assert!(info["conversionIssue"].as_str().unwrap().contains(message));
        for target in crate::image_format::ImageFormat::ALL {
            let mut settings = options();
            settings.target = target.id().into();
            assert!(
                matches!(backend().convert(&source, None, &settings, &AtomicBool::new(false)), Err(ConversionError::Failed(reason)) if reason.contains(message))
            );
        }
        assert_eq!(fs::read(source).unwrap(), bytes);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn tiff_malformed_offsets_counts_and_payloads_are_rejected() {
    let base = fs::read(fixture("rgb-le.tiff")).unwrap();
    let mut variants = vec![base[..base.len() - 1].to_vec(), base[..18].to_vec()];
    let mut offset = base.clone();
    offset[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    variants.push(offset);
    let mut count = fs::read(fixture("bigtiff-le.tiff")).unwrap();
    count[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    variants.push(count);
    let mut bad_array = base.clone();
    let at = entry(&bad_array, 258);
    bad_array[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    variants.push(bad_array);
    let mut pixels = base.clone();
    let at = entry(&pixels, 273);
    pixels[at + 8..at + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    variants.push(pixels);
    for bytes in variants {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bad.tiff");
        fs::write(&source, bytes).unwrap();
        assert!(crate::inputs::require_supported(&source).is_err());
        assert!(crate::image_format::ImageFormat::Tiff
            .validate_output(&source)
            .is_err());
    }
    // Valid structure, invalid compressed pixels: decoding must fail without a
    // published output, even though bounded container inspection can succeed.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bad-deflate.tiff");
    let mut corrupt = fs::read(fixture("deflate.tiff")).unwrap();
    let at = entry(&corrupt, 273);
    let offset = u32::from_le_bytes(corrupt[at + 8..at + 12].try_into().unwrap()) as usize;
    corrupt[offset..].fill(0xff);
    fs::write(&source, corrupt).unwrap();
    crate::inputs::require_supported(&source).unwrap();
    let mut settings = options();
    settings.target = "tiff".into();
    assert!(backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}
