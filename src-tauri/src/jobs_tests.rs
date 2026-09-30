use super::*;
use std::{
    fs,
    sync::atomic::AtomicUsize,
    thread,
    time::{Duration, Instant},
};

fn backend() -> ImageBackend {
    let root = std::env::var_os("RECAST_TEST_BACKEND")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/image-backend")
        });
    let backend = ImageBackend { root };
    backend
        .verify()
        .expect("Run npm run backend:prepare before native tests");
    backend
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn options() -> ImageOptions {
    ImageOptions {
        target: "webp".into(),
        quality: 85,
        lossless: false,
        resize: 100,
        metadata: true,
        background: "#ffffff".into(),
        colors: 256,
        dither: true,
    }
}
fn copy(name: &str, destination: &Path) -> PathBuf {
    fs::copy(fixture(name), destination).unwrap();
    destination.canonicalize().unwrap()
}
fn request(paths: &[PathBuf], folder: &Path) -> ConversionRequest {
    ConversionRequest {
        paths: paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        output_folder: Some(folder.to_string_lossy().into_owned()),
        options: options(),
    }
}
fn identify(path: &Path, format: &str) -> String {
    let result = backend()
        .command()
        .args(["identify", "-format", format])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn no_partials(folder: &Path) {
    assert!(fs::read_dir(folder).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".recast-")));
}

#[test]
fn converts_every_still_image_pair_and_same_format_without_overwriting() {
    for (name, format) in [
        ("rgba.png", "png"),
        ("photo.jpg", "jpeg"),
        ("rgba.webp", "webp"),
        ("photo.webp", "webp"),
        ("rgb.bmp", "bmp"),
        ("rgba.bmp", "bmp"),
        ("top-down.bmp", "bmp"),
        ("palette.bmp", "bmp"),
        ("rle.bmp", "bmp"),
        ("rle4.bmp", "bmp"),
        ("rgb565.bmp", "bmp"),
        ("profiled.bmp", "bmp"),
        ("core.bmp", "bmp"),
        ("rgb-le.tiff", "tiff"),
        ("rgb-be.tiff", "tiff"),
        ("rgba.tiff", "tiff"),
        ("bigtiff-le.tiff", "tiff"),
        ("bigtiff-be.tiff", "tiff"),
        ("deflate.tiff", "tiff"),
        ("packbits.tiff", "tiff"),
        ("tiled.tiff", "tiff"),
        ("planar.tiff", "tiff"),
        ("palette.tiff", "tiff"),
        ("gray16.tiff", "tiff"),
        ("rgb.avif", "avif"),
        ("rgba.avif", "avif"),
        ("rgba12.avif", "avif"),
        ("gray10.avif", "avif"),
        ("still.gif", "gif"),
    ] {
        for target in crate::image_format::ImageFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let source = copy(name, &dir.path().join(name));
            let original = fs::read(&source).unwrap();
            let mut settings = options();
            settings.target = target.id().into();
            let input = crate::inputs::inspect(&source).unwrap();
            let json = serde_json::to_value(input).unwrap();
            assert_eq!(
                json["targets"],
                serde_json::json!(["webp", "jpeg", "png", "bmp", "tiff", "avif", "gif"])
            );
            let output = backend()
                .convert(&source, None, &settings, &AtomicBool::new(false))
                .unwrap_or_else(|error| panic!("{name} -> {target:?}: {error:?}"));
            let output_path = Path::new(&output.path);
            assert_eq!(output_path.extension().unwrap(), target.extension());
            assert_eq!(
                identify(output_path, "%m %w %h %n"),
                format!(
                    "{} 32 20 1",
                    if target == crate::image_format::ImageFormat::Bmp {
                        "BMP3".into()
                    } else {
                        target.id().to_uppercase()
                    }
                )
            );
            assert_eq!(fs::read(&source).unwrap(), original);
            assert_ne!(output_path, source);
            if target.id() == format {
                assert!(output_path
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .ends_with(" (1)"));
            }
            let second = backend()
                .convert(&source, None, &settings, &AtomicBool::new(false))
                .unwrap();
            assert_ne!(output.path, second.path);
            assert_eq!(fs::metadata(output_path).unwrap().len(), output.bytes);
            no_partials(dir.path());
        }
    }
}

