//! Bounded Word 97–2003 inspection. Layout/rendering stays with LibreOffice.
//! FIB, CLX and formatting pages follow Microsoft's MS-DOC specification.
use crate::{documents::check_fields, docx::Document, error, Result};
use std::io::{Cursor, Read};

const DAMAGED: &str = "The DOC contains damaged document tables.";
const CHANGES: &str = "Accept or reject tracked changes in the document before converting it.";
fn part(bytes: &[u8], offset: usize, count: usize) -> Result<&[u8]> {
    bytes
        .get(offset..offset.checked_add(count).ok_or(DAMAGED)?)
        .ok_or(DAMAGED.into())
}
fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        part(bytes, offset, 2)?.try_into().unwrap(),
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        part(bytes, offset, 4)?.try_into().unwrap(),
    ))
}
fn properties(mut bytes: &[u8]) -> Result<()> {
    while bytes.len() >= 2 {
        let code = u16_at(bytes, 0)?;
        if code == 0 {
            break;
        } // optional padding
        bytes = &bytes[2..];
        let size = match code >> 13 {
            0 | 1 => 1,
            2 | 4 | 5 => 2,
            3 => 4,
            7 => 3,
            _ if matches!(code, 0xd606 | 0xd608) => {
                let size = u16_at(bytes, 0)? as usize;
                if size == 0 {
                    return Err(DAMAGED.into());
                }
                size + 1
            }
            _ => 1 + *bytes.first().ok_or(DAMAGED)? as usize,
        };
        let value = part(bytes, 0, size)?;
        if matches!(code, 0x0800 | 0x0801) && value[0] != 0
            || matches!(code, 0xca57 | 0xca89 | 0xc63e | 0xd243 | 0xd667)
                && value.get(1).is_some_and(|v| *v != 0)
        {
            return Err(CHANGES.into());
        }
        if code == 0x080a && value[0] != 0 {
            return Err("Embedded Office objects are not supported yet.".into());
        }
        bytes = &bytes[size..];
    }
    Ok(())
}
fn formatting(word: &[u8], plc: &[u8], paragraph: bool) -> Result<()> {
    if plc.is_empty() {
        return Ok(());
    }
    if plc.len() < 4 || !(plc.len() - 4).is_multiple_of(8) {
        return Err(DAMAGED.into());
    }
    let count = (plc.len() - 4) / 8;
    for i in 0..count {
        let page = u32_at(plc, (count + 1) * 4 + i * 4)? as usize;
        let page = part(word, page.checked_mul(512).ok_or(DAMAGED)?, 512)?;
        let runs = page[511] as usize;
        let stride = if paragraph { 13 } else { 1 };
        if (runs + 1) * 4 + runs * stride > 511 {
            return Err(DAMAGED.into());
        }
        for run in 0..runs {
            let offset = page[(runs + 1) * 4 + run * stride] as usize * 2;
            if offset == 0 {
                continue;
            }
            let size = page[offset] as usize;
            if paragraph {
                let (offset, size) = if size == 0 {
                    (
                        offset + 2,
                        *page.get(offset + 1).ok_or(DAMAGED)? as usize * 2,
                    )
                } else {
                    (offset + 1, size * 2 - 1)
                };
                let value = part(page, offset, size)?;
                properties(value.get(2..).ok_or(DAMAGED)?)?; // paragraph style precedes properties
            } else {
                properties(part(page, offset + 1, size)?)?;
            }
        }
    }
    Ok(())
}

