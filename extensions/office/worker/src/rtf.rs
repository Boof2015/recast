use crate::{documents::check_fields, docx::Document, Result};

#[derive(Clone, Default)]
struct State {
    instruction: bool,
    unicode_skip: usize,
}

pub fn inspect(bytes: &[u8]) -> Result<Document> {
    if !bytes.starts_with(b"{\\rtf1") {
        return Err("Choose a standard RTF document.".into());
    }
    let mut document = Document::default();
    let mut stack = Vec::new();
    let mut state = State {
        unicode_skip: 1,
        ..State::default()
    };
    let mut fields = String::new();
    let mut skip = 0;
    let mut i = 0;
    let mut closed = false;
    while i < bytes.len() {
        let byte = bytes[i];
        i += 1;
        if closed {
            if !byte.is_ascii_whitespace() {
                return Err("The RTF contains data after its document.".into());
            }
            continue;
        }
        match byte {
            b'{' => {
                stack.push(state.clone());
                if stack.len() > 256 {
                    return Err("The RTF contains excessive nesting.".into());
                }
                skip = 0;
            }
            b'}' => {
                state = stack.pop().ok_or("The RTF has unmatched braces.")?;
                closed = stack.is_empty();
                skip = 0;
            }
            b'\\' => {
                let symbol = *bytes
                    .get(i)
                    .ok_or("The RTF ends in an incomplete control.")?;
                i += 1;
                if !symbol.is_ascii_alphabetic() {
                    let value = if symbol == b'\'' {
                        let hex = bytes
                            .get(i..i + 2)
                            .ok_or("The RTF contains an incomplete escape.")?;
                        let hex = std::str::from_utf8(hex)
                            .map_err(|_| "The RTF has an invalid escape.")?;
                        i += 2;
                        u8::from_str_radix(hex, 16).map_err(|_| "The RTF has an invalid escape.")?
                    } else {
                        symbol
                    };
                    if skip > 0 {
                        skip -= 1;
                    } else if state.instruction && symbol != b'*' {
                        fields.push(value as char);
                    }
                    continue;
                }
                let start = i - 1;
                while bytes.get(i).is_some_and(u8::is_ascii_alphabetic) {
                    i += 1;
                }
                let word = std::str::from_utf8(&bytes[start..i]).unwrap();
                if word.len() > 64 {
                    return Err("The RTF has an invalid control word.".into());
                }
                let start = i;
                if bytes.get(i) == Some(&b'-') {
                    i += 1;
                }
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let number = if start != i {
                    Some(
                        std::str::from_utf8(&bytes[start..i])
                            .unwrap()
                            .parse::<i32>()
                            .map_err(|_| "The RTF has an invalid control value.")?,
                    )
                } else {
                    None
                };
                if bytes.get(i) == Some(&b' ') {
                    i += 1;
                }
                match word {
                    "rtf" if number != Some(1) => {
                        return Err("Choose a standard RTF document.".into());
                    }
                    "object" | "objdata" | "objlink" | "objautlink" | "objemb" | "filetbl"
                    | "datastore" => {
                        return Err("Embedded objects and linked resources are not supported yet. Embed images directly and remove attached objects before converting.".into());
                    }
                    "revised" | "deleted" | "crauth" | "crdate" | "revauth" | "revdttm"
                    | "revauthdel" | "revdttmdel"
                        if number != Some(0) =>
                    {
                        return Err("Accept or reject tracked changes in the document before converting it.".into());
                    }
                    "annotation" => document.comments = true,
                    "fldinst" => {
                        state.instruction = true;
                        fields.push(' ');
                    }
                    "fldrslt" => state.instruction = false,
                    "uc" => {
                        let count = number.ok_or("The RTF has an incomplete Unicode control.")?;
                        if !(0..=16).contains(&count) {
                            return Err("The RTF has an invalid Unicode fallback length.".into());
                        }
                        state.unicode_skip = count as usize;
                    }
                    "u" => {
                        let value = number.ok_or("The RTF has an incomplete Unicode escape.")?;
                        if !(-32768..=65535).contains(&value) {
                            return Err("The RTF has an invalid Unicode escape.".into());
                        }
                        if state.instruction {
                            fields.push(char::from_u32(value as u16 as u32).unwrap_or('\u{fffd}'));
                        }
                        skip = state.unicode_skip;
                    }
                    "bin" => {
                        let count = number.ok_or("The RTF has an incomplete binary control.")?;
                        if count < 0 || count as usize > bytes.len() - i {
                            return Err("The RTF contains truncated binary data.".into());
                        }
                        if state.instruction {
                            return Err("Binary field instructions are not supported.".into());
                        }
                        i += count as usize;
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => {}
            _ => {
                if skip > 0 {
                    skip -= 1;
                } else if state.instruction {
                    fields.push(byte as char);
                }
            }
        }
    }
    if !closed || !stack.is_empty() {
        return Err("The RTF is incomplete or has unmatched braces.".into());
    }
    check_fields(&fields)?;
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_binary_braces_and_escaped_fields() {
        assert!(inspect(br"{\rtf1{\pict\bin3 {}\}text}").is_ok());
        assert!(inspect(br#"{\rtf1{\field{\*\fldinst HYPERLINK "https://example.com"}}}"#).is_ok());
        for data in [
            br"{\rtf1{\*\fldinst INCL\'55DETEXT x}}".as_slice(),
            br"{\rtf1{\*\fldinst INCL\u85?DETEXT x}}",
        ] {
            assert!(inspect(data).err().unwrap().contains("external fields"));
        }
    }
    #[test]
    fn rejects_truncation_objects_and_changes() {
        for data in [
            br"{\rtf1 text".as_slice(),
            br"{\rtf1\bin200 x}",
            br"{\rtf1}trailing",
            br"{\rtf1{\object x}}",
            br"{\rtf1\revised text}",
            br"{\rtf10 text}",
        ] {
            assert!(inspect(data).is_err(), "{data:?}");
        }
        assert!(inspect(br"{\rtf1{\annotation note}text}").unwrap().comments);
        assert!(inspect(br"{\rtf1\revised0 text}").is_ok());
    }
}
