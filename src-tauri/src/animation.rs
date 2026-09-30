//! Bounded container inspection; pixel decoding remains the worker's job.
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};

pub const MAX_FRAMES: usize = 1000;
const MAX_CANVAS_PIXELS: u64 = 100_000_000;

#[derive(Debug, Clone)]
pub struct Sequence {
    pub width: u32,
    pub height: u32,
    pub delays: Vec<u32>, // milliseconds
    pub loops: u32,       // total plays; zero means forever
    delay_offsets: Vec<u64>,
}

impl Sequence {
    pub fn animated(&self) -> bool {
        self.delays.len() > 1
    }
    fn validate(self) -> Result<Self, String> {
        if self.width == 0
            || self.height == 0
            || self.width > 32768
            || self.height > 32768
            || self.delays.is_empty()
        {
            return Err("This image has invalid dimensions or no frames.".into());
        }
        if self.delays.len() > MAX_FRAMES
            || (self.animated()
                && u64::from(self.width) * u64::from(self.height) * self.delays.len() as u64
                    > MAX_CANVAS_PIXELS)
        {
            return Err("This animation is too large for this build. Use at most 1,000 frames and 100 million pixels across the full frames.".into());
        }
        Ok(self)
    }
    pub fn target_issue(&self, target: &str) -> Option<String> {
        if !self.animated() {
            return None;
        }
        match target {
            "webp" if self.loops > 65535 => Some("This animation repeats more times than WebP can store. Choose GIF to retain its loop count.".into()),
            "gif" if self.delays.iter().any(|&ms| (ms > 0 && ms < 10) || ms > 655350) => Some("This animation has frame timing outside GIF’s range. Choose WebP to preserve it.".into()),
            "gif" | "webp" => None,
            _ => Some("This file is animated. Choose GIF or WebP, or remove it to convert the rest.".into()),
        }
    }
    // GIF stores centiseconds. Round cumulative timestamps to avoid drift over
    // long sequences (e.g. 33/33/34 ms); retain zero-delay frames as zero.
    pub fn gif_delays(&self) -> Vec<u32> {
        let (mut elapsed, mut previous) = (0u64, 0u64);
        self.delays
            .iter()
            .map(|&ms| {
                elapsed += u64::from(ms);
                let rounded = (elapsed + 5) / 10;
                let duration = ((rounded - previous) * 10) as u32;
                previous = rounded;
                duration
            })
            .collect()
    }
    pub fn finish_output(
        &self,
        path: &std::path::Path,
        target: &str,
        resize: u8,
    ) -> Result<(), String> {
        if !self.animated() {
            return Ok(());
        }
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| "The animation output could not be checked.")?;
        let actual = if target == "gif" {
            inspect_gif(&mut file)?
        } else {
            inspect_webp(&mut file)?.ok_or("The converter lost the animation.")?
        };
        let scale =
            |dimension: u32| ((u64::from(dimension) * u64::from(resize) + 50) / 100).max(1) as u32;
        if actual.delays.len() != self.delays.len()
            || actual.loops != self.loops
            || ![
                (scale(self.width), scale(self.height)),
                (scale(self.height), scale(self.width)),
            ]
            .contains(&(actual.width, actual.height))
        {
            return Err(
                "The converter did not preserve the animation’s frames, dimensions, or looping."
                    .into(),
            );
        }
        if target == "gif" {
            let delays = self.gif_delays();
            if actual.delay_offsets.len() != delays.len() {
                return Err("The GIF output is missing frame timing.".into());
            }
            for (offset, ms) in actual.delay_offsets.iter().zip(delays) {
                file.seek(SeekFrom::Start(*offset))
                    .and_then(|_| file.write_all(&((ms / 10) as u16).to_le_bytes()))
                    .map_err(|_| "The GIF timing could not be saved.")?;
            }
        } else if actual.delays != self.delays {
            return Err("The converter did not preserve the animation’s timing.".into());
        }
        Ok(())
    }
}

struct Reader<'a> {
    file: &'a mut File,
    length: u64,
    position: u64,
    blocks: usize,
}
impl<'a> Reader<'a> {
    fn new(file: &'a mut File) -> std::io::Result<Self> {
        let length = file.metadata()?.len();
        file.rewind()?;
        Ok(Self {
            file,
            length,
            position: 0,
            blocks: 0,
        })
    }
    fn read<const N: usize>(&mut self) -> std::io::Result<[u8; N]> {
        let mut bytes = [0; N];
        self.file.read_exact(&mut bytes)?;
        self.position += N as u64;
        Ok(bytes)
    }
    fn skip(&mut self, count: u64) -> std::io::Result<()> {
        let end = self
            .position
            .checked_add(count)
            .filter(|&end| end <= self.length)
            .ok_or_else(invalid)?;
        self.file.seek(SeekFrom::Start(end))?;
        self.position = end;
        Ok(())
    }
    fn subblocks(&mut self) -> std::io::Result<()> {
        loop {
            self.blocks += 1;
            if self.blocks > 1_000_000 {
                return Err(invalid());
            }
            let size = self.read::<1>()?[0];
            if size == 0 {
                return Ok(());
            }
            self.skip(u64::from(size))?;
        }
    }
}
fn invalid() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid image container")
}
fn le16(bytes: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes(bytes[..2].try_into().unwrap()))
}
fn le24(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}
fn le32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().unwrap())
}

