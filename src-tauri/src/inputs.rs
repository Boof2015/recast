use crate::image_format::ImageFormat;
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
    if header[..read].starts_with(b"BA") {
        return Err("BMP bitmap arrays are not supported. Choose a single-image BMP.".into());
    }
    let is_tiff = crate::tiff::has_signature(&header[..read]);
    let detected = infer::get(&header[..read]);
    // infer only recognizes classic TIFF; inspect BigTIFF by its full signature.
    let (mime, extension) = if is_tiff {
        ("image/tiff", "tiff")
    } else if crate::avif::has_signature(&header[..read]) {
        ("image/avif", "avif")
    } else {
        let detected = detected.ok_or("This file type is not recognized yet.")?;
        (detected.mime_type(), detected.extension())
    };
    let kind = match mime.split('/').next() {
        Some("image") => "images",
        Some("audio") => "audio",
        Some("video") => "video",
        _ => return Err("This file type is not supported in the current prototype.".into()),
    };
    let canonical = path
        .canonicalize()
        .map_err(|_| "This file is no longer available.")?;
    let path_string = canonical.to_string_lossy().into_owned();
    let conversion_issue = match extension {
        "jpg" => None,
        "png" => png_conversion_issue(&mut file),
        "webp" => webp_conversion_issue(&mut file),
        "bmp" => bmp_conversion_issue(&mut file),
        "tiff" => crate::tiff::inspect(&mut file).err(),
        "avif" => crate::avif::inspect(&mut file).err(),
        _ => Some(
            "This build converts still PNG, JPEG, WebP, BMP, TIFF, and AVIF images. This file is not supported yet."
                .into(),
        ),
    };
    let targets = if conversion_issue.is_none() {
        ImageFormat::ALL
            .into_iter()
            .map(|format| format.id().into())
            .collect()
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
        format: match extension {
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

fn bmp_conversion_issue(file: &mut File) -> Option<String> {
    let mut check = || -> std::io::Result<Option<String>> {
        let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "Incomplete BMP");
        file.rewind()?;
        let length = file.metadata()?.len();
        let mut header = [0; 18];
        file.read_exact(&mut header)?;
        let file_size = u64::from(u32::from_le_bytes(header[2..6].try_into().unwrap()));
        let offset = u64::from(u32::from_le_bytes(header[10..14].try_into().unwrap()));
        let dib_size = u32::from_le_bytes(header[14..18].try_into().unwrap());
        // A normal BMP has one file header and one bitmap. This also rejects
        // ordinary concatenated BMPs instead of sending a sequence to an encoder.
        if &header[..2] != b"BM"
            || file_size != length
            || ![12, 40, 52, 56, 64, 78, 108, 124].contains(&dib_size)
            || offset < 14 + u64::from(dib_size)
            || offset >= length
        {
            return Err(invalid());
        }
        let mut dib = [0; 124];
        dib[..4].copy_from_slice(&header[14..18]);
        file.read_exact(&mut dib[4..dib_size as usize])?;
        let (width, height, planes, bits, compression) = if dib_size == 12 {
            (
                i32::from(u16::from_le_bytes(dib[4..6].try_into().unwrap())),
                i32::from(u16::from_le_bytes(dib[6..8].try_into().unwrap())),
                u16::from_le_bytes(dib[8..10].try_into().unwrap()),
                u16::from_le_bytes(dib[10..12].try_into().unwrap()),
                0,
            )
        } else {
            (
                i32::from_le_bytes(dib[4..8].try_into().unwrap()),
                i32::from_le_bytes(dib[8..12].try_into().unwrap()),
                u16::from_le_bytes(dib[12..14].try_into().unwrap()),
                u16::from_le_bytes(dib[14..16].try_into().unwrap()),
                u32::from_le_bytes(dib[16..20].try_into().unwrap()),
            )
        };
        // Embedded PNG/JPEG can hide a different container (including APNG).
        // Supporting those wrappers needs their own inspection path first.
        if [4, 5].contains(&compression) {
            return Ok(Some("BMP files containing embedded PNG or JPEG are not supported yet. Remove this file to convert the rest.".into()));
        }
        if width <= 0
            || height == 0
            || planes != 1
            || ![1, 4, 8, 16, 24, 32].contains(&bits)
            || ![0, 1, 2, 3, 6].contains(&compression)
        {
            return Err(invalid());
        }
        if [0, 3, 6].contains(&compression) {
            let row_bytes = (width as u64 * u64::from(bits)).div_ceil(32) * 4;
            let pixel_end = offset + row_bytes * u64::from(height.unsigned_abs());
            if pixel_end > length {
                return Err(invalid());
            }
        }
        Ok(None)
    };
    check().unwrap_or_else(|_| {
        Some("This BMP is incomplete, unreadable, or contains more than one bitmap.".into())
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
        Ok(true) => Some(
            "Animated PNG conversion is not supported yet. Remove this file to convert the rest."
                .into(),
        ),
        Err(_) => Some("This PNG is incomplete or unreadable.".into()),
    }
}

// Check both the feature flag and frame chunks: never flatten an animation,
// even when its VP8X animation flag is missing or inconsistent.
fn webp_conversion_issue(file: &mut File) -> Option<String> {
    let mut check = || -> std::io::Result<bool> {
        let invalid = || std::io::Error::new(std::io::ErrorKind::InvalidData, "Incomplete WebP");
        file.seek(SeekFrom::Start(0))?;
        let length = file.metadata()?.len();
        let mut header = [0; 12];
        file.read_exact(&mut header)?;
        if &header[..4] != b"RIFF"
            || &header[8..] != b"WEBP"
            || u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8 != length
        {
            return Err(invalid());
        }
        let mut images = 0;
        while file.stream_position()? < length {
            let mut chunk = [0; 8];
            file.read_exact(&mut chunk)?;
            let count = u64::from(u32::from_le_bytes(chunk[4..].try_into().unwrap()));
            let end = file.stream_position()? + count + (count % 2);
            if end > length {
                return Err(invalid());
            }
            match &chunk[..4] {
                b"ANIM" | b"ANMF" => return Ok(true),
                b"VP8X" => {
                    if count != 10 {
                        return Err(invalid());
                    }
                    let mut flags = [0];
                    file.read_exact(&mut flags)?;
                    if flags[0] & 0x02 != 0 {
                        return Ok(true);
                    }
                }
                b"VP8 " | b"VP8L" => images += 1,
                _ => {}
            }
            file.seek(SeekFrom::Start(end))?;
        }
        if images != 1 {
            return Err(invalid());
        }
        Ok(false)
    };
    match check() {
        Ok(false) => None,
        Ok(true) => Some(
            "Animated WebP conversion is not supported yet. Remove this file to convert the rest."
                .into(),
        ),
        Err(_) => Some("This WebP is incomplete or unreadable.".into()),
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
