//! Bounded AVIF container inspection. Decoding remains the bundled codec's job.
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

const INVALID: &str = "This AVIF is incomplete, unreadable, or uses an unsupported layout.";
const SEQUENCE: &str =
    "Animated or multi-image AVIF is not supported yet. Remove this file to convert the rest.";
const HDR: &str =
    "HDR and gain-map AVIF are not supported yet. Remove this file to convert the rest.";
const MAX_META: u64 = 16 * 1024 * 1024;
const MAX_BOXES: usize = 4096;

type Result<T> = std::result::Result<T, String>;

pub fn has_signature(header: &[u8]) -> bool {
    if header.len() < 16 || &header[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
    let start = if size == 1 { 16 } else { 8 };
    let end = if size == 1 {
        header.len()
    } else {
        size.min(header.len())
    };
    end >= start + 8
        && header[start..end]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .any(|(index, brand)| index != 1 && matches!(brand, b"avif" | b"avis"))
}

#[derive(Clone)]
struct BoxRange {
    kind: [u8; 4],
    data: Range<usize>,
}

fn boxes(data: &[u8], start: usize) -> Result<Vec<BoxRange>> {
    let mut cursor = start;
    let mut result = Vec::new();
    while cursor < data.len() {
        if result.len() == MAX_BOXES || data.len() - cursor < 8 {
            return Err(INVALID.into());
        }
        let size = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap());
        let kind = data[cursor + 4..cursor + 8].try_into().unwrap();
        let (size, header) = match size {
            0 => ((data.len() - cursor) as u64, 8),
            1 if data.len() - cursor >= 16 => (
                u64::from_be_bytes(data[cursor + 8..cursor + 16].try_into().unwrap()),
                16,
            ),
            1 => return Err(INVALID.into()),
            value => (u64::from(value), 8),
        };
        if size < header as u64 || size > (data.len() - cursor) as u64 {
            return Err(INVALID.into());
        }
        let end = cursor + size as usize;
        result.push(BoxRange {
            kind,
            data: cursor + header..end,
        });
        cursor = end;
    }
    Ok(result)
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}
impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(count).ok_or(INVALID)?;
        let result = self.data.get(self.position..end).ok_or(INVALID)?;
        self.position = end;
        Ok(result)
    }
    fn number(&mut self, count: usize) -> Result<u64> {
        if count > 8 {
            return Err(INVALID.into());
        }
        Ok(self
            .take(count)?
            .iter()
            .fold(0, |n, byte| (n << 8) | u64::from(*byte)))
    }
    fn full(&mut self) -> Result<(u64, u64)> {
        Ok((self.number(1)?, self.number(3)?))
    }
    fn done(&self) -> Result<()> {
        if self.position == self.data.len() {
            Ok(())
        } else {
            Err(INVALID.into())
        }
    }
}

pub fn inspect(file: &mut File) -> Result<()> {
    let io = |_| INVALID.to_string();
    file.rewind().map_err(io)?;
    let length = file.metadata().map_err(io)?.len();
    let mut cursor = 0;
    let mut count = 0;
    let mut ftyp = false;
    let mut meta = None;
    let mut media = Vec::new();
    while cursor < length {
        count += 1;
        if count > MAX_BOXES || length - cursor < 8 {
            return Err(INVALID.into());
        }
        file.seek(SeekFrom::Start(cursor)).map_err(io)?;
        let mut header = [0; 8];
        file.read_exact(&mut header).map_err(io)?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let (size, header_size) = match size {
            0 => (length - cursor, 8),
            1 => {
                let mut extended = [0; 8];
                file.read_exact(&mut extended).map_err(io)?;
                (u64::from_be_bytes(extended), 16)
            }
            size => (u64::from(size), 8),
        };
        if size < header_size || size > length - cursor {
            return Err(INVALID.into());
        }
        let data_start = cursor + header_size;
        let end = cursor + size;
        match &header[4..] {
            b"ftyp" => {
                if ftyp || cursor != 0 || size - header_size > 4096 {
                    return Err(INVALID.into());
                }
                let mut data = vec![0; (size - header_size) as usize];
                file.read_exact(&mut data).map_err(io)?;
                if data.len() < 8 || !data.len().is_multiple_of(4) {
                    return Err(INVALID.into());
                }
                for (index, brand) in data.as_chunks::<4>().0.iter().enumerate() {
                    if index == 1 {
                        continue;
                    }
                    if matches!(brand, b"avis" | b"msf1") {
                        return Err(SEQUENCE.into());
                    }
                    ftyp |= brand == b"avif";
                }
                if !ftyp {
                    return Err(INVALID.into());
                }
            }
            b"moov" | b"moof" => return Err(SEQUENCE.into()),
            b"meta" => {
                if meta.is_some() || size - header_size > MAX_META {
                    return Err(INVALID.into());
                }
                let mut data = vec![0; (size - header_size) as usize];
                file.read_exact(&mut data).map_err(io)?;
                meta = Some(data);
            }
            b"mdat" if end > data_start => media.push(data_start..end),
            // Unknown top-level boxes are bounded and skipped, never decoded.
            _ => (),
        }
        cursor = end;
    }
    if !ftyp {
        return Err(INVALID.into());
    }
    inspect_meta(&meta.ok_or(INVALID)?, &media)
}