pub fn inspect_gif(file: &mut File) -> Result<Sequence, String> {
    let mut check = || -> std::io::Result<Sequence> {
        let mut r = Reader::new(file)?;
        let header = r.read::<13>()?;
        if &header[..6] != b"GIF87a" && &header[..6] != b"GIF89a" {
            return Err(invalid());
        }
        let mut info = Sequence {
            width: le16(&header[6..]),
            height: le16(&header[8..]),
            delays: vec![],
            loops: 1,
            delay_offsets: vec![],
        };
        let global_colors = if header[10] & 0x80 != 0 {
            2u32 << (header[10] & 7)
        } else {
            0
        };
        r.skip(u64::from(global_colors * 3))?;
        let mut control: Option<(u32, u64, Option<u8>)> = None;
        let mut loop_seen = false;
        loop {
            match r.read::<1>()?[0] {
                0x3b => {
                    if r.position != r.length || control.is_some() {
                        return Err(invalid());
                    }
                    return Ok(info);
                }
                0x21 => match r.read::<1>()?[0] {
                    0xf9 => {
                        if control.is_some() {
                            return Err(invalid());
                        }
                        let start = r.position;
                        let data = r.read::<6>()?;
                        if data[0] != 4
                            || data[5] != 0
                            || data[1] & 0xe2 != 0
                            || (data[1] >> 2) & 7 > 3
                        {
                            return Err(invalid());
                        }
                        control = Some((
                            le16(&data[2..]) * 10,
                            start + 2,
                            (data[1] & 1 != 0).then_some(data[4]),
                        ));
                    }
                    0xff => {
                        if r.read::<1>()?[0] != 11 {
                            return Err(invalid());
                        }
                        let app = r.read::<11>()?;
                        if &app == b"NETSCAPE2.0" || &app == b"ANIMEXTS1.0" {
                            if loop_seen {
                                return Err(invalid());
                            }
                            let data = r.read::<5>()?;
                            if data[0] != 3 || data[1] != 1 || data[4] != 0 {
                                return Err(invalid());
                            }
                            let repeats = le16(&data[2..]);
                            info.loops = if repeats == 0 { 0 } else { repeats + 1 };
                            loop_seen = true;
                        } else {
                            r.subblocks()?;
                        }
                    }
                    0xfe => r.subblocks()?,
                    // Plain-text rendering and input-driven animations cannot
                    // be preserved by this image-only path.
                    _ => return Err(invalid()),
                },
                0x2c => {
                    if info.delays.len() >= MAX_FRAMES {
                        return Err(invalid());
                    }
                    let image = r.read::<9>()?;
                    let (x, y, w, h) = (
                        le16(&image),
                        le16(&image[2..]),
                        le16(&image[4..]),
                        le16(&image[6..]),
                    );
                    if w == 0
                        || h == 0
                        || x + w > info.width
                        || y + h > info.height
                        || image[8] & 0x18 != 0
                    {
                        return Err(invalid());
                    }
                    let colors = if image[8] & 0x80 != 0 {
                        let count = 2u32 << (image[8] & 7);
                        r.skip(u64::from(count * 3))?;
                        count
                    } else {
                        global_colors
                    };
                    if colors == 0 {
                        return Err(invalid());
                    }
                    let (delay, offset, transparent) = control.take().unwrap_or((0, 0, None));
                    if transparent.is_some_and(|index| u32::from(index) >= colors)
                        || !(2..=8).contains(&r.read::<1>()?[0])
                    {
                        return Err(invalid());
                    }
                    let start = r.position;
                    r.subblocks()?;
                    if r.position == start + 1 {
                        return Err(invalid());
                    }
                    info.delays.push(delay);
                    if offset != 0 {
                        info.delay_offsets.push(offset);
                    }
                }
                _ => return Err(invalid()),
            }
        }
    };
    check()
        .map_err(|_| {
            "This GIF is incomplete or uses unsupported frame, text, or interaction data."
                .to_string()
        })?
        .validate()
}

