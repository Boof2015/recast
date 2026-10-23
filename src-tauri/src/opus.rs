//! Validate Ogg framing before a recovering demuxer can conceal lost audio.
//! RFC 7845: one logical Opus stream, mapping family 0/1, complete pages,
//! pre-skip and final granule trimming. Chained/multiplexed streams are explicit
//! exclusions rather than an implicit selection of the first stream.
use crate::image_backend::ConversionError;
use std::{path::Path, sync::atomic::AtomicBool};

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
    let mut reader = crate::ogg::Reader::open(path)?;
    let mut packets = 0u64;
    let mut total = 0u64;
    let mut preskip = 0u64;
    let mut offset = None;
    let mut last_granule = 0u64;
    while let Some(page) = reader.next(cancel)? {
        let granule = page.granule;
        let eos = page.eos;
        let before = packets;
        for (index, packet) in page.packets.iter().enumerate() {
            match packets {
                0 => {
                    if packet.len() < 19
                        || !packet.starts_with(b"OpusHead")
                        || packet[8] != 1
                        || !(1..=8).contains(&packet[9])
                        || (index + 1 != page.packets.len() || page.continued)
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
                        || (index + 1 != page.packets.len() || page.continued)
                    {
                        return Err(invalid());
                    }
                }
                _ => {
                    total = total
                        .checked_add(duration(packet).ok_or_else(invalid)?)
                        .ok_or_else(invalid)?
                }
            }
            packets += 1;
        }
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
        if eos && (packets <= 2 || packets == before) {
            return Err(invalid());
        }
    }
    last_granule
        .checked_sub(offset.ok_or_else(invalid)?)
        .and_then(|v| v.checked_sub(preskip))
        .filter(|v| *v > 0)
        .ok_or_else(invalid)
}