fn unique<'a>(data: &'a [u8], children: &[BoxRange], kind: &[u8; 4]) -> Result<&'a [u8]> {
    let mut matches = children.iter().filter(|entry| &entry.kind == kind);
    let entry = matches.next().ok_or(INVALID)?;
    if matches.next().is_some() {
        return Err(INVALID.into());
    }
    Ok(&data[entry.data.clone()])
}

fn inspect_meta(data: &[u8], media: &[Range<u64>]) -> Result<()> {
    if data.get(..4) != Some(&[0, 0, 0, 0]) {
        return Err(INVALID.into());
    }
    let children = boxes(data, 4)?;
    let mut primary = Cursor::new(unique(data, &children, b"pitm")?);
    let (version, flags) = primary.full()?;
    if version > 1 || flags != 0 {
        return Err(INVALID.into());
    }
    let primary_id = primary.number(if version == 0 { 2 } else { 4 })?;
    primary.done()?;

    let info = unique(data, &children, b"iinf")?;
    let mut cursor = Cursor::new(info);
    let (version, flags) = cursor.full()?;
    if version > 1 || flags != 0 {
        return Err(INVALID.into());
    }
    let count = cursor.number(if version == 0 { 2 } else { 4 })?;
    if count == 0 || count > 256 {
        return Err(INVALID.into());
    }
    let entries = boxes(info, cursor.position)?;
    if entries.len() as u64 != count {
        return Err(INVALID.into());
    }
    let mut items = HashMap::new();
    let mut visible = Vec::new();
    for entry in entries {
        if &entry.kind != b"infe" {
            return Err(INVALID.into());
        }
        let mut entry = Cursor::new(&info[entry.data]);
        let (version, flags) = entry.full()?;
        if ![2, 3].contains(&version) || flags > 1 {
            return Err(INVALID.into());
        }
        let id = entry.number(if version == 2 { 2 } else { 4 })?;
        if id == 0 || entry.number(2)? != 0 {
            return Err(INVALID.into());
        }
        let kind: [u8; 4] = entry.take(4)?.try_into().unwrap();
        if &kind == b"tmap" {
            return Err(HDR.into());
        }
        if !matches!(&kind, b"av01" | b"grid" | b"Exif" | b"mime")
            || !entry.data[entry.position..].contains(&0)
            || items.insert(id, kind).is_some()
        {
            return Err(INVALID.into());
        }
        if matches!(&kind, b"av01" | b"grid") && flags == 0 {
            visible.push(id);
        }
    }
    if !visible.contains(&primary_id) {
        return Err(INVALID.into());
    }
    // Alpha, thumbnails, and grid tiles may be non-hidden image items. Follow
    // their actual references; do not mistake alpha for a second photograph or
    // silently ignore an unrelated image just because it has the hidden flag.
    let references: Vec<_> = children
        .iter()
        .filter(|entry| &entry.kind == b"iref")
        .collect();
    if references.len() > 1 {
        return Err(INVALID.into());
    }
    let mut graph: HashMap<u64, Vec<u64>> = HashMap::new();
    if let Some(reference) = references.first() {
        let data = &data[reference.data.clone()];
        let mut cursor = Cursor::new(data);
        let (version, flags) = cursor.full()?;
        if version > 1 || flags != 0 {
            return Err(INVALID.into());
        }
        let width = if version == 0 { 2 } else { 4 };
        for entry in boxes(data, 4)? {
            let mut cursor = Cursor::new(&data[entry.data]);
            let from = cursor.number(width)?;
            let count = cursor.number(2)?;
            if count > 256 || !items.contains_key(&from) {
                return Err(INVALID.into());
            }
            for _ in 0..count {
                let to = cursor.number(width)?;
                if !items.contains_key(&to) || from == to {
                    return Err(INVALID.into());
                }
                match &entry.kind {
                    b"dimg" => graph.entry(from).or_default().push(to),
                    b"auxl" | b"thmb" => graph.entry(to).or_default().push(from),
                    b"cdsc" | b"prem" => (),
                    _ => return Err(INVALID.into()),
                }
            }
            cursor.done()?;
        }
    }
    fn visit(
        id: u64,
        graph: &HashMap<u64, Vec<u64>>,
        active: &mut HashSet<u64>,
        seen: &mut HashSet<u64>,
    ) -> Result<()> {
        if active.contains(&id) || active.len() >= 32 {
            return Err(INVALID.into());
        }
        if !seen.insert(id) {
            return Ok(());
        }
        active.insert(id);
        for child in graph.get(&id).into_iter().flatten() {
            visit(*child, graph, active, seen)?;
        }
        active.remove(&id);
        Ok(())
    }
    let mut reachable = HashSet::new();
    visit(primary_id, &graph, &mut HashSet::new(), &mut reachable)?;
    if items
        .iter()
        .any(|(id, kind)| matches!(kind, b"av01" | b"grid") && !reachable.contains(id))
    {
        return Err(SEQUENCE.into());
    }

    let properties = unique(data, &children, b"iprp")?;
    let property_children = boxes(properties, 0)?;
    let ipco = unique(properties, &property_children, b"ipco")?;
    let mut av1 = false;
    for entry in boxes(ipco, 0)? {
        let property = &ipco[entry.data];
        match &entry.kind {
            b"auxC" => {
                if property.get(..4) != Some(&[0; 4])
                    || property.get(4..) != Some(b"urn:mpeg:mpegB:cicp:systems:auxiliary:alpha\0")
                {
                    return Err(
                        "AVIF auxiliary images other than transparency are not supported yet."
                            .into(),
                    );
                }
            }
            b"av1C" => {
                if property.len() < 4 || property[0] != 0x81 {
                    return Err(INVALID.into());
                }
                av1 = true;
            }
            b"colr" if property.starts_with(b"nclx") => {
                if property.len() != 11 {
                    return Err(INVALID.into());
                }
                let transfer = u16::from_be_bytes(property[6..8].try_into().unwrap());
                if [16, 18].contains(&transfer) {
                    return Err(HDR.into());
                }
            }
            b"pixi" => {
                if property.len() < 6
                    || property[..4] != [0; 4]
                    || usize::from(property[4]) != property.len() - 5
                    || !property[5..]
                        .iter()
                        .all(|depth| [8, 10, 12].contains(depth))
                {
                    return Err(INVALID.into());
                }
            }
            b"ispe" => {
                if property.len() != 12 || property[..4] != [0; 4] {
                    return Err(INVALID.into());
                }
                let w = u32::from_be_bytes(property[4..8].try_into().unwrap());
                let h = u32::from_be_bytes(property[8..12].try_into().unwrap());
                if w == 0 || h == 0 || w > 32768 || h > 32768 {
                    return Err(INVALID.into());
                }
            }
            _ => (),
        }
    }
    if !av1 {
        return Err(INVALID.into());
    }
    let idats: Vec<_> = children
        .iter()
        .filter(|entry| &entry.kind == b"idat")
        .collect();
    if idats.len() > 1 {
        return Err(INVALID.into());
    }
    let idat_len = idats.first().map(|entry| entry.data.len() as u64);
    inspect_locations(unique(data, &children, b"iloc")?, &items, media, idat_len)
}