#[test]
fn png_output_preserves_rgba_and_ignores_lossy_settings() {
    for name in ["rgba.png", "rgba.webp"] {
        let dir = tempfile::tempdir().unwrap();
        let source = copy(name, &dir.path().join(name));
        let mut settings = options();
        settings.target = "png".into();
        settings.quality = 1;
        settings.lossless = false;
        let output = backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .unwrap();
        let pixels = "%[pixel:p{0,0}]|%[pixel:p{1,0}]|%[pixel:p{15,9}]|%[pixel:p{7,0}]";
        assert_eq!(
            identify(&source, pixels),
            identify(Path::new(&output.path), pixels)
        );
        settings.quality = 100;
        settings.lossless = true;
        let high = backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            identify(Path::new(&output.path), pixels),
            identify(Path::new(&high.path), pixels)
        );
    }
}

#[test]
fn jpeg_composites_transparency_over_the_selected_background() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("transparent.png");
    let result = backend()
        .command()
        .arg(fixture("rgba.png"))
        .args(["-alpha", "transparent"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut settings = options();
    settings.target = "jpeg".into();
    settings.quality = 100;
    settings.lossless = true; // A previous WebP choice must not override JPEG quality.
    for (background, expected) in [
        ("#ffffff", [255, 255, 255]),
        ("#000000", [0, 0, 0]),
        ("#287ec4", [40, 126, 196]),
    ] {
        settings.background = background.into();
        let output = backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .unwrap();
        let channels = identify(
            Path::new(&output.path),
            "%[fx:round(255*r)] %[fx:round(255*g)] %[fx:round(255*b)]",
        );
        let channels: Vec<i32> = channels
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert!(
            channels
                .iter()
                .zip(expected)
                .all(|(actual, expected)| (actual - expected).abs() <= 2),
            "{background}: {channels:?}"
        );
        assert_eq!(identify(Path::new(&output.path), "%[opaque]"), "True");
    }
}

#[test]
fn jpeg_composites_partial_alpha_instead_of_discarding_it() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("half-red.png");
    let result = backend()
        .command()
        .arg(fixture("rgba.png"))
        .args([
            "-fill",
            "#ff0000",
            "-colorize",
            "100%",
            "-alpha",
            "set",
            "-channel",
            "A",
            "-evaluate",
            "set",
            "50%",
            "+channel",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut settings = options();
    settings.target = "jpeg".into();
    settings.quality = 100;
    let output = backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .unwrap();
    let channels = identify(
        Path::new(&output.path),
        "%[fx:round(255*r)] %[fx:round(255*g)] %[fx:round(255*b)]",
    );
    let channels: Vec<i32> = channels
        .split_whitespace()
        .map(|n| n.parse().unwrap())
        .collect();
    assert!(
        channels
            .iter()
            .zip([255, 127, 127])
            .all(|(actual, expected)| (actual - expected).abs() <= 2),
        "{channels:?}"
    );
}

#[test]
fn jpeg_quality_is_independent_of_hidden_webp_lossless_setting() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgb.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.target = "jpeg".into();
    settings.lossless = true;
    settings.quality = 10;
    let low = backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .unwrap();
    settings.quality = 95;
    let high = backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(identify(Path::new(&low.path), "%Q"), "10");
    assert_eq!(identify(Path::new(&high.path), "%Q"), "95");
    assert_ne!(fs::read(low.path).unwrap(), fs::read(high.path).unwrap());
}

#[test]
fn mixed_image_batch_creates_one_output_per_input_for_each_target() {
    for target in crate::image_format::ImageFormat::ALL {
        let dir = tempfile::tempdir().unwrap();
        let sources: Vec<_> = [
            "rgba.png",
            "photo.jpg",
            "rgba.webp",
            "rgb.bmp",
            "rgb-le.tiff",
            "rgba.avif",
        ]
        .into_iter()
        .map(|name| copy(name, &dir.path().join(name)))
        .collect();
        let manager = JobManager::default();
        let mut request = request(&sources, dir.path());
        request.options.target = target.id().into();
        let job = manager.prepare("mixed", request, false).unwrap();
        run_job(&job, &backend(), |_| {});
        let snapshot = job.snapshot();
        assert_eq!(snapshot.status, BatchStatus::Completed);
        assert_eq!(snapshot.files.len(), sources.len());
        let mut paths = HashSet::new();
        for file in snapshot.files {
            assert_eq!(file.status, FileStatus::Succeeded, "{:?}", file.error);
            let path = file.output_path.unwrap();
            assert_eq!(Path::new(&path).extension().unwrap(), target.extension());
            assert!(paths.insert(path));
        }
        no_partials(dir.path());
    }
}

#[test]
fn all_formats_resize_after_orientation_and_handle_metadata() {
    for name in [
        "metadata.png",
        "rotated.jpg",
        "rotated.webp",
        "rotated.avif",
    ] {
        for target in crate::image_format::ImageFormat::ALL {
            let dir = tempfile::tempdir().unwrap();
            let source = copy(name, &dir.path().join(name));
            let mut settings = options();
            settings.target = target.id().into();
            settings.resize = 50;
            let retained = backend()
                .convert(&source, None, &settings, &AtomicBool::new(false))
                .unwrap();
            settings.metadata = false;
            let stripped = backend()
                .convert(&source, None, &settings, &AtomicBool::new(false))
                .unwrap();
            assert_eq!(
                identify(Path::new(&retained.path), "%w %h"),
                "10 16",
                "{name} -> {target:?}"
            );
            assert_eq!(identify(Path::new(&stripped.path), "%w %h"), "10 16");
            let profiles = identify(Path::new(&retained.path), "%[profiles]");
            if matches!(
                target,
                crate::image_format::ImageFormat::Bmp | crate::image_format::ImageFormat::Gif
            ) {
                assert!(profiles.is_empty());
            } else {
                assert!(
                    profiles.contains("icc")
                        && (target == crate::image_format::ImageFormat::Tiff
                            || profiles.contains("exif")),
                    "{name} -> {target:?}: {profiles}"
                );
            }
            assert!(!identify(Path::new(&retained.path), "%[EXIF:Orientation]").contains('6'));
            assert!(identify(Path::new(&stripped.path), "%[profiles]").is_empty());
        }
    }
}

#[test]
fn rejects_animated_webp_for_still_targets_without_publishing() {
    for target in crate::image_format::ImageFormat::ALL
        .into_iter()
        .filter(|format| !["webp", "gif"].contains(&format.id()))
    {
        let dir = tempfile::tempdir().unwrap();
        let source = copy("animated.webp", &dir.path().join("animated.webp"));
        let mut settings = options();
        settings.target = target.id().into();
        let result = backend().convert(&source, None, &settings, &AtomicBool::new(false));
        assert!(
            matches!(result, Err(ConversionError::Failed(reason)) if reason.contains("animated"))
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn rejects_truncated_webp_and_inconsistent_animation_flags() {
    let dir = tempfile::tempdir().unwrap();
    let mut truncated = fs::read(fixture("rgba.webp")).unwrap();
    truncated.truncate(truncated.len() - 3);
    let source = dir.path().join("truncated.webp");
    fs::write(&source, truncated).unwrap();
    assert!(crate::inputs::require_supported(&source)
        .unwrap_err()
        .contains("incomplete"));
    let mut animation = fs::read(fixture("animated.webp")).unwrap();
    assert_eq!(&animation[12..16], b"VP8X");
    animation[20] &= !0x02;
    fs::write(&source, animation).unwrap();
    assert!(crate::inputs::require_supported(&source)
        .unwrap_err()
        .contains("inconsistent"));
}

#[test]
fn invalid_target_and_background_fail_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.target = "invalid-format".into();
    assert!(backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .is_err());
    settings.target = "jpeg".into();
    for invalid in ["white", "#fff", "#gg0000", "#ffffff-extra", "#é0000"] {
        settings.background = invalid.into();
        assert!(backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .is_err());
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn verifies_output_container_and_terminator_before_publication() {
    let dir = tempfile::tempdir().unwrap();
    for (format, name) in [
        (crate::image_format::ImageFormat::Png, "rgb.png"),
        (crate::image_format::ImageFormat::Jpeg, "photo.jpg"),
        (crate::image_format::ImageFormat::WebP, "rgba.webp"),
        (crate::image_format::ImageFormat::Bmp, "rgb.bmp"),
        (crate::image_format::ImageFormat::Tiff, "rgb-le.tiff"),
        (crate::image_format::ImageFormat::Avif, "rgb.avif"),
        (crate::image_format::ImageFormat::Gif, "still.gif"),
    ] {
        assert!(format.validate_output(&fixture(name)).is_ok());
        let path = dir.path().join(name);
        let mut bytes = fs::read(fixture(name)).unwrap();
        bytes.truncate(bytes.len() - 1);
        fs::write(&path, bytes).unwrap();
        assert!(format.validate_output(&path).is_err());
        for other in crate::image_format::ImageFormat::ALL
            .into_iter()
            .filter(|other| *other != format)
        {
            assert!(other.validate_output(&fixture(name)).is_err());
        }
    }
}

#[path = "bmp_tests.rs"]
mod bmp;

#[path = "tiff_tests.rs"]
mod tiff;

#[test]
fn converts_png_and_jpeg_preserving_sources_and_existing_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let png = copy("rgba.png", &dir.path().join("Photo.png"));
    let jpg = copy("photo.jpg", &dir.path().join("Photo.jpg"));
    let existing = dir.path().join("Photo.webp");
    fs::write(&existing, b"existing output must survive").unwrap();
    let png_before = fs::read(&png).unwrap();
    let jpg_before = fs::read(&jpg).unwrap();
    let backend = backend();
    let a = backend
        .convert(&png, None, &options(), &AtomicBool::new(false))
        .unwrap();
    let b = backend
        .convert(&jpg, None, &options(), &AtomicBool::new(false))
        .unwrap();
    assert!(a.path.ends_with("Photo (1).webp"));
    assert!(b.path.ends_with("Photo (2).webp"));
    assert_eq!(identify(Path::new(&a.path), "%w %h"), "32 20");
    assert_eq!(identify(Path::new(&b.path), "%w %h"), "32 20");
    assert_eq!(fs::read(existing).unwrap(), b"existing output must survive");
    assert_eq!(fs::read(png).unwrap(), png_before);
    assert_eq!(fs::read(jpg).unwrap(), jpg_before);
    no_partials(dir.path());
}

#[test]
fn lossless_preserves_rgba_pixels_including_fully_transparent_colors() {
    let dir = tempfile::tempdir().unwrap();
    let png = copy("rgba.png", &dir.path().join("alpha.png"));
    let mut settings = options();
    settings.lossless = true;
    let result = backend()
        .convert(&png, None, &settings, &AtomicBool::new(false))
        .unwrap();
    let checks = "%[pixel:p{0,0}]|%[pixel:p{1,0}]|%[pixel:p{15,9}]|%[pixel:p{7,0}]";
    assert_eq!(
        identify(&png, checks),
        identify(Path::new(&result.path), checks)
    );
}

#[test]
fn honors_resize_quality_and_explicit_lossy_at_quality_100() {
    let dir = tempfile::tempdir().unwrap();
    let png = copy("rgb.png", &dir.path().join("source.png"));
    let mut settings = options();
    settings.resize = 50;
    settings.quality = 10;
    let low = backend()
        .convert(&png, None, &settings, &AtomicBool::new(false))
        .unwrap();
    settings.quality = 100;
    let high = backend()
        .convert(&png, None, &settings, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(identify(Path::new(&high.path), "%w %h"), "16 10");
    let a = fs::read(low.path).unwrap();
    let b = fs::read(high.path).unwrap();
    assert_ne!(a, b);
    assert!(b.windows(4).any(|w| w == b"VP8 "));
    assert!(!b.windows(4).any(|w| w == b"VP8L"));
}

#[test]
fn normalizes_orientation_and_retains_or_strips_metadata() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["metadata.png", "rotated.jpg"] {
        let source = copy(name, &dir.path().join(name));
        let retained = backend()
            .convert(&source, None, &options(), &AtomicBool::new(false))
            .unwrap();
        let mut settings = options();
        settings.metadata = false;
        let stripped = backend()
            .convert(&source, None, &settings, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(identify(Path::new(&retained.path), "%w %h"), "20 32");
        assert_eq!(identify(Path::new(&stripped.path), "%w %h"), "20 32");
        let retained = fs::read(retained.path).unwrap();
        let stripped = fs::read(stripped.path).unwrap();
        assert!(retained.windows(4).any(|w| w == b"EXIF"));
        assert!(!stripped
            .windows(4)
            .any(|w| matches!(w, b"EXIF" | b"ICCP" | b"XMP ")));
    }
}

#[test]
fn converts_profiled_pixels_before_retaining_or_stripping_color_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("linear-rgb.png", &dir.path().join("linear.png"));
    let mut settings = options();
    settings.lossless = true;
    let retained = backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .unwrap();
    settings.metadata = false;
    let stripped = backend()
        .convert(&source, None, &settings, &AtomicBool::new(false))
        .unwrap();
    let pixels = "%[pixel:p{15,9}]|%[pixel:p{7,0}]";
    assert_ne!(
        identify(&source, pixels),
        identify(Path::new(&retained.path), pixels)
    );
    assert_eq!(
        identify(Path::new(&retained.path), pixels),
        identify(Path::new(&stripped.path), pixels)
    );
    assert!(fs::read(retained.path)
        .unwrap()
        .windows(4)
        .any(|w| w == b"ICCP"));
    assert!(!fs::read(stripped.path)
        .unwrap()
        .windows(4)
        .any(|w| w == b"ICCP"));
}

#[test]
fn filenames_are_literal_even_with_brackets_percent_and_unicode() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.png", &dir.path().join("été [1] 50% $photo.png"));
    let result = backend()
        .convert(&source, None, &options(), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(
        Path::new(&result.path).file_name().unwrap(),
        "été [1] 50% $photo.webp"
    );
    assert!(result.bytes > 0);
}

#[test]
fn concurrent_conversions_never_clobber_a_shared_output_name() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgb.png", &dir.path().join("same.png"));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let source = source.clone();
            thread::spawn(move || {
                backend()
                    .convert(&source, None, &options(), &AtomicBool::new(false))
                    .unwrap()
                    .path
            })
        })
        .collect();
    let outputs: HashSet<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(outputs.len(), 4);
    for path in outputs {
        assert_eq!(identify(Path::new(&path), "%w %h"), "32 20");
    }
    no_partials(dir.path());
}

#[test]
fn malformed_input_fails_without_publishing_or_leaving_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.jpg");
    fs::write(&path, b"\xff\xd8\xff\xe0broken image").unwrap();
    let result = backend().convert(&path, None, &options(), &AtomicBool::new(false));
    assert!(matches!(result, Err(ConversionError::Failed(_))));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    no_partials(dir.path());
}

