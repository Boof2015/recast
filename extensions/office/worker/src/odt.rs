use crate::{docx::Document, error, Result};
use quick_xml::{events::Event, name::ResolveResult, NsReader};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

const MIME: &str = "application/vnd.oasis.opendocument.text";

fn local(value: &[u8]) -> String {
    String::from_utf8_lossy(value.rsplit(|c| *c == b':').next().unwrap_or(value)).into_owned()
}
fn resource(name: &str, href: &str, names: &BTreeSet<String>) -> Result<String> {
    let href = percent_encoding::percent_decode_str(href.split('#').next().unwrap_or(href))
        .decode_utf8()
        .map_err(error)?;
    if href.is_empty() {
        return Ok(String::new());
    }
    if href.starts_with('/') || href.contains([':', '\\', '?']) {
        return Err(
            "This document links to an external resource. Embed it before converting.".into(),
        );
    }
    let mut parts: Vec<_> = name
        .rsplit_once('/')
        .map(|(base, _)| base.split('/').collect())
        .unwrap_or_default();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop().ok_or("The ODT has an invalid resource path.")?;
            }
            _ => parts.push(part),
        }
    }
    let path = parts.join("/");
    if !names.contains(&path)
        && !names
            .iter()
            .any(|name| name.starts_with(&(path.clone() + "/")))
    {
        return Err("The ODT is missing a referenced image or document part.".into());
    }
    Ok(path)
}

fn inspect_xml(
    name: &str,
    bytes: &[u8],
    names: &BTreeSet<String>,
    types: &mut BTreeMap<String, String>,
    document: &mut Document,
) -> Result<()> {
    let mut reader = NsReader::from_reader(bytes);
    let mut root = String::new();
    let mut depth = 0usize;
    let mut roots = 0;
    let mut text_body = false;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| "The ODT contains damaged XML.")?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let tag_name = local(tag.name().as_ref());
                let office_namespace = matches!(reader.resolver().resolve_element(tag.name()).0, ResolveResult::Bound(ns) if ns.as_ref() == b"urn:oasis:names:tc:opendocument:xmlns:office:1.0");
                if depth == 0 {
                    roots += 1;
                    root = tag_name.clone();
                }
                if !empty {
                    depth += 1;
                }
                if roots > 1 || depth > 256 {
                    return Err("The ODT contains invalid XML nesting.".into());
                }
                let mut attrs = BTreeMap::new();
                for attr in tag.attributes() {
                    let attr = attr.map_err(error)?;
                    attrs.insert(
                        local(attr.key.as_ref()),
                        attr.decode_and_unescape_value(reader.decoder())
                            .map_err(error)?
                            .into_owned(),
                    );
                }
                match tag_name.as_str() {
                    "encryption-data" => return Err("Encrypted documents are not supported. Save an unencrypted copy.".into()),
                    "script" | "event-listener" => return Err("Documents containing macros or scripts are not supported yet.".into()),
                    "object-ole" | "applet" | "plugin" | "floating-frame" => return Err("Embedded objects are not supported yet.".into()),
                    "changed-region" | "change-start" | "change-end" | "change" => return Err("Accept or reject tracked changes in the document before converting it.".into()),
                    "dde-connection" | "dde-connection-decl" | "database-display" | "database-next" | "database-row-select" | "database-source-query" | "database-source-table" | "database-source-sql" => return Err("This document uses linked content or external fields. Unlink those fields before converting.".into()),
                    "annotation" if office_namespace => document.comments = true,
                    "text" if name == "content.xml" => text_body = true,
                    "file-entry" if name == "META-INF/manifest.xml" => {
                        let path = attrs.get("full-path").ok_or("The ODT manifest has an invalid entry.")?;
                        let mime = attrs.get("media-type").ok_or("The ODT manifest has an invalid entry.")?;
                        if types.insert(path.clone(), mime.clone()).is_some() { return Err("The ODT manifest contains duplicate entries.".into()); }
                    }
                    _ => {}
                }
                if let Some(href) = attrs.get("href") {
                    if tag_name != "a" {
                        let path = resource(name, href, names)?;
                        if tag_name == "object"
                            && types.get(&(path + "/")).map(String::as_str)
                                != Some("application/vnd.oasis.opendocument.formula")
                        {
                            return Err("Embedded Office objects are not supported yet. Native equations are supported.".into());
                        }
                    }
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("The ODT contains damaged XML.")?;
            }
            Event::DocType(_) => {
                return Err("ODT files with XML entities are not supported.".into())
            }
            Event::GeneralRef(reference) => {
                let entity = reference.decode().map_err(error)?;
                if !entity.starts_with('#')
                    && !["amp", "lt", "gt", "quot", "apos"].contains(&entity.as_ref())
                {
                    return Err("The ODT contains an unsupported XML entity.".into());
                }
            }
            Event::Text(text) if depth == 0 && !text.decode().map_err(error)?.trim().is_empty() => {
                return Err("The ODT contains text outside its XML root.".into())
            }
            Event::CData(_) if depth == 0 => {
                return Err("The ODT contains data outside its XML root.".into())
            }
            Event::Eof => {
                if roots != 1 || depth != 0 {
                    return Err("The ODT contains incomplete XML.".into());
                }
                break;
            }
            _ => {}
        }
    }
    if name == "content.xml" && (root != "document-content" || !text_body) {
        return Err("This is not an ODT text document.".into());
    }
    if name == "META-INF/manifest.xml"
        && (root != "manifest" || types.get("/").map(String::as_str) != Some(MIME))
    {
        return Err("Choose a standard ODT text document.".into());
    }
    Ok(())
}

