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
}

impl ImageFormat {
    pub const ALL: [Self; 3] = [Self::WebP, Self::Jpeg, Self::Png];

    pub fn from_id(id: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|format| format.id() == id)
            .ok_or_else(|| "Choose PNG, JPEG, or WebP for the output.".into())
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::WebP => "webp",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
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
                Self::WebP => {
                    bytes > 20
                        && &header[..4] == b"RIFF"
                        && &header[8..] == b"WEBP"
                        && u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap())) + 8
                            == bytes
                }
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