pub fn inspect_webp(file: &mut File) -> Result<Option<Sequence>, String> {
    let mut check = || -> std::io::Result<Option<Sequence>> {
        let mut r = Reader::new(file)?;
        let header = r.read::<12>()?;
        if &header[..4] != b"RIFF"
            || &header[8..] != b"WEBP"
            || u64::from(le32(&header[4..])) + 8 != r.length
        {
            return Err(invalid());
        }
        let mut info = Sequence {
            width: 0,
            height: 0,
            delays: vec![],
            loops: 1,
            delay_offsets: vec![],
        };
        let (mut extended, mut animated, mut anim_seen, mut stills) = (false, false, false, 0);
        let (mut profile_flags, mut seen_profiles, mut profile_bytes) = (0u8, 0u8, 0u64);
        while r.position < r.length {
            r.blocks += 1;
            if r.blocks > 16384 {
                return Err(invalid());
            }
            let header = r.read::<8>()?;
            let size = u64::from(le32(&header[4..]));
            let end = r
                .position
                .checked_add(size)
                .filter(|&end| end + size % 2 <= r.length)
                .ok_or_else(invalid)?;
            match &header[..4] {
                b"VP8X" => {
                    if extended || r.position != 20 || size != 10 {
                        return Err(invalid());
                    }
                    let data = r.read::<10>()?;
                    if data[0] & 0xc1 != 0 || data[1..4] != [0, 0, 0] {
                        return Err(invalid());
                    }
                    extended = true;
                    animated = data[0] & 2 != 0;
                    profile_flags = data[0] & 0x2c;
                    info.width = le24(&data[4..]) + 1;
                    info.height = le24(&data[7..]) + 1;
                }
                b"ANIM" => {
                    if !animated || anim_seen || size != 6 || !info.delays.is_empty() {
                        return Err(invalid());
                    }
                    let data = r.read::<6>()?;
                    info.loops = le16(&data[4..]);
                    anim_seen = true;
                }
                b"ANMF" => {
                    if !anim_seen || size < 24 || info.delays.len() >= MAX_FRAMES {
                        return Err(invalid());
                    }
                    let data = r.read::<16>()?;
                    let (x, y, w, h) = (
                        le24(&data) * 2,
                        le24(&data[3..]) * 2,
                        le24(&data[6..]) + 1,
                        le24(&data[9..]) + 1,
                    );
                    if data[15] & 0xfc != 0 || x + w > info.width || y + h > info.height {
                        return Err(invalid());
                    }
                    let (mut images, mut alpha) = (0, false);
                    while r.position < end {
                        if r.position + 8 > end {
                            return Err(invalid());
                        }
                        let chunk = r.read::<8>()?;
                        let count = u64::from(le32(&chunk[4..]));
                        if count == 0 || r.position + count + count % 2 > end {
                            return Err(invalid());
                        }
                        match &chunk[..4] {
                            b"ALPH" if !alpha && images == 0 => alpha = true,
                            b"VP8 " if images == 0 => images += 1,
                            b"VP8L" if images == 0 && !alpha => images += 1,
                            _ => return Err(invalid()),
                        }
                        r.skip(count + count % 2)?;
                    }
                    if images != 1 {
                        return Err(invalid());
                    }
                    info.delays.push(le24(&data[12..]));
                }
                b"ICCP" | b"EXIF" | b"XMP " => {
                    let flag = match &header[..4] {
                        b"ICCP" => 0x20,
                        b"EXIF" => 0x08,
                        _ => 0x04,
                    };
                    if !extended || seen_profiles & flag != 0 {
                        return Err(invalid());
                    }
                    seen_profiles |= flag;
                    profile_bytes += size;
                    if profile_bytes > 16 * 1024 * 1024 {
                        return Err(invalid());
                    }
                }
                b"VP8 " | b"VP8L" => {
                    if animated || size == 0 {
                        return Err(invalid());
                    }
                    stills += 1;
                }
                _ => {}
            }
            if r.position > end {
                return Err(invalid());
            }
            r.skip(end - r.position + size % 2)?;
        }
        if profile_flags != seen_profiles
            || profile_bytes * info.delays.len().max(1) as u64 > 16 * 1024 * 1024
        {
            return Err(invalid());
        }
        if animated {
            if !anim_seen || info.delays.is_empty() || stills != 0 {
                return Err(invalid());
            }
            Ok(Some(info))
        } else if stills == 1 {
            Ok(None)
        } else {
            Err(invalid())
        }
    };
    check()
        .map_err(|_| "This WebP is incomplete or has inconsistent animation data.".to_string())?
        .map(Sequence::validate)
        .transpose()
}