pub fn inspect(bytes: &[u8]) -> Result<Document> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "This is not a readable ODT file.")?;
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
        if total > 512 * 1024 * 1024
            || entry.encrypted()
            || entry.is_symlink()
            || name.starts_with('/')
            || name.contains('\\')
            || name.split('/').any(|p| p == "..")
            || !names.insert(name.clone())
        {
            return Err("The ODT contains unsupported, duplicate, or oversized parts.".into());
        }
        if name.to_ascii_lowercase().starts_with("basic/")
            || name.to_ascii_lowercase().starts_with("scripts/")
        {
            return Err("Documents containing macros or scripts are not supported yet.".into());
        }
    }
    if !["mimetype", "META-INF/manifest.xml", "content.xml"]
        .iter()
        .all(|n| names.contains(*n))
    {
        return Err("The ODT is missing required document parts.".into());
    }
    let mut mime = String::new();
    zip.by_name("mimetype")
        .map_err(error)?
        .take(256)
        .read_to_string(&mut mime)
        .map_err(error)?;
    if mime != MIME {
        return Err("Choose an ODT text document, not a spreadsheet or presentation.".into());
    }
    let mut order: Vec<_> = names
        .iter()
        .filter(|name| !name.ends_with('/'))
        .cloned()
        .collect();
    order.sort_by_key(|name| (name != "META-INF/manifest.xml", name.clone()));
    let mut document = Document::default();
    let mut types = BTreeMap::new();
    for name in order {
        let entry = zip.by_name(&name).map_err(error)?;
        let xml = name.ends_with(".xml") || name.ends_with(".svg");
        let size = entry.size();
        if xml && size > 32 * 1024 * 1024 {
            return Err("A document XML part exceeds 32 MiB.".into());
        }
        let mut content = Vec::new();
        entry
            .take(size + 1)
            .read_to_end(&mut content)
            .map_err(|_| "The ODT contains a damaged document part.")?;
        if content.len() as u64 != size {
            return Err("The ODT contains an incomplete part.".into());
        }
        if xml {
            inspect_xml(&name, &content, &names, &mut types, &mut document)?;
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_hyperlinks_but_requires_embedded_resources() {
        let names = BTreeSet::from(["Pictures/a.png".into()]);
        assert_eq!(
            resource("content.xml", "./Pictures/a.png", &names).unwrap(),
            "Pictures/a.png"
        );
        for href in [
            "https://example.com/a.png",
            "../outside",
            "Pictures/missing.png",
            "%2fetc/passwd",
        ] {
            assert!(resource("content.xml", href, &names).is_err());
        }
        assert!(inspect_xml(
            "styles.xml",
            br#"<styles><a href="https://example.com"/></styles>"#,
            &names,
            &mut BTreeMap::new(),
            &mut Document::default()
        )
        .is_ok());
    }
    #[test]
    fn rejects_encryption_scripts_and_changes() {
        for xml in [
            "<manifest><encryption-data/></manifest>",
            "<styles><script/></styles>",
            "<styles><changed-region/></styles>",
            "<styles><object-ole/></styles>",
            "<styles>",
        ] {
            assert!(inspect_xml(
                "styles.xml",
                xml.as_bytes(),
                &BTreeSet::new(),
                &mut BTreeMap::new(),
                &mut Document::default()
            )
            .is_err());
        }
    }
}
