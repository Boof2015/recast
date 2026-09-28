use crate::inputs;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

#[derive(Debug, Clone)]
pub struct ImageBackend {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOptions {
    pub target: String,
    pub quality: u8,
    pub lossless: bool,
    pub resize: u8,
    pub metadata: bool,
}

impl ImageOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.target != "webp" {
            return Err("Only WebP output is available in this build.".into());
        }
        if !(1..=100).contains(&self.quality) {
            return Err("Quality must be between 1 and 100.".into());
        }
        if ![100, 75, 50, 25].contains(&self.resize) {
            return Err("Choose Original, 75%, 50%, or 25% for resize.".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug)]
pub enum ConversionError {
    Cancelled,
    Failed(String),
}
impl From<String> for ConversionError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

impl ImageBackend {
    pub fn command(&self) -> Command {
        let mut command = Command::new(self.root.join(if cfg!(windows) {
            "magick.exe"
        } else {
            "magick"
        }));
        // Absolute bundled executable; no external delegates or configuration.
        command
            .env("MAGICK_CONFIGURE_PATH", &self.root)
            .env("LC_ALL", "C");
        command.stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        command
    }

    pub fn verify(&self) -> Result<(), String> {
        let result = self.command().arg("-version").output().map_err(|_| {
            "The bundled image converter is missing. Rebuild Recast with its image backend."
                .to_string()
        })?;
        let text = String::from_utf8_lossy(&result.stdout);
        if !result.status.success()
            || !["ImageMagick 7.1.2-32", "jpeg", "png", "webp", "lcms"]
                .iter()
                .all(|value| text.contains(value))
        {
            return Err("The bundled image converter does not have the expected PNG, JPEG, WebP, and color support.".into());
        }
        if !self.root.join("sRGB.icc").is_file() {
            return Err("The bundled color profile is missing.".into());
        }
        Ok(())
    }