fn text(word: &[u8], mut clx: &[u8]) -> Result<()> {
    while clx.first() == Some(&1) {
        let size = u16_at(clx, 1)? as usize;
        properties(part(clx, 3, size)?)?;
        clx = clx.get(3 + size..).ok_or(DAMAGED)?;
    }
    if clx.first() != Some(&2) {
        return Err(DAMAGED.into());
    }
    let plc = part(clx, 5, u32_at(clx, 1)? as usize)?;
    if plc.len() < 4 || !(plc.len() - 4).is_multiple_of(12) {
        return Err(DAMAGED.into());
    }
    let pieces = (plc.len() - 4) / 12;
    let mut fields = String::new();
    let mut instructions = Vec::new();
    for i in 0..pieces {
        let chars = u32_at(plc, (i + 1) * 4)?
            .checked_sub(u32_at(plc, i * 4)?)
            .ok_or(DAMAGED)? as usize;
        let descriptor = (pieces + 1) * 4 + i * 8;
        let fc = u32_at(plc, descriptor + 2)?;
        let compressed = fc & 0x40000000 != 0;
        let offset = (fc & 0x3fffffff) as usize / if compressed { 2 } else { 1 };
        let bytes = part(
            word,
            offset,
            chars
                .checked_mul(if compressed { 1 } else { 2 })
                .ok_or(DAMAGED)?,
        )?;
        let prm = u16_at(plc, descriptor + 6)?;
        if prm & 1 == 0 && [0x41, 0x42].contains(&((prm >> 1) & 0x7f)) && prm >> 8 != 0 {
            return Err(CHANGES.into());
        }
        let mut character = |value: u16| -> Result<()> {
            match value {
                0x13 => {
                    instructions.push(true);
                    fields.push(' ');
                }
                0x14 => {
                    if let Some(state) = instructions.last_mut() {
                        *state = false;
                    }
                }
                0x15 => {
                    instructions.pop();
                    fields.push(' ');
                }
                _ if instructions.last() == Some(&true) => {
                    fields.push(char::from_u32(value as u32).unwrap_or('\u{fffd}'))
                }
                _ => {}
            }
            if instructions.len() > 256 || fields.len() > 32 * 1024 * 1024 {
                return Err("The DOC contains excessive field instructions.".into());
            }
            Ok(())
        };
        if compressed {
            for &byte in bytes {
                character(byte as u16)?;
            }
        } else {
            for value in bytes.as_chunks::<2>().0 {
                character(u16::from_le_bytes(*value))?;
            }
        }
    }
    check_fields(&fields)
}

