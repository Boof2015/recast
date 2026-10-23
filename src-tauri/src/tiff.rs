//! Bounded TIFF container inspection, not a second pixel decoder.
//! Classic TIFF and BigTIFF have different directory widths; both can contain
//! additional pages or image subdirectories that must not be silently dropped.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
};

const INVALID: &str = "This TIFF is incomplete or has an invalid image directory.";
const MULTIPAGE: &str =
    "Multipage TIFF conversion is not supported yet. Remove this file to convert the rest.";

pub fn has_signature(bytes: &[u8]) -> bool {
    bytes.starts_with(b"II\x2a\0")
        || bytes.starts_with(b"MM\0\x2a")
        || bytes.starts_with(b"II\x2b\0")
        || bytes.starts_with(b"MM\0\x2b")
}

struct Reader<'a> {
    file: &'a mut File,
    length: u64,
    little: bool,
}

impl Reader<'_> {
    fn number(&self, bytes: &[u8]) -> u64 {
        if self.little {
            bytes.iter().rev().fold(0, |n, b| (n << 8) | u64::from(*b))
        } else {
            bytes.iter().fold(0, |n, b| (n << 8) | u64::from(*b))
        }
    }

    fn range(&self, offset: u64, size: u64) -> Result<(), String> {
        if offset.checked_add(size).is_none_or(|end| end > self.length) {
            return Err(INVALID.into());
        }
        Ok(())
    }

    fn read(&mut self, offset: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.range(offset, bytes.len() as u64)?;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| INVALID)?;
        self.file.read_exact(bytes).map_err(|_| INVALID.into())
    }
}

