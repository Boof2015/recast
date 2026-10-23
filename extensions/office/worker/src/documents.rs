use crate::{doc, docx, odt, rtf, Result};
use std::io::Cursor;

pub fn inspect(bytes: &[u8]) -> Result<(&'static str, docx::Document)> {
    if bytes.starts_with(b"\xd0\xcf\x11\xe0") {
        return doc::inspect(bytes).map(|document| ("doc", document));
    }
    if bytes.starts_with(b"{\\rtf") {
        return rtf::inspect(bytes).map(|document| ("rtf", document));
    }
    if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) {
        if archive.by_name("mimetype").is_ok() {
            return odt::inspect(bytes).map(|document| ("odt", document));
        }
    }
    docx::inspect(bytes).map(|document| ("docx", document))
}

pub fn check_fields(fields: &str) -> Result<()> {
    if fields
        .to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|word| {
            [
                "INCLUDEPICTURE",
                "INCLUDETEXT",
                "DDE",
                "DDEAUTO",
                "DATABASE",
                "LINK",
            ]
            .contains(&word)
        })
    {
        return Err("This document uses linked content or external fields. Unlink those fields before converting.".into());
    }
    Ok(())
}