pub fn inspect(bytes: &[u8]) -> Result<Document> {
    let mut compound = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|_| "This is not a readable Word DOC file. Encrypted DOCX is not supported.")?;
    let entries: Vec<_> = compound.walk().take(10_001).collect();
    if entries.len() > 10_000 {
        return Err("The DOC contains too many parts.".into());
    }
    let mut total = 0u64;
    for entry in &entries {
        let name = entry.name().to_ascii_lowercase();
        if ["encryptedpackage", "encryptioninfo"].contains(&name.as_str()) {
            return Err("Encrypted documents are not supported. Save an unencrypted copy.".into());
        }
        if ["macros", "vba", "objectpool"].contains(&name.as_str())
            || name.starts_with("_vba_project")
        {
            return Err("Macros and embedded Office objects are not supported yet.".into());
        }
        total = total.checked_add(entry.len()).ok_or(DAMAGED)?;
        if total > 512 * 1024 * 1024 || entry.len() > 256 * 1024 * 1024 {
            return Err("The DOC contains oversized streams.".into());
        }
        if entry.is_stream() {
            let count = std::io::copy(
                &mut compound
                    .open_stream(entry.path())
                    .map_err(error)?
                    .take(entry.len() + 1),
                &mut std::io::sink(),
            )
            .map_err(|_| DAMAGED)?;
            if count != entry.len() {
                return Err(DAMAGED.into());
            }
        }
    }
    let mut stream = |name: &str| -> Result<Vec<u8>> {
        let mut value = Vec::new();
        compound
            .open_stream(name)
            .map_err(|_| "The DOC is missing a required Word stream.")?
            .take(256 * 1024 * 1024 + 1)
            .read_to_end(&mut value)
            .map_err(error)?;
        if value.len() > 256 * 1024 * 1024 {
            return Err("The DOC contains an oversized stream.".into());
        }
        Ok(value)
    };
    let word = stream("/WordDocument")?;
    if u16_at(&word, 0)? != 0xa5ec {
        return Err("This is not a Word DOC document.".into());
    }
    if u16_at(&word, 2)? < 0x00c1 {
        return Err(
            "This older Word format is not supported. Save a DOCX or Word 97–2003 DOC copy.".into(),
        );
    }
    let flags = u16_at(&word, 10)?;
    if flags & 0x0100 != 0 {
        return Err("Encrypted documents are not supported. Save an unencrypted copy.".into());
    }
    let table = stream(if flags & 0x0200 != 0 {
        "/1Table"
    } else {
        "/0Table"
    })?;
    let long_start = 34 + u16_at(&word, 32)? as usize * 2;
    let arrays = long_start + 2 + u16_at(&word, long_start)? as usize * 4;
    let count = u16_at(&word, arrays)? as usize;
    if count < 93 {
        return Err(DAMAGED.into());
    }
    part(&word, arrays + 2, count * 8)?;
    let data = |index: usize| -> Result<&[u8]> {
        let pair = arrays + 2 + index * 8;
        let size = u32_at(&word, pair + 4)? as usize;
        if size == 0 {
            return Ok(&[]);
        }
        part(&table, u32_at(&word, pair)? as usize, size)
    };
    text(&word, data(33)?)?;
    formatting(&word, data(12)?, false)?;
    formatting(&word, data(13)?, true)?;
    // Section property records carry their own property-revision marks.
    let sections = data(6)?;
    if !sections.is_empty() {
        if sections.len() < 4 || !(sections.len() - 4).is_multiple_of(16) {
            return Err(DAMAGED.into());
        }
        let count = (sections.len() - 4) / 16;
        for index in 0..count {
            let offset = u32_at(sections, (count + 1) * 4 + index * 12 + 2)?;
            if offset != u32::MAX {
                let offset = offset as usize;
                properties(part(&word, offset + 2, u16_at(&word, offset)? as usize)?)?;
            }
        }
    }
    Ok(Document {
        comments: !data(4)?.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_revisions_but_accepts_ordinary_formatting() {
        assert!(properties(&[0x35, 0x08, 1]).is_ok()); // bold
        assert!(properties(&[0x01, 0x08, 0]).is_ok());
        assert_eq!(properties(&[0x01, 0x08, 1]).unwrap_err(), CHANGES);
        assert_eq!(
            properties(&[0x89, 0xca, 7, 1, 0, 0, 0, 0, 0, 0]).unwrap_err(),
            CHANGES
        );
        assert!(properties(&[0x0a, 0x08, 1])
            .unwrap_err()
            .contains("objects"));
        assert!(properties(&[0x89, 0xca, 40]).is_err());
    }
    #[test]
    fn rejects_incomplete_compound_files() {
        assert!(inspect(b"\xd0\xcf\x11\xe0").is_err());
        assert!(part(&[], usize::MAX, 2).is_err());
    }

    #[test]
    fn reads_legacy_word_and_rejects_encryption_macros_and_revisions() {
        use std::io::{Seek, SeekFrom, Write};
        let bytes = include_bytes!("../../testdata/layout.doc");
        assert!(inspect(bytes).is_ok());
        assert!(inspect(include_bytes!("../../testdata/tracked.doc"))
            .err()
            .unwrap()
            .contains("tracked changes"));
        assert!(inspect(include_bytes!("../../testdata/linked.doc"))
            .err()
            .unwrap()
            .contains("external fields"));
        let mut encrypted = cfb::CompoundFile::open(Cursor::new(bytes.to_vec())).unwrap();
        {
            let mut stream = encrypted.open_stream("/WordDocument").unwrap();
            stream.seek(SeekFrom::Start(10)).unwrap();
            let mut flags = [0; 2];
            stream.read_exact(&mut flags).unwrap();
            stream.seek(SeekFrom::Start(10)).unwrap();
            stream
                .write_all(&(u16::from_le_bytes(flags) | 0x0100).to_le_bytes())
                .unwrap();
        }
        assert!(inspect(encrypted.into_inner().get_ref())
            .err()
            .unwrap()
            .contains("Encrypted"));
        let mut macros = cfb::CompoundFile::open(Cursor::new(bytes.to_vec())).unwrap();
        macros.create_storage("/Macros").unwrap();
        assert!(inspect(macros.into_inner().get_ref())
            .err()
            .unwrap()
            .contains("Macros"));
    }
}