fn inspect_locations(
    data: &[u8],
    items: &HashMap<u64, [u8; 4]>,
    media: &[Range<u64>],
    idat: Option<u64>,
) -> Result<()> {
    let mut cursor = Cursor::new(data);
    let (version, flags) = cursor.full()?;
    if version > 2 || flags != 0 {
        return Err(INVALID.into());
    }
    let sizes = cursor.number(2)?;
    let offset_size = ((sizes >> 12) & 15) as usize;
    let length_size = ((sizes >> 8) & 15) as usize;
    let base_size = ((sizes >> 4) & 15) as usize;
    let index_size = (sizes & 15) as usize;
    if ![offset_size, length_size, base_size, index_size]
        .iter()
        .all(|size| [0, 4, 8].contains(size))
        || (version == 0 && index_size != 0)
    {
        return Err(INVALID.into());
    }
    let count = cursor.number(if version == 2 { 4 } else { 2 })?;
    if count as usize != items.len() {
        return Err(INVALID.into());
    }
    let mut seen = HashSet::new();
    let mut extent_total = 0;
    for _ in 0..count {
        let id = cursor.number(if version == 2 { 4 } else { 2 })?;
        if !items.contains_key(&id) || !seen.insert(id) {
            return Err(INVALID.into());
        }
        let method = if version > 0 { cursor.number(2)? } else { 0 };
        if method > 1 || cursor.number(2)? != 0 {
            return Err(INVALID.into());
        }
        let base = cursor.number(base_size)?;
        let extents = cursor.number(2)?;
        extent_total += extents;
        if extents == 0 || extent_total > 4096 {
            return Err(INVALID.into());
        }
        for _ in 0..extents {
            if version > 0 && cursor.number(index_size)? != 0 {
                return Err(INVALID.into());
            }
            let offset = cursor.number(offset_size)?;
            let length = cursor.number(length_size)?;
            let start = base.checked_add(offset).ok_or(INVALID)?;
            let end = start.checked_add(length).ok_or(INVALID)?;
            let valid = if method == 0 {
                media
                    .iter()
                    .any(|range| start >= range.start && end <= range.end)
            } else {
                idat.is_some_and(|size| end <= size)
            };
            if length == 0 || !valid {
                return Err(INVALID.into());
            }
        }
    }
    cursor.done()
}
