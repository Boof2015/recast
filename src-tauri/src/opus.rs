//! Validate Ogg framing before a recovering demuxer can conceal lost audio.
//! RFC 7845: one logical Opus stream, mapping family 0/1, complete pages,
//! pre-skip and final granule trimming. Chained/multiplexed streams are explicit
//! exclusions rather than an implicit selection of the first stream.
use crate::image_backend::{check_cancel, ConversionError};
use std::{fs::File, io::Read, path::Path, sync::atomic::AtomicBool};

const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = (index as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            value = (value << 1)
                ^ if value & 0x8000_0000 != 0 {
                    0x04c1_1db7
                } else {
                    0
                };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

pub(crate) fn crc(bytes: &[u8]) -> u32 {
    let mut value = 0u32;
    for &byte in bytes {
        value = (value << 8) ^ CRC_TABLE[((value >> 24) as u8 ^ byte) as usize];
    }
    value
}

fn duration(packet: &[u8]) -> Option<u64> {
    let &toc = packet.first()?;
    let frame = if toc & 128 != 0 {
        120 << ((toc >> 3) & 3)
    } else if toc & 96 == 96 {
        if toc & 8 != 0 {
            960
        } else {
            480
        }
    } else {
        match (toc >> 3) & 3 {
            3 => 2880,
            n => 480 << n,
        }
    };
    let count = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => u64::from(*packet.get(1)? & 63),
    };
    let samples = frame * count;
    (samples > 0 && samples <= 5760).then_some(samples)
}

pub fn validate(path: &Path, cancel: &AtomicBool) -> Result<u64, ConversionError> {
    let invalid = || {
        ConversionError::from(
            "This Opus file has incomplete or invalid Ogg pages, packets, or timing.".to_string(),
        )
    };
    let unsupported = || {
        ConversionError::from("Chained or multiplexed Ogg streams are not supported yet. Choose a file containing one complete Opus stream.".to_string())
    };
    let mut file = File::open(path).map_err(|_| invalid())?;
    let mut remaining = file.metadata().map_err(|_| invalid())?.len();
    let mut serial = None;
    let mut sequence = 0u32;
    let mut packet = Vec::new();
    let mut packets = 0u64;
    let mut total = 0u64;
    let mut preskip = 0u64;
    let mut offset = None;
    let mut last_granule = 0u64;
    let mut eos = false;
    let mut continued = false;
    while remaining > 0 {
        check_cancel(cancel)?;
        if eos {
            return Err(unsupported());
        }
        let mut header = [0u8; 27];
        file.read_exact(&mut header).map_err(|_| invalid())?;
        if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
            return Err(invalid());
        }
        let flags = header[5];
        let page_serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if serial.is_some_and(|s| s != page_serial) || (serial.is_some() && flags & 2 != 0) {
            return Err(unsupported());
        }
        if u32::from_le_bytes(header[18..22].try_into().unwrap()) != sequence
            || (flags & 1 != 0) != continued
            || (serial.is_none() && flags != 2)
        {
            return Err(invalid());
        }
        serial = Some(page_serial);
        sequence = sequence.wrapping_add(1);
        let granule = u64::from_le_bytes(header[6..14].try_into().unwrap());
        let checksum = u32::from_le_bytes(header[22..26].try_into().unwrap());
        let mut lacing = vec![0; usize::from(header[26])];
        file.read_exact(&mut lacing).map_err(|_| invalid())?;
        let size: usize = lacing.iter().map(|v| usize::from(*v)).sum();
        let mut data = vec![0; size];
        file.read_exact(&mut data).map_err(|_| invalid())?;
        remaining = remaining
            .checked_sub((27 + lacing.len() + size) as u64)
            .ok_or_else(invalid)?;
        header[22..26].fill(0);
        let mut page = header.to_vec();
        page.extend_from_slice(&lacing);
        page.extend_from_slice(&data);
        if crc(&page) != checksum {
            return Err(invalid());
        }
        let before = packets;
        let mut position = 0;
        for (segment, &length) in lacing.iter().enumerate() {
            let end = position + usize::from(length);
            packet.extend_from_slice(&data[position..end]);
            position = end;
            // Bound large comment headers and damaged packet continuations.
            if packet.len() > 8 * 1024 * 1024 {
                return Err(invalid());
            }
            continued = length == 255;
            if continued {
                continue;
            }
            match packets {
                0 => {
                    if packet.len() < 19
                        || !packet.starts_with(b"OpusHead")
                        || packet[8] != 1
                        || !(1..=8).contains(&packet[9])
                        || segment + 1 != lacing.len()
                    {
                        return Err(invalid());
                    }
                    let channels = usize::from(packet[9]);
                    match packet[18] {
                        0 if channels <= 2 && packet.len() == 19 => (),
                        1 if packet.len() == 21 + channels && packet[19] > 0
                            && packet[20] <= packet[19] && u16::from(packet[19]) + u16::from(packet[20]) <= 255
                            && packet[21..].iter().all(|&m| m == 255 || u16::from(m) < u16::from(packet[19]) + u16::from(packet[20])) => (),
                        _ => return Err("This Opus channel mapping is not supported. Recast accepts standard mono, stereo, and surround mapping families 0 and 1.".to_string().into()),
                    }
                    preskip = u64::from(u16::from_le_bytes(packet[10..12].try_into().unwrap()));
                }
                1 => {
                    if !packet.starts_with(b"OpusTags")
                        || packet.len() < 16
                        || segment + 1 != lacing.len()
                    {
                        return Err(invalid());
                    }
                }
                _ => {
                    total = total
                        .checked_add(duration(&packet).ok_or_else(invalid)?)
                        .ok_or_else(invalid)?
                }
            }
            packets += 1;
            packet.clear();
        }
        eos = flags & 4 != 0;
        if packets > before && packets > 2 {
            let initial = *offset.get_or_insert(if eos {
                0
            } else {
                granule.checked_sub(total).ok_or_else(invalid)?
            });
            let expected = initial.checked_add(total).ok_or_else(invalid)?;
            if granule == u64::MAX
                || granule < last_granule
                || granule > expected
                || (!eos && granule != expected)
            {
                return Err(invalid());
            }
            last_granule = granule;
        } else if packets <= 2 {
            if granule != 0 && granule != u64::MAX {
                return Err(invalid());
            }
        } else if granule != u64::MAX {
            return Err(invalid());
        }
        if eos && (continued || packets <= 2 || packets == before) {
            return Err(invalid());
        }
    }
    if !eos {
        return Err(invalid());
    }
    last_granule
        .checked_sub(offset.ok_or_else(invalid)?)
        .and_then(|v| v.checked_sub(preskip))
        .filter(|v| *v > 0)
        .ok_or_else(invalid)
}
