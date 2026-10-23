use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    WebP,
    Jpeg,
    Png,
    Bmp,
    Tiff,
    Avif,
    Gif,
}

impl ImageFormat {
    pub const ALL: [Self; 7] = [
        Self::WebP,
        Self::Jpeg,
        Self::Png,
        Self::Bmp,
        Self::Tiff,
        Self::Avif,
        Self::Gif,
    ];

    pub fn from_id(id: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|format| format.id() == id)
            .ok_or_else(|| "Choose PNG, JPEG, WebP, BMP, TIFF, AVIF, or GIF for the output.".into())
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::WebP => "webp",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Avif => "avif",
            Self::Gif => "gif",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            _ => self.id(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::WebP => "WebP",
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
            Self::Avif => "AVIF",
            Self::Gif => "GIF",
        }
    }

    // Check the encoder's container and terminator before publishing. Actual
    // decoding is exercised by real-worker tests; this is not a second decoder.
    pub fn validate_output(self, path: &Path) -> Result<u64, String> {
        let check = || -> std::io::Result<Option<u64>> {
            let mut file = File::open(path)?;
            let bytes = file.metadata()?.len();
            let mut header = [0u8; 12];
            file.read_exact(&mut header)?;
            let valid = match self {
                Self::Gif => crate::animation::inspect_gif(&mut file).is_ok(),
                Self::Tiff => crate::tiff::inspect(&mut file).is_ok(),
                Self::Avif => crate::avif::inspect(&mut file).is_ok(),
                Self::WebP => crate::animation::inspect_webp(&mut file).is_ok(),
                Self::Jpeg => {
                    let mut end = [0; 2];
                    file.seek(SeekFrom::End(-2))?;
                    file.read_exact(&mut end)?;
                    header[..3] == [0xff, 0xd8, 0xff] && end == [0xff, 0xd9]
                }
                Self::Png => {
                    let mut end = [0; 12];
                    file.seek(SeekFrom::End(-12))?;
                    file.read_exact(&mut end)?;
                    bytes >= 57
                        && &header[..8] == b"\x89PNG\r\n\x1a\n"
                        && header[8..12] == [0, 0, 0, 13]
                        && &end == b"\0\0\0\0IEND\xae\x42\x60\x82"
                }
                Self::Bmp => {
                    let mut bmp = [0; 54];
                    file.rewind()?;
                    file.read_exact(&mut bmp)?;
                    let u32_at =
                        |start| u32::from_le_bytes(bmp[start..start + 4].try_into().unwrap());
                    let width = i32::from_le_bytes(bmp[18..22].try_into().unwrap());
                    let height = i32::from_le_bytes(bmp[22..26].try_into().unwrap());
                    // Recast writes precisely a V3 header + bottom-up, padded
                    // 24-bit RGB rows. Reject short or unexpected output layouts.
                    let row_bytes = (u64::from(width.unsigned_abs()) * 3).div_ceil(4) * 4;
                    let pixels = row_bytes * u64::from(height.unsigned_abs());
                    &bmp[..2] == b"BM"
                        && u64::from(u32_at(2)) == bytes
                        && u32_at(10) == 54
                        && u32_at(14) == 40
                        && width > 0
                        && height > 0
                        && bmp[26..30] == [1, 0, 24, 0]
                        && u32_at(30) == 0
                        && u64::from(u32_at(34)) == pixels
                        && bytes == 54 + pixels
                }
            };
            Ok(valid.then_some(bytes))
        };
        check().ok().flatten().ok_or_else(|| {
            format!(
                "The converter did not produce a complete {} file.",
                self.label()
            )
        })
    }
}
