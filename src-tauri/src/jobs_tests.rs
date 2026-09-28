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