#[test]
fn unwritable_destination_does_not_touch_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy("rgb.png", &dir.path().join("source.png"));
    let result = backend().convert(
        &path,
        Some(&dir.path().join("missing")),
        &options(),
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(ConversionError::Failed(_))));
    assert_eq!(
        fs::read(path).unwrap(),
        fs::read(fixture("rgb.png")).unwrap()
    );
}

#[test]
fn rejects_animated_png_without_flattening() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = fs::read(fixture("rgba.png")).unwrap();
    let marker = [
        0, 0, 0, 8, b'a', b'c', b'T', b'L', 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    bytes.splice(33..33, marker);
    let path = dir.path().join("animated.png");
    fs::write(&path, bytes).unwrap();
    let result = backend().convert(&path, None, &options(), &AtomicBool::new(false));
    assert!(
        matches!(result, Err(ConversionError::Failed(reason)) if reason.contains("Animated PNG"))
    );
    assert!(!dir.path().join("animated.webp").exists());
}

#[test]
fn continues_after_failure_and_retries_only_unfinished_files() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.jpg");
    fs::write(&bad, b"\xff\xd8\xff\xe0broken").unwrap();
    let good = copy("rgba.png", &dir.path().join("good.png"));
    let manager = JobManager::default();
    let request = request(&[bad.clone(), good], dir.path());
    let job = manager.prepare("one", request.clone(), false).unwrap();
    let updates = AtomicUsize::new(0);
    run_job(&job, &backend(), |_| {
        updates.fetch_add(1, Ordering::Relaxed);
    });
    assert!(updates.load(Ordering::Relaxed) >= 5);
    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, BatchStatus::Completed);
    assert_eq!(snapshot.files[0].status, FileStatus::Failed);
    assert_eq!(snapshot.files[1].status, FileStatus::Succeeded);
    let output = snapshot.files[1].output_path.clone().unwrap();
    let bytes = fs::read(&output).unwrap();
    copy("photo.jpg", &bad);
    let retry = manager.prepare("one", request, true).unwrap();
    run_job(&retry, &backend(), |_| {});
    assert!(retry
        .snapshot()
        .files
        .iter()
        .all(|file| file.status == FileStatus::Succeeded));
    assert_eq!(fs::read(output).unwrap(), bytes);
    assert!(!dir.path().join("good (1).webp").exists());
    no_partials(dir.path());
}