pub fn inspect(file: &mut File) -> Result<(), String> {
    let mut reader = Reader {
        length: file.metadata().map_err(|_| INVALID)?.len(),
        file,
        little: false,
    };
    let mut header = [0; 16];
    reader.read(0, &mut header[..8])?;
    if !has_signature(&header) {
        return Err(INVALID.into());
    }
    reader.little = &header[..2] == b"II";
    let big = reader.number(&header[2..4]) == 43;
    let (header_size, count_size, entry_size, pointer_size, value_start) = if big {
        if reader.number(&header[4..6]) != 8 || reader.number(&header[6..8]) != 0 {
            return Err(INVALID.into());
        }
        reader.read(8, &mut header[8..])?;
        (16, 8, 20, 8, 12)
    } else {
        // Canon CR2 shares TIFF's initial signature but is a raw container.
        reader.read(8, &mut header[8..12])?;
        if &header[8..12] == b"CR\x02\0" {
            return Err("Raw camera images are not supported yet.".into());
        }
        (8, 2, 12, 4, 8)
    };
    let ifd = reader.number(&header[header_size - pointer_size..header_size]);
    if ifd < header_size as u64 {
        return Err(INVALID.into());
    }
    let mut count = [0; 8];
    reader.read(ifd, &mut count[..count_size])?;
    let count = reader.number(&count[..count_size]);
    // Standard image directories contain tens of tags. Bound work independently
    // of a BigTIFF's untrusted 64-bit count and never follow a directory chain.
    if count == 0 || count > 4096 {
        return Err(INVALID.into());
    }
    let directory_size = count_size as u64 + count * entry_size as u64;
    reader.range(ifd, directory_size + pointer_size as u64)?;
    let mut next = [0; 8];
    reader.read(ifd + directory_size, &mut next[..pointer_size])?;
    if reader.number(&next[..pointer_size]) != 0 {
        return Err(MULTIPAGE.into());
    }

    let mut tags = BTreeMap::<u16, Vec<u64>>::new();
    let mut seen = BTreeSet::new();
    for index in 0..count {
        let entry_offset = ifd + count_size as u64 + index * entry_size as u64;
        let mut entry = [0; 20];
        reader.read(entry_offset, &mut entry[..entry_size])?;
        let tag = reader.number(&entry[..2]) as u16;
        let datatype = reader.number(&entry[2..4]);
        let values = reader.number(&entry[4..value_start]);
        if !seen.insert(tag) {
            return Err(INVALID.into());
        }
        if values > 0 && [330, 37724].contains(&tag) {
            return Err("TIFF files with image subdirectories or layers are not supported yet. Remove this file to convert the rest.".into());
        }
        if tag == 50706 {
            return Err(
                "Raw DNG images are not supported yet. Remove this file to convert the rest."
                    .into(),
            );
        }
        let width = match datatype {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 | 16 | 17 | 18 => 8,
            _ => return Err(INVALID.into()),
        };
        let size = values.checked_mul(width).ok_or(INVALID)?;
        let offset = if size <= pointer_size as u64 {
            entry_offset + value_start as u64
        } else {
            reader.number(&entry[value_start..entry_size])
        };
        reader.range(offset, size)?;
        if ![
            256, 257, 258, 259, 262, 273, 277, 278, 279, 284, 322, 323, 324, 325, 338, 339,
        ]
        .contains(&tag)
        {
            continue;
        }
        let maximum = if [273, 279, 324, 325].contains(&tag) {
            1_048_576
        } else {
            16
        };
        if ![3, 4, 16].contains(&datatype) || values == 0 || values > maximum {
            return Err(INVALID.into());
        }
        let mut data = vec![0; size as usize];
        reader.read(offset, &mut data)?;
        tags.insert(
            tag,
            data.chunks_exact(width as usize)
                .map(|bytes| reader.number(bytes))
                .collect(),
        );
    }
    let scalar = |tag, default| -> Result<u64, String> {
        match tags.get(&tag).map(Vec::as_slice) {
            Some([value]) => Ok(*value),
            None => Ok(default),
            _ => Err(INVALID.into()),
        }
    };
    if scalar(256, 0)? == 0 || scalar(257, 0)? == 0 {
        return Err(INVALID.into());
    }
    let samples = scalar(277, 1)?;
    if samples == 0 || samples > 5 || ![1, 2].contains(&scalar(284, 1)?) {
        return Err("This TIFF channel layout is not supported yet.".into());
    }
    let color_channels = match scalar(262, u64::MAX)? {
        0 | 1 | 3 => 1,
        2 | 6 | 8 => 3,
        5 => 4,
        _ => return Err("This TIFF color model is not supported yet.".into()),
    };
    let has_alpha = matches!(tags.get(&338).map(Vec::as_slice), Some([1 | 2]));
    if samples != color_channels + u64::from(has_alpha)
        || tags
            .get(&338)
            .is_some_and(|extra| !extra.is_empty() && !has_alpha)
    {
        return Err("This TIFF channel layout is not supported yet.".into());
    }
    if tags.get(&258).is_some_and(|bits| {
        ![1, samples as usize].contains(&bits.len())
            || bits.iter().any(|value| ![1, 2, 4, 8, 16].contains(value))
    }) || tags
        .get(&339)
        .is_some_and(|formats| formats.iter().any(|value| *value != 1))
    {
        return Err("This build supports TIFF with unsigned channels up to 16 bits. Floating-point and other channel types are not supported yet.".into());
    }
    if ![1, 2, 3, 4, 5, 7, 8, 32773, 32946].contains(&scalar(259, 1)?) {
        return Err(
            "This TIFF compression is not supported yet. Remove this file to convert the rest."
                .into(),
        );
    }
    let (offsets, sizes) = match (
        tags.get(&273),
        tags.get(&279),
        tags.get(&324),
        tags.get(&325),
    ) {
        (Some(offsets), Some(sizes), None, None) => (offsets, sizes),
        (None, None, Some(offsets), Some(sizes)) => (offsets, sizes),
        _ => return Err(INVALID.into()),
    };
    if offsets.len() != sizes.len() {
        return Err(INVALID.into());
    }
    for (&offset, &size) in offsets.iter().zip(sizes) {
        if offset < header_size as u64 || size == 0 {
            return Err(INVALID.into());
        }
        reader.range(offset, size)?;
    }
    Ok(())
}