    pub fn convert(
        &self,
        source: &Path,
        folder: Option<&Path>,
        options: &ImageOptions,
        cancel: &AtomicBool,
    ) -> Result<OutputFile, ConversionError> {
        options.validate()?;
        check_cancel(cancel)?;
        let input = inputs::require_supported(source)?;
        let source = Path::new(&input.path);
        let parent = folder.unwrap_or_else(|| source.parent().unwrap());
        if !parent.is_dir() {
            return Err(
                "The output folder is no longer available. Choose another folder."
                    .to_string()
                    .into(),
            );
        }
        let work = tempfile::Builder::new()
            .prefix("recast-image-")
            .tempdir()
            .map_err(|_| "A temporary work folder could not be created.".to_string())?;
        // Controlled input names avoid ImageMagick's filename expansion syntax
        // (brackets, percent signs, prefixes) for arbitrary user filenames.
        let staged = work.path().join("input");
        let mut source_file =
            File::open(source).map_err(|_| "The source file could not be opened.".to_string())?;
        let mut staged_file =
            File::create(&staged).map_err(|_| "The image could not be prepared.".to_string())?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let read = source_file
                .read(&mut buffer)
                .map_err(|_| "The source file could not be read.".to_string())?;
            if read == 0 {
                break;
            }
            staged_file.write_all(&buffer[..read]).map_err(|_| {
                "The image could not be prepared. Check available disk space.".to_string()
            })?;
        }
        drop(staged_file);
        // Recheck staged bytes in case the source changed while it was copied.
        inputs::require_supported(&staged)?;
        let output = tempfile::Builder::new()
            .prefix(".recast-")
            .suffix(".webp")
            .tempfile_in(parent)
            .map_err(|_| {
                "Cannot write to this output folder. Choose another folder and try again."
                    .to_string()
            })?;
        let diagnostics_path = work.path().join("diagnostics.txt");
        let diagnostics = File::create(&diagnostics_path)
            .map_err(|_| "The image converter could not start.".to_string())?;
        let mut command = self.command();
        command
            .current_dir(work.path())
            .env("MAGICK_TEMPORARY_PATH", work.path());
        command.arg(&staged).arg("-auto-orient");
        if options.resize != 100 {
            command.args(["-resize", &format!("{}%", options.resize)]);
        }
        // WebP stores RGB pixels. Convert embedded profiles (including CMYK)
        // before encoding so retained ICC data describes the resulting pixels.
        // The colorspace fallback also handles unprofiled CMYK/gray inputs.
        command
            .arg("-profile")
            .arg(self.root.join("sRGB.icc"))
            .args(["-colorspace", "sRGB"]);
        if !options.metadata {
            command.arg("-strip");
        }
        command.args([
            "-quality",
            &if options.lossless {
                "100".into()
            } else {
                options.quality.to_string()
            },
        ]);
        command.args([
            "-define",
            if options.lossless {
                "webp:lossless=true"
            } else {
                "webp:lossless=false"
            },
            "-define",
            "webp:exact=true",
            "webp:-",
        ]);
        command.stdout(Stdio::from(
            output
                .reopen()
                .map_err(|_| "The output file could not be opened.".to_string())?,
        ));
        command.stderr(Stdio::from(diagnostics));
        let mut child = command
            .spawn()
            .map_err(|_| "The bundled image converter could not start.".to_string())?;
        let started = Instant::now();
        let status = loop {
            if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(300) {
                let _ = child.kill();
                let _ = child.wait();
                check_cancel(cancel)?;
                return Err("This image took too long to convert. Try a smaller image."
                    .to_string()
                    .into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(35)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("The image converter stopped unexpectedly."
                        .to_string()
                        .into());
                }
            }
        };
        check_cancel(cancel)?;
        if !status.success() {
            let mut detail = String::new();
            if let Ok(file) = File::open(diagnostics_path) {
                let _ = file.take(4096).read_to_string(&mut detail);
            }
            let detail = detail.replace(staged.to_string_lossy().as_ref(), &input.name);
            let detail = detail
                .lines()
                .next()
                .unwrap_or("The image data could not be converted.")
                .trim_start_matches("magick: ");
            return Err(format!("Conversion failed: {detail}").into());
        }
        let bytes = output
            .as_file()
            .metadata()
            .map_err(|_| "The output could not be verified.".to_string())?
            .len();
        let mut header = [0; 12];
        File::open(output.path())
            .and_then(|mut file| file.read_exact(&mut header))
            .map_err(|_| "The converter did not produce a complete WebP file.".to_string())?;
        if &header[..4] != b"RIFF"
            || &header[8..] != b"WEBP"
            || u32::from_le_bytes(header[4..8].try_into().unwrap()) as u64 + 8 != bytes
        {
            return Err("The converter did not produce a valid WebP file."
                .to_string()
                .into());
        }
        output.as_file().sync_all().map_err(|_| {
            "The output could not be saved. Check available disk space.".to_string()
        })?;
        check_cancel(cancel)?;
        let path = publish(output, source.file_stem().unwrap_or_default(), parent)?;
        Ok(OutputFile {
            path: path.to_string_lossy().into_owned(),
            bytes,
        })
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ConversionError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ConversionError::Cancelled)
    } else {
        Ok(())
    }
}

fn publish(
    mut temporary: NamedTempFile,
    stem: &std::ffi::OsStr,
    folder: &Path,
) -> Result<PathBuf, String> {
    for index in 0..10_000 {
        let mut name = stem.to_os_string();
        if index > 0 {
            name.push(format!(" ({index})"));
        }
        name.push(".webp");
        let destination = folder.join(name);
        // Atomic no-clobber publication also protects concurrent windows and
        // unrelated processes creating the same destination during conversion.
        match temporary.persist_noclobber(&destination) {
            Ok(file) => { drop(file); return Ok(destination); }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => temporary = error.file,
            Err(_) => return Err("The output could not be saved in this folder. Choose another folder and try again.".into()),
        }
    }
    Err("Too many files already use this output name. Choose another folder.".into())
}