#[test]
fn cancelling_preserves_completed_files_and_retry_does_not_duplicate_them() {
    let dir = tempfile::tempdir().unwrap();
    let a = copy("rgb.png", &dir.path().join("first.png"));
    let b = copy("photo.jpg", &dir.path().join("second.jpg"));
    let manager = JobManager::default();
    let request = request(&[a, b], dir.path());
    let job = manager.prepare("one", request.clone(), false).unwrap();
    run_job(&job, &backend(), |snapshot| {
        if snapshot.files[0].status == FileStatus::Succeeded {
            job.request_cancel();
        }
    });
    let snapshot = job.snapshot();
    assert_eq!(snapshot.status, BatchStatus::Cancelled);
    assert_eq!(snapshot.files[0].status, FileStatus::Succeeded);
    assert_eq!(snapshot.files[1].status, FileStatus::Cancelled);
    assert!(!dir.path().join("second.webp").exists());
    let retry = manager.prepare("one", request, true).unwrap();
    run_job(&retry, &backend(), |_| {});
    assert!(retry
        .snapshot()
        .files
        .iter()
        .all(|file| file.status == FileStatus::Succeeded));
    assert!(!dir.path().join("first (1).webp").exists());
    assert!(dir.path().join("second.webp").exists());
    // A late cancel must not put a completed job back into Cancelling.
    assert_eq!(retry.request_cancel().status, BatchStatus::Completed);
    no_partials(dir.path());
}

