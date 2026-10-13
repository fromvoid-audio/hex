use crate::Error;
use hex_project::MAX_IMPORT_DOCUMENT_BYTES;
use std::io::Read;
pub const MAX_XML_BYTES: usize = 4 * 1024 * 1024;

pub fn decode(bytes: &[u8]) -> Result<String, Error> {
    if bytes.is_empty() || bytes.len() > MAX_IMPORT_DOCUMENT_BYTES {
        return Err(Error::Limit("original document bytes"));
    }
    let expanded = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut gzip = flate2::bufread::GzDecoder::new(bytes);
        let mut result = Vec::new();
        gzip.by_ref()
            .take(MAX_XML_BYTES as u64 + 1)
            .read_to_end(&mut result)
            .map_err(|_| Error::Invalid("corrupt gzip"))?;
        if result.len() > MAX_XML_BYTES {
            return Err(Error::Limit("expanded xml bytes"));
        }
        if !gzip.into_inner().is_empty() {
            return Err(Error::Invalid("extra gzip payload"));
        }
        result
    } else {
        bytes.to_vec()
    };
    String::from_utf8(expanded).map_err(|_| Error::Invalid("xml must be utf-8"))
}

/// Bound nesting before building a tree; reject declarations of entities even
/// when the tree parser would permit an empty DTD. No external resolution exists.
pub fn preflight(xml: &str) -> Result<(), Error> {
    use quick_xml::{Reader, events::Event};
    let mut reader = Reader::from_str(xml);
    let mut depth = 0usize;
    let mut nodes = 0usize;
    loop {
        match reader
            .read_event()
            .map_err(|_| Error::Invalid("malformed xml"))?
        {
            Event::DocType(_) => return Err(Error::Unsupported("dtd or entities")),
            Event::Start(e) => {
                check_element(&e, depth, &mut nodes)?;
                depth += 1;
            }
            Event::Empty(e) => check_element(&e, depth, &mut nodes)?,
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or(Error::Invalid("xml nesting"))?;
            }
            Event::Text(e) if e.len() > 16_384 => return Err(Error::Limit("xml text")),
            Event::CData(e) if e.len() > 16_384 => return Err(Error::Limit("xml text")),
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(Error::Invalid("xml nesting"));
    }
    Ok(())
}

fn check_element(
    e: &quick_xml::events::BytesStart<'_>,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), Error> {
    *nodes += 1;
    if *nodes > 100_000 {
        return Err(Error::Limit("xml elements"));
    }
    if depth >= 64 {
        return Err(Error::Limit("xml depth"));
    }
    if e.len() > 16_384 || e.attributes().count() > 32 {
        return Err(Error::Limit("xml attributes"));
    }
    Ok(())
}
