//! Reject incomplete ADTS frames before a permissive AAC demuxer can skip them.
use crate::image_backend::{check_cancel, ConversionError};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};

pub fn has_adts_signature(bytes: &[u8]) -> bool {
    bytes.len() >= 7 && bytes[0] == 0xff && bytes[1] & 0xf6 == 0xf0 && (bytes[2] >> 2) & 15 < 13
}

pub fn validate_adts(path: &Path, cancel: &AtomicBool) -> Result<(), ConversionError> {
    let invalid = || {
        ConversionError::from(
            "This raw AAC file is incomplete or contains invalid ADTS frames.".to_string(),
        )
    };
    let mut file = File::open(path).map_err(|_| invalid())?;
    let end = file.metadata().map_err(|_| invalid())?.len();
    let mut header = [0u8; 10];
    file.read_exact(&mut header).map_err(|_| invalid())?;
    let mut offset = 0;
    if &header[..3] == b"ID3" {
        if !(2..=4).contains(&header[3]) || header[6..].iter().any(|b| b & 128 != 0) {
            return Err(invalid());
        }
        let size = header[6..]
            .iter()
            .fold(0u64, |n, b| (n << 7) | u64::from(*b));
        offset = 10
            + size
            + if header[3] == 4 && header[5] & 16 != 0 {
                10
            } else {
                0
            };
    }
    let mut format = None;
    while offset < end {
        check_cancel(cancel)?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
        file.read_exact(&mut header[..7]).map_err(|_| invalid())?;
        if !has_adts_signature(&header) {
            return Err(invalid());
        }
        let channels = ((header[2] & 1) << 2) | (header[3] >> 6);
        if channels == 0 || header[6] & 3 != 0 {
            return Err("Raw AAC with program-config elements or multiple blocks per ADTS frame is not supported yet. Use an M4A file.".to_string().into());
        }
        let current = (header[1] & 8, header[2] >> 2, channels);
        if format.is_some_and(|previous| previous != current) {
            return Err("This raw AAC changes profile, sample rate, or channels partway through. Recast cannot preserve it as one consistent stream.".to_string().into());
        }
        format = Some(current);
        let size = (u64::from(header[3] & 3) << 11)
            | (u64::from(header[4]) << 3)
            | u64::from(header[5] >> 5);
        let header_size = if header[1] & 1 == 0 { 9 } else { 7 };
        if size <= header_size || offset + size > end {
            return Err(invalid());
        }
        offset += size;
    }
    if offset != end || format.is_none() {
        return Err(invalid());
    }
    Ok(())
}