#[test]
fn active_cancellation_cleans_the_private_output() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgba.png", &dir.path().join("cancel.png"));
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let worker = thread::spawn(move || backend().convert(&source, None, &options(), &flag));
    let started = Instant::now();
    while !fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".recast-")
    }) {
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(1));
    }
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(
        worker.join().unwrap(),
        Err(ConversionError::Cancelled)
    ));
    assert!(!dir.path().join("cancel.webp").exists());
    no_partials(dir.path());
}

#[test]
fn active_window_rejects_another_run_but_other_windows_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let source = copy("rgb.png", &dir.path().join("source.png"));
    let manager = JobManager::default();
    let request = request(&[source], dir.path());
    manager.prepare("one", request.clone(), false).unwrap();
    assert!(manager.prepare("one", request.clone(), false).is_err());
    assert!(manager.prepare("two", request, false).is_ok());
    let job = manager.cancel_before_close("one").unwrap();
    run_job(&job, &backend(), |_| {});
    assert_eq!(job.snapshot().status, BatchStatus::Cancelled);
    assert!(manager.0.lock().unwrap()["two"].active());
}

#[path = "avif_tests.rs"]
mod avif;

#[test]
fn quitting_cancels_all_windows_preserves_outputs_and_blocks_new_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let first = copy("rgb.png", &dir.path().join("first.png"));
    let second = copy("rgba.png", &dir.path().join("second.png"));
    let manager = JobManager::default();
    let request = request(&[first, second], dir.path());
    let one = manager.prepare("one", request.clone(), false).unwrap();
    let two = manager.prepare("two", request.clone(), false).unwrap();
    run_job(&one, &backend(), |snapshot| {
        if snapshot.status == BatchStatus::Running
            && snapshot.files[0].status == FileStatus::Succeeded
        {
            assert_eq!(manager.cancel_before_exit().len(), 2);
        }
    });
    assert!(manager.prepare("three", request.clone(), false).is_err());
    assert!(manager.prepare("one", request, true).is_err());
    run_job(&two, &backend(), |_| {});
    assert_eq!(one.snapshot().status, BatchStatus::Cancelled);
    assert_eq!(one.snapshot().files[0].status, FileStatus::Succeeded);
    assert_eq!(one.snapshot().files[1].status, FileStatus::Cancelled);
    assert_eq!(two.snapshot().status, BatchStatus::Cancelled);
    assert!(two
        .snapshot()
        .files
        .iter()
        .all(|file| file.status == FileStatus::Cancelled));
    assert!(dir.path().join("first.webp").is_file());
    assert!(!dir.path().join("first (1).webp").exists());
    assert!(!dir.path().join("second.webp").exists());
    assert!(manager.cancel_before_exit().is_empty());
    no_partials(dir.path());
}

#[path = "animation_tests.rs"]
mod animation;
