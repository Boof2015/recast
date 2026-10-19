//! Bounded Ogg page/packet reader shared by the audio validators.
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

pub struct Page {
    pub granule: u64,
    pub eos: bool,
    pub continued: bool,
    pub packets: Vec<Vec<u8>>,
}

pub struct Reader {
    file: File,
    remaining: u64,
    serial: Option<u32>,
    sequence: u32,
    packet: Vec<u8>,
    continued: bool,
    eos: bool,
}

pub fn invalid() -> ConversionError {
    "This Ogg audio file has incomplete or invalid pages, packets, or timing."
        .to_string()
        .into()
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self, ConversionError> {
        let file = File::open(path).map_err(|_| invalid())?;
        let remaining = file.metadata().map_err(|_| invalid())?.len();
        Ok(Self {
            file,
            remaining,
            serial: None,
            sequence: 0,
            packet: Vec::new(),
            continued: false,
            eos: false,
        })
    }

    pub fn next(&mut self, cancel: &AtomicBool) -> Result<Option<Page>, ConversionError> {
        check_cancel(cancel)?;
        if self.remaining == 0 {
            return if self.eos { Ok(None) } else { Err(invalid()) };
        }
        let unsupported = || {
            ConversionError::from("Chained or multiplexed Ogg streams are not supported yet. Choose a file containing one complete audio stream.".to_string())
        };
        if self.eos {
            return Err(unsupported());
        }
        let mut header = [0u8; 27];
        self.file.read_exact(&mut header).map_err(|_| invalid())?;
        if &header[..4] != b"OggS" || header[4] != 0 || header[5] & !7 != 0 {
            return Err(invalid());
        }
        let flags = header[5];
        let serial = u32::from_le_bytes(header[14..18].try_into().unwrap());
        if self.serial.is_some_and(|s| s != serial) || (self.serial.is_some() && flags & 2 != 0) {
            return Err(unsupported());
        }
        if u32::from_le_bytes(header[18..22].try_into().unwrap()) != self.sequence
            || (flags & 1 != 0) != self.continued
            || (self.serial.is_none() && flags != 2)
        {
            return Err(invalid());
        }
        self.serial = Some(serial);
        self.sequence = self.sequence.wrapping_add(1);
        let granule = u64::from_le_bytes(header[6..14].try_into().unwrap());
        let checksum = u32::from_le_bytes(header[22..26].try_into().unwrap());
        let mut lacing = vec![0; header[26] as usize];
        self.file.read_exact(&mut lacing).map_err(|_| invalid())?;
        let size: usize = lacing.iter().map(|v| *v as usize).sum();
        let mut data = vec![0; size];
        self.file.read_exact(&mut data).map_err(|_| invalid())?;
        self.remaining = self
            .remaining
            .checked_sub((27 + lacing.len() + size) as u64)
            .ok_or_else(invalid)?;
        header[22..26].fill(0);
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(&lacing);
        bytes.extend_from_slice(&data);
        if crc(&bytes) != checksum {
            return Err(invalid());
        }
        let mut packets = Vec::new();
        let mut position = 0;
        for length in lacing {
            let end = position + length as usize;
            self.packet.extend_from_slice(&data[position..end]);
            position = end;
            if self.packet.len() > 8 * 1024 * 1024 {
                return Err(invalid());
            }
            self.continued = length == 255;
            if !self.continued {
                packets.push(std::mem::take(&mut self.packet));
            }
        }
        self.eos = flags & 4 != 0;
        if self.eos && (self.continued || packets.is_empty()) {
            return Err(invalid());
        }
        Ok(Some(Page {
            granule,
            eos: self.eos,
            continued: self.continued,
            packets,
        }))
    }
}
