//! Check complete Layer III frames before trusting a permissive demuxer.
//! FFmpeg can otherwise decode a truncated final frame without an error.
use crate::image_backend::{check_cancel, ConversionError};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};

pub fn has_signature(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"ID3") {
        return true;
    }
    if bytes.len() < 4 {
        return false;
    }
    let h = u32::from_be_bytes(bytes[..4].try_into().unwrap());
    h & 0xffe00000 == 0xffe00000
        && (h >> 19) & 3 != 1
        && (h >> 17) & 3 == 1
        && (h >> 12) & 15 != 15
        && (h >> 10) & 3 != 3
}

pub fn validate(path: &Path, cancel: &AtomicBool) -> Result<(), ConversionError> {
    let invalid = || {
        ConversionError::from(
            "This MP3 is incomplete or contains invalid audio frames.".to_string(),
        )
    };
    let mut file = File::open(path).map_err(|_| invalid())?;
    let mut end = file.metadata().map_err(|_| invalid())?.len();
    let mut start = 0u64;
    let mut header = [0u8; 10];
    if end < 10 {
        return Err(invalid());
    }
    file.read_exact(&mut header).map_err(|_| invalid())?;
    if &header[..3] == b"ID3" {
        if !(2..=4).contains(&header[3]) || header[6..].iter().any(|b| b & 128 != 0) {
            return Err(invalid());
        }
        let size = header[6..]
            .iter()
            .fold(0u64, |n, b| (n << 7) | u64::from(*b));
        start = 10
            + size
            + if header[3] == 4 && header[5] & 16 != 0 {
                10
            } else {
                0
            };
    }
    // Ordinary trailing ID3v1 and APEv2 tags are metadata, not MPEG frames.
    if end >= 128 {
        file.seek(SeekFrom::Start(end - 128))
            .map_err(|_| invalid())?;
        file.read_exact(&mut header[..3]).map_err(|_| invalid())?;
        if &header[..3] == b"TAG" {
            end -= 128;
        }
    }
    if end >= 32 {
        let mut footer = [0u8; 32];
        file.seek(SeekFrom::Start(end - 32))
            .map_err(|_| invalid())?;
        file.read_exact(&mut footer).map_err(|_| invalid())?;
        if &footer[..8] == b"APETAGEX" {
            let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
            let flags = u32::from_le_bytes(footer[20..24].try_into().unwrap());
            if size < 32 {
                return Err(invalid());
            }
            end = end
                .checked_sub(size + if flags & 0x80000000 != 0 { 32 } else { 0 })
                .ok_or_else(invalid)?;
        }
    }
    if start >= end {
        return Err(invalid());
    }
    let mut offset = start;
    let mut frames = 0u64;
    let mut format = None;
    let mut expected_frames = None;
    let mut frame = [0u8; 2881];
    while offset < end {
        check_cancel(cancel)?;
        file.seek(SeekFrom::Start(offset)).map_err(|_| invalid())?;
        if end - offset < 4 {
            return Err(invalid());
        }
        file.read_exact(&mut frame[..4]).map_err(|_| invalid())?;
        let h = u32::from_be_bytes(frame[..4].try_into().unwrap());
        // Some encoders pad the file with zero bytes outside its audio frames.
        if h == 0 && frames > 0 {
            let mut remaining = end - offset - 4;
            while remaining > 0 {
                check_cancel(cancel)?;
                let n = remaining.min(frame.len() as u64) as usize;
                file.read_exact(&mut frame[..n]).map_err(|_| invalid())?;
                if frame[..n].iter().any(|b| *b != 0) {
                    return Err(invalid());
                }
                remaining -= n as u64;
            }
            break;
        }
        let version = (h >> 19) & 3;
        let index = ((h >> 12) & 15) as usize;
        let rate_index = ((h >> 10) & 3) as usize;
        if h & 0xffe00000 != 0xffe00000
            || version == 1
            || (h >> 17) & 3 != 1
            || index == 15
            || rate_index == 3
        {
            return Err(invalid());
        }
        if index == 0 {
            return Err(
                "Free-format MP3 is not supported yet. Choose a standard-bitrate or VBR MP3."
                    .to_string()
                    .into(),
            );
        }
        let rate = [44100, 48000, 32000][rate_index]
            / if version == 3 {
                1
            } else if version == 2 {
                2
            } else {
                4
            };
        let channels = if (h >> 6) & 3 == 3 { 1 } else { 2 };
        let current = (rate, channels);
        if format.is_some_and(|previous| previous != current) {
            return Err("This MP3 changes sample rate or channel count partway through. Recast cannot preserve it as one consistent stream.".to_string().into());
        }
        format = Some(current);
        let bitrate = if version == 3 {
            [
                0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
            ][index]
        } else {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][index]
        };
        let size = ((if version == 3 { 144000 } else { 72000 }) * bitrate / rate + ((h >> 9) & 1))
            as usize;
        if size < 4 || size > frame.len() || offset + size as u64 > end {
            return Err(invalid());
        }
        file.read_exact(&mut frame[4..size])
            .map_err(|_| invalid())?;
        let side = match (version == 3, channels) {
            (true, 1) => 17,
            (true, _) => 32,
            (false, 1) => 9,
            _ => 17,
        };
        let xing = 4 + side; // Xing writers reserve the normal side-info offset, even with CRC.
        if size >= xing + 12 && [&b"Xing"[..], &b"Info"[..]].contains(&&frame[xing..xing + 4]) {
            if frames != 0 {
                return Err("This MP3 contains another stream header partway through. Combined MP3 files are not supported yet.".to_string().into());
            }
            let flags = u32::from_be_bytes(frame[xing + 4..xing + 8].try_into().unwrap());
            if flags & 1 != 0 {
                expected_frames = Some(
                    u64::from(u32::from_be_bytes(
                        frame[xing + 8..xing + 12].try_into().unwrap(),
                    )) + 1,
                );
            }
        }
        offset += size as u64;
        frames += 1;
    }
    if frames == 0 || expected_frames.is_some_and(|expected| expected != frames) {
        return Err(invalid());
    }
    Ok(())
}
