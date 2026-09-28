use serde::Serialize;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputFile {
    pub id: String,
    pub name: String,
    pub path: String,
    kind: String,
    format: String,
    bytes: u64,
    // Stream inspection is a later backend step. Unknown is never treated as true.
    has_audio: Option<bool>,
    targets: Vec<String>,
    conversion_issue: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InputError {
    name: String,
    message: String,
}

#[derive(Debug, Serialize, Default)]
pub struct Inspection {
    files: Vec<InputFile>,
    errors: Vec<InputError>,
}

pub fn inspect(path: &Path) -> Result<InputFile, String> {
    let mut file = File::open(path).map_err(|_| "This file could not be opened.")?;
    let metadata = file
        .metadata()
        .map_err(|_| "This file could not be read.")?;
    if !metadata.is_file() {
        return Err("Choose individual files, rather than a folder.".into());
    }
    let mut header = [0u8; 8192];
    let read = file
        .read(&mut header)
        .map_err(|_| "This file could not be read.")?;
    let detected = infer::get(&header[..read]).ok_or("This file type is not recognized yet.")?;
    let kind = match detected.mime_type().split('/').next() {
        Some("image") => "images",
        Some("audio") => "audio",
        Some("video") => "video",
        _ => return Err("This file type is not supported in the current prototype.".into()),
    };
    let canonical = path
        .canonicalize()
        .map_err(|_| "This file is no longer available.")?;
    let path_string = canonical.to_string_lossy().into_owned();
    let conversion_issue = match detected.extension() {
        "jpg" => None,
        "png" => png_conversion_issue(&mut file),
        _ => Some(
            "This build converts PNG and JPEG images to WebP. This file is not supported yet."
                .into(),
        ),
    };
    let targets = if conversion_issue.is_none() {
        vec!["webp".into()]
    } else {
        vec![]
    };
    Ok(InputFile {
        id: path_string.clone(),
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: path_string,
        kind: kind.into(),
        format: match detected.extension() {
            "jpg" => "JPEG".into(),
            "webp" => "WebP".into(),
            extension => extension.to_uppercase(),
        },
        bytes: metadata.len(),
        has_audio: None,
        targets,
        conversion_issue,
    })
}

// APNG must not silently become a still image. acTL precedes the first IDAT.
fn png_conversion_issue(file: &mut File) -> Option<String> {
    let mut check = || -> Result<bool, std::io::Error> {
        file.seek(SeekFrom::Start(8))?;
        let length = file.metadata()?.len();
        loop {
            let mut chunk = [0; 8];
            file.read_exact(&mut chunk)?;
            let count = u32::from_be_bytes(chunk[..4].try_into().unwrap()) as u64;
            if &chunk[4..] == b"acTL" {
                return Ok(true);
            }
            if &chunk[4..] == b"IDAT" {
                return Ok(false);
            }
            if &chunk[4..] == b"IEND" || file.stream_position()?.saturating_add(count + 4) > length
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Incomplete PNG",
                ));
            }
            file.seek(SeekFrom::Current((count + 4) as i64))?;
        }
    };
    match check() {
        Ok(false) => None,
        Ok(true) => Some("Animated PNG conversion is not supported yet. Remove this file to convert the rest to WebP.".into()),
        Err(_) => Some("This PNG is incomplete or unreadable.".into()),
    }
}

pub fn require_supported(path: &Path) -> Result<InputFile, String> {
    let input = inspect(path)?;
    if let Some(issue) = &input.conversion_issue {
        return Err(issue.clone());
    }
    Ok(input)
}

#[tauri::command]
pub async fn inspect_inputs(paths: Vec<String>) -> Result<Inspection, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut result = Inspection::default();
        for path in paths {
            match inspect(Path::new(&path)) {
                Ok(file) => result.files.push(file),
                Err(message) => result.errors.push(InputError {
                    name: Path::new(&path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    message,
                }),
            }
        }
        result
    })
    .await
    .map_err(|_| "File inspection could not finish.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_content_despite_a_misleading_extension() {
        let mut file = tempfile::Builder::new().suffix(".mp3").tempfile().unwrap();
        file.write_all(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        let input = inspect(file.path()).unwrap();
        assert_eq!(input.kind, "images");
        assert_eq!(input.format, "PNG");
        assert_eq!(input.has_audio, None);
    }

    #[test]
    fn rejects_unrecognized_content_without_trusting_the_name() {
        let mut file = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
        file.write_all(b"This is not an image.").unwrap();
        assert!(inspect(file.path()).is_err());
    }

    #[test]
    fn rejects_directories() {
        let directory = tempfile::tempdir().unwrap();
        assert!(inspect(directory.path()).is_err());
    }
}
