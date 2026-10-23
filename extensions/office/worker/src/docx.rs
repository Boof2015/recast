use crate::{error, Result};
use quick_xml::{events::Event, Reader};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

const MAIN: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
#[derive(Default)]
pub struct Document {
    pub comments: bool,
}
fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|c| *c == b':').next().unwrap_or(name)
}
fn inspect_xml(
    name: &str,
    bytes: &[u8],
    _document: &mut Document,
    names: &BTreeSet<String>,
) -> Result<()> {
    let mut reader = Reader::from_reader(bytes);
    let mut fields = String::new();
    let mut in_instruction = false;
    let mut root = None;
    let mut main_type = false;
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| "The DOCX contains damaged XML.".to_string())?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                if depth == 0 {
                    roots += 1;
                }
                if !empty {
                    depth += 1;
                }
                if depth > 256 || roots > 1 {
                    return Err("The DOCX contains invalid XML nesting.".into());
                }
                let tag_name = String::from_utf8_lossy(local(tag.name().as_ref())).into_owned();
                if root.is_none() {
                    root = Some(tag_name.clone());
                }
                let mut attrs = BTreeMap::new();
                for attr in tag.attributes() {
                    let attr = attr.map_err(error)?;
                    attrs.insert(
                        String::from_utf8_lossy(local(attr.key.as_ref())).into_owned(),
                        attr.decode_and_unescape_value(reader.decoder())
                            .map_err(error)?
                            .into_owned(),
                    );
                }
                if name == "[Content_Types].xml" {
                    if attrs
                        .get("PartName")
                        .is_some_and(|s| s == "/word/document.xml")
                        && attrs.get("ContentType").is_some_and(|s| s == MAIN)
                    {
                        main_type = true;
                    }
                    if attrs.get("ContentType").is_some_and(|s| {
                        s.to_ascii_lowercase().contains("macroenabled")
                            || s.to_ascii_lowercase().contains("vbaproject")
                    }) {
                        return Err("Macro-enabled documents are not supported yet. Save a macro-free DOCX copy.".into());
                    }
                }
                if name.ends_with(".rels") && tag_name == "Relationship" {
                    let target = attrs
                        .get("Target")
                        .ok_or("A DOCX relationship has no target.")?;
                    let kind = attrs
                        .get("Type")
                        .ok_or("A DOCX relationship has no type.")?;
                    if attrs
                        .get("TargetMode")
                        .is_some_and(|s| s.eq_ignore_ascii_case("External"))
                    {
                        if !kind.ends_with("/hyperlink") {
                            return Err("This document links to an external image, template, or other resource. Embed it in the DOCX before converting.".into());
                        }
                    } else {
                        let base = name
                            .rsplit_once("/_rels/")
                            .map(|(base, _)| base)
                            .unwrap_or("");
                        let target = percent_encoding::percent_decode_str(
                            target.split('#').next().unwrap_or(target),
                        )
                        .decode_utf8()
                        .map_err(error)?;
                        if target.contains([':', '\\', '?']) {
                            return Err("Invalid internal DOCX relationship.".into());
                        }
                        let mut parts: Vec<&str> = if target.starts_with('/') {
                            Vec::new()
                        } else {
                            base.split('/').filter(|p| !p.is_empty()).collect()
                        };
                        for part in target.split('/') {
                            match part {
                                "" | "." => {}
                                ".." => {
                                    if parts.pop().is_none() {
                                        return Err("Invalid DOCX part path.".into());
                                    }
                                }
                                _ => parts.push(part),
                            }
                        }
                        if !names.contains(&parts.join("/")) {
                            return Err(
                                "The DOCX is missing a referenced image or document part.".into()
                            );
                        }
                    }
                }
                if name.starts_with("word/") {
                    if ["altChunk", "OLEObject", "control", "object", "subDoc"]
                        .contains(&tag_name.as_str())
                    {
                        return Err(
                            "Embedded objects and imported document chunks are not supported yet."
                                .into(),
                        );
                    }
                    if [
                        "ins",
                        "del",
                        "moveFrom",
                        "moveTo",
                        "cellIns",
                        "cellDel",
                        "cellMerge",
                        "numberingChange",
                    ]
                    .contains(&tag_name.as_str())
                        || tag_name.ends_with("PrChange")
                        || [
                            "moveFromRange",
                            "moveToRange",
                            "customXmlInsRange",
                            "customXmlDelRange",
                            "customXmlMoveFromRange",
                            "customXmlMoveToRange",
                        ]
                        .iter()
                        .any(|prefix| tag_name.starts_with(prefix))
                    {
                        return Err("Accept or reject tracked changes in the document before converting it.".into());
                    }
                    if tag_name == "fldChar" {
                        fields.push(' ');
                    }
                    if tag_name == "fldSimple" {
                        if let Some(instruction) = attrs.get("instr") {
                            fields.push_str(instruction);
                        }
                    }
                    if tag_name == "instrText" && !empty {
                        in_instruction = true;
                    }
                }
            }
            Event::End(tag) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("The DOCX contains damaged XML.")?;
                if local(tag.name().as_ref()) == b"instrText" {
                    in_instruction = false;
                }
            }
            Event::Text(text) if in_instruction => fields.push_str(&text.decode().map_err(error)?),
            Event::Text(text) if depth == 0 && !text.decode().map_err(error)?.trim().is_empty() => {
                return Err("The DOCX contains text outside its XML root.".into())
            }
            Event::CData(_) if depth == 0 => {
                return Err("The DOCX contains data outside its XML root.".into())
            }
            Event::CData(text) if in_instruction => fields.push_str(&text.decode().map_err(error)?),
            Event::GeneralRef(reference) => {
                let entity = reference.decode().map_err(error)?;
                if in_instruction {
                    if let Some(value) = entity
                        .strip_prefix("#x")
                        .and_then(|s| u32::from_str_radix(s, 16).ok())
                        .or_else(|| entity.strip_prefix('#').and_then(|s| s.parse().ok()))
                        .and_then(char::from_u32)
                    {
                        fields.push(value);
                    } else if let Some(value) = match entity.as_ref() {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    } {
                        fields.push(value);
                    } else {
                        return Err("The DOCX contains an unsupported XML entity.".into());
                    }
                }
                if !entity.starts_with('#')
                    && !["amp", "lt", "gt", "quot", "apos"].contains(&entity.as_ref())
                {
                    return Err("The DOCX contains an unsupported XML entity.".into());
                }
            }
            Event::DocType(_) => {
                return Err("DOCX files with external XML entities are not supported.".into())
            }
            Event::Eof => {
                if depth != 0 || roots != 1 {
                    return Err("The DOCX contains incomplete XML.".into());
                }
                break;
            }
            _ => {}
        }
    }
    if name == "word/document.xml" && root.as_deref() != Some("document") {
        return Err("This is not a Word DOCX document.".into());
    }
    if name == "[Content_Types].xml" && !main_type {
        return Err("Choose a standard, macro-free DOCX document.".into());
    }
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
pub fn inspect(bytes: &[u8]) -> Result<Document> {
    if bytes.starts_with(b"\xd0\xcf\x11\xe0") {
        return Err("Encrypted DOCX and legacy DOC files are not supported yet. Choose an unencrypted DOCX copy.".into());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "This is not a readable DOCX file.".to_string())?;
    if zip.len() > 10_000 {
        return Err("This document contains too many parts.".into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(error)?;
        let name = entry.name().map_err(error)?.into_owned();
        total = total
            .checked_add(entry.size())
            .ok_or("This document is too large.")?;
        if total > MAX_EXPANDED
            || entry.is_symlink()
            || entry.encrypted()
            || name.contains('\\')
            || name.starts_with('/')
            || name.split('/').any(|p| p == "..")
            || !names.insert(name.clone())
        {
            return Err("The DOCX contains unsupported, duplicate, or oversized parts.".into());
        }
        if name.to_ascii_lowercase().contains("vbaproject")
            || name.starts_with("word/activeX/")
            || name.starts_with("word/embeddings/")
        {
            return Err("Macros and embedded Office objects are not supported yet.".into());
        }
    }
    if !["[Content_Types].xml", "_rels/.rels", "word/document.xml"]
        .iter()
        .all(|n| names.contains(*n))
    {
        return Err("This DOCX is missing required document parts.".into());
    }
    let mut document = Document::default();
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(error)?;
        let name = entry.name().map_err(error)?.into_owned();
        if entry.is_dir() {
            continue;
        }
        // Read every part, including images, to verify CRC and truncation.
        let size = entry.size();
        if (name.ends_with(".xml") || name.ends_with(".rels")) && size > 32 * 1024 * 1024 {
            return Err("A document XML part exceeds 32 MiB.".into());
        }
        let mut content = Vec::new();
        entry
            .take(size + 1)
            .read_to_end(&mut content)
            .map_err(|_| "The DOCX contains a damaged document part.".to_string())?;
        if content.len() as u64 != size {
            return Err("The DOCX contains an incomplete document part.".into());
        }
        if name.ends_with(".xml") || name.ends_with(".rels") {
            if content.len() > 32 * 1024 * 1024 {
                return Err("A document XML part exceeds 32 MiB.".into());
            }
            inspect_xml(&name, &content, &mut document, &names)?;
        }
        if name == "word/comments.xml" {
            document.comments = true;
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn xml(value: &str) -> Result<()> {
        inspect_xml(
            "word/document.xml",
            value.as_bytes(),
            &mut Document::default(),
            &BTreeSet::new(),
        )
    }
    #[test]
    fn rejects_truncated_or_multiple_roots() {
        assert!(xml("<document><body/></document>").is_ok());
        for bad in [
            "<document><body/>",
            "<document/><document/>",
            "<document><body></document>",
            "",
        ] {
            assert!(xml(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn prevents_external_fields_and_active_content_without_rejecting_links() {
        assert!(
            xml("<document><fldSimple instr=\"HYPERLINK https://example.com\"/></document>")
                .is_ok()
        );
        for bad in [
            "<document><fldSimple instr=\"INCLUDETEXT file:///missing\"/></document>",
            "<document><instrText>INCLUDE</instrText><instrText>TEXT file:///missing</instrText></document>",
            "<document><instrText>INCLUDE&#84;EXT file:///missing</instrText></document>",
            "<document><instrText><![CDATA[INCLUDETEXT file:///missing]]></instrText></document>",
            "<document><object/></document>",
            "<document><ins/></document>",
            "<document><pPrChange/></document>",
            "<document><cellDel/></document>",
            "<!DOCTYPE document [<!ENTITY e SYSTEM 'file:///missing'>]><document/>",
        ] { assert!(xml(bad).is_err(), "{bad}"); }
    }
    #[test]
    fn validates_relationships_without_platform_dependent_file_urls() {
        let names = BTreeSet::from(["word/media/image.png".into()]);
        for target in [
            "media/image.png",
            "media/./image.png",
            "/word/media/image.png",
            "media/%69mage.png",
        ] {
            let value = format!(
                "<Relationships><Relationship Type='image' Target='{target}'/></Relationships>"
            );
            assert!(inspect_xml(
                "word/_rels/document.xml.rels",
                value.as_bytes(),
                &mut Document::default(),
                &names
            )
            .is_ok());
        }
        for target in [
            "../../../outside.png",
            "media/absent.png",
            "file:///image.png",
            "https://example.com/image.png",
        ] {
            let value = format!(
                "<Relationships><Relationship Type='image' Target='{target}'/></Relationships>"
            );
            assert!(inspect_xml(
                "word/_rels/document.xml.rels",
                value.as_bytes(),
                &mut Document::default(),
                &names
            )
            .is_err());
        }
    }
}
