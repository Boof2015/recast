//! Ogg Vorbis framing checks; complete PCM length is checked after decoding.
use crate::{
    image_backend::ConversionError,
    ogg::{invalid, Reader},
};
use std::{path::Path, sync::atomic::AtomicBool};

pub fn validate(path: &Path, cancel: &AtomicBool) -> Result<u64, ConversionError> {
    let mut reader = Reader::open(path)?;
    let mut count = 0;
    let mut last = 0;
    let mut complete = None;
    while let Some(page) = reader.next(cancel)? {
        let before = count;
        for (index, packet) in page.packets.iter().enumerate() {
            match count {
                0 => {
                    if packet.len() != 30
                        || !packet.starts_with(b"\x01vorbis")
                        || packet[7..11] != [0, 0, 0, 0]
                        || !(1..=8).contains(&packet[11])
                        || u32::from_le_bytes(packet[12..16].try_into().unwrap()) == 0
                        || !(6..=13).contains(&(packet[28] & 15))
                        || !(6..=13).contains(&(packet[28] >> 4))
                        || packet[28] & 15 > packet[28] >> 4
                        || packet[29] != 1
                        || index + 1 != page.packets.len()
                        || page.continued
                    {
                        return Err(invalid());
                    }
                }
                1 if !packet.starts_with(b"\x03vorbis") => return Err(invalid()),
                2 if !packet.starts_with(b"\x05vorbis") => return Err(invalid()),
                1 | 2 => (),
                _ if packet.first().is_none_or(|b| b & 1 != 0) => return Err(invalid()),
                _ => (),
            }
            count += 1;
        }
        if count > 3 && count > before {
            if page.granule > i64::MAX as u64 || page.granule < last {
                return Err(invalid());
            }
            last = page.granule;
        } else if count <= 3 {
            if page.granule != 0 && page.granule != u64::MAX {
                return Err(invalid());
            }
        } else if page.granule != u64::MAX {
            return Err(invalid());
        }
        if page.eos {
            if count <= 3 || last == 0 {
                return Err(invalid());
            }
            complete = Some(last);
        }
    }
    complete.ok_or_else(invalid)
}
