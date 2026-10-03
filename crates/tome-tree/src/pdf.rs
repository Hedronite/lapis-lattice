//! PDF bytes, per-page text, and the content hash that keys the store.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use std::collections::HashSet;

use lopdf::content::Content;
use lopdf::{Document, Object, ObjectId};
use sha2::{Digest, Sha256};

use crate::error::{Result, io, parse};

pub(crate) struct LoadedPdf {
    pub doc: Document,
    pub pages: Vec<String>,
    pub sha256: String,
}

pub(crate) fn load(path: &Path) -> Result<LoadedPdf> {
    let bytes = std::fs::read(path).map_err(|e| io(format!("{}: {e}", path.display())))?;
    if bytes.is_empty() {
        return Err(parse(format!("{}: empty file", path.display())));
    }
    let sha256 = sha256_bytes(&bytes);
    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Document::load_mem(&bytes)));
    let doc = match loaded {
        Ok(Ok(doc)) => doc,
        Ok(Err(e)) => return Err(parse(format!("{}: {e}", path.display()))),
        Err(_) => return Err(parse(format!("{}: pdf parser panicked", path.display()))),
    };
    let ids = page_ids(&doc)?;
    let mut pages = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().copied().enumerate() {
        pages.push(page_text(&doc, (i as u32) + 1, id));
    }
    Ok(LoadedPdf { doc, pages, sha256 })
}

const MAX_DEPTH: u32 = 64;
/// Same bound lopdf uses for a page tree. A linked list of `/Pages` nodes
/// in a long book is deeper than 64 and must still load.
const PAGE_TREE_DEPTH: u32 = 256;

/// Physical pages in reading order. A `/Kids` or `/Parent` cycle is `parse`,
/// not a stack overflow inside lopdf.
fn page_ids(doc: &Document) -> Result<Vec<ObjectId>> {
    let catalog = doc.catalog().map_err(parse)?;
    let root =
        catalog.get(b"Pages").and_then(Object::as_reference).map_err(|_| parse("pdf has no page tree"))?;
    let mut out = Vec::new();
    let mut stack = vec![(root, 0u32)];
    let mut seen = HashSet::new();
    let mut steps = 0u32;
    while let Some((id, depth)) = stack.pop() {
        steps += 1;
        if steps > 100_000 {
            return Err(parse("page tree has more than 100000 nodes"));
        }
        if !seen.insert(id) {
            return Err(parse("page tree cycle"));
        }
        if depth > PAGE_TREE_DEPTH {
            return Err(parse("page tree deeper than 256"));
        }
        let dict = doc.get_dictionary(id).map_err(parse)?;
        let kind = dict.get(b"Type").ok().and_then(|o| o.as_name().ok());
        // `/Kids` is often an indirect array. `get` would miss it and fail the book.
        let kids = dict.get_deref(b"Kids", doc).ok().and_then(|o| o.as_array().ok());
        let is_pages = kind == Some(b"Pages") || (kind != Some(b"Page") && kids.is_some());
        if !is_pages {
            out.push(id);
            continue;
        }
        let Some(kids) = kids else {
            continue;
        };
        for kid in kids.iter().rev() {
            let Ok(kid_id) = kid.as_reference() else {
                continue;
            };
            stack.push((kid_id, depth + 1));
        }
    }
    if out.is_empty() {
        return Err(parse("pdf has no pages"));
    }
    Ok(out)
}

/// One page of text. pdf-extract panics become a lopdf fallback for that page.
/// A cyclic `/Parent` or Form XObject is not handed to pdf-extract: that walk
/// recurses with no bound and aborts the process on stack overflow.
fn page_text(doc: &Document, page_num: u32, page_id: ObjectId) -> String {
    // The safety scan walks content streams. A panic there must not kill the book.
    let safe = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pdf_extract_safe(doc, page_id)));
    if !matches!(safe, Ok(true)) {
        return lopdf_page(doc, page_num);
    }
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut s = String::new();
        let mut output = pdf_extract::PlainTextOutput::new(&mut s);
        pdf_extract::output_doc_page(doc, &mut output, page_num).map(|_| s)
    }));
    match extracted {
        Ok(Ok(text)) => text,
        _ => lopdf_page(doc, page_num),
    }
}

fn lopdf_page(doc: &Document, page_num: u32) -> String {
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        doc.extract_text(&[page_num]).unwrap_or_default()
    }));
    extracted.unwrap_or_default()
}

fn pdf_extract_safe(doc: &Document, page_id: ObjectId) -> bool {
    if parent_chain_unsafe(doc, page_id) {
        return false;
    }
    !form_graph_unsafe(doc, page_id)
}

fn parent_chain_unsafe(doc: &Document, page_id: ObjectId) -> bool {
    let mut seen = HashSet::new();
    let mut current = page_id;
    for _ in 0..=MAX_DEPTH {
        if !seen.insert(current) {
            return true;
        }
        let Ok(dict) = doc.get_dictionary(current) else {
            return false;
        };
        let Ok(Object::Reference(id)) = dict.get(b"Parent") else {
            return false;
        };
        current = *id;
    }
    true
}

/// `true` when a Form XObject `Do` cycle (or a graph deeper than 64) would
/// make pdf-extract recurse without a bound.
fn form_graph_unsafe(doc: &Document, page_id: ObjectId) -> bool {
    let page_resources = inherited_resources(doc, page_id);
    let contents = doc.get_dictionary(page_id).ok().and_then(|page| page.get(b"Contents").ok().cloned());
    let mut stack: Vec<(ObjectId, Option<lopdf::Dictionary>, u32)> = Vec::new();
    if let Some(contents) = contents.as_ref() {
        push_contents(&mut stack, contents, page_resources.clone(), 0);
    }
    let mut seen = HashSet::new();
    let mut steps = 0u32;
    while let Some((id, resources, depth)) = stack.pop() {
        steps += 1;
        if steps > 10_000 || depth > MAX_DEPTH {
            return true;
        }
        if !seen.insert(id) {
            return true;
        }
        let Ok(obj) = doc.get_object(id) else {
            continue;
        };
        let Ok(stream) = obj.as_stream() else {
            continue;
        };
        let Ok(bytes) = stream.decompressed_content() else {
            return true;
        };
        let Ok(content) = Content::decode(&bytes) else {
            return true;
        };
        let own = stream_resources(doc, &stream.dict).or(resources);
        for op in &content.operations {
            if op.operator != "Do" {
                continue;
            }
            let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
                continue;
            };
            let Some(xid) = xobject_id(doc, own.as_ref(), name) else {
                continue;
            };
            // pdf-extract recurses into every `Do` target, including images.
            // Anything that is not a shallow Form DAG is extracted with lopdf.
            if !is_form(doc, xid) {
                return true;
            }
            let form_resources = doc
                .get_object(xid)
                .ok()
                .and_then(|o| o.as_stream().ok())
                .and_then(|s| stream_resources(doc, &s.dict))
                .or_else(|| own.clone());
            stack.push((xid, form_resources, depth + 1));
        }
    }
    false
}

fn push_contents(
    stack: &mut Vec<(ObjectId, Option<lopdf::Dictionary>, u32)>,
    obj: &Object,
    resources: Option<lopdf::Dictionary>,
    depth: u32,
) {
    match obj {
        Object::Reference(id) => stack.push((*id, resources, depth)),
        Object::Array(items) => {
            for item in items.iter().rev() {
                push_contents(stack, item, resources.clone(), depth);
            }
        }
        _ => {}
    }
}

fn inherited_resources(doc: &Document, start: ObjectId) -> Option<lopdf::Dictionary> {
    let mut seen = HashSet::new();
    let mut current = start;
    for _ in 0..=MAX_DEPTH {
        if !seen.insert(current) {
            return None;
        }
        let Ok(dict) = doc.get_dictionary(current) else {
            return None;
        };
        if let Some(resources) = stream_resources(doc, dict) {
            return Some(resources);
        }
        let Ok(Object::Reference(id)) = dict.get(b"Parent") else {
            return None;
        };
        current = *id;
    }
    None
}

fn stream_resources(doc: &Document, dict: &lopdf::Dictionary) -> Option<lopdf::Dictionary> {
    let obj = dict.get(b"Resources").ok()?;
    match obj {
        Object::Dictionary(d) => Some(d.clone()),
        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
        _ => None,
    }
}

fn xobject_id(doc: &Document, resources: Option<&lopdf::Dictionary>, name: &[u8]) -> Option<ObjectId> {
    let resources = resources?;
    let xobjects = resources.get(b"XObject").ok()?;
    let dict = match xobjects {
        Object::Dictionary(d) => d.clone(),
        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned()?,
        _ => return None,
    };
    match dict.get(name).ok()? {
        Object::Reference(id) => Some(*id),
        _ => None,
    }
}

fn is_form(doc: &Document, id: ObjectId) -> bool {
    doc.get_object(id)
        .ok()
        .and_then(|obj| obj.as_stream().ok())
        .and_then(|stream| stream.dict.get(b"Subtype").ok())
        .and_then(|o| o.as_name().ok())
        == Some(b"Form")
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| io(format!("{}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

pub(crate) fn lead_text(pages: &[String], start: u32, end: u32) -> String {
    if start == 0 || end < start {
        return String::new();
    }
    let mut acc = String::new();
    for page in start..=end {
        let Some(text) = pages.get((page as usize).wrapping_sub(1)) else {
            break;
        };
        if !acc.is_empty() {
            acc.push('\n');
        }
        acc.push_str(text.trim());
        if acc.chars().count() >= crate::LEAD_CHARS {
            break;
        }
    }
    acc.trim().chars().take(crate::LEAD_CHARS).collect()
}

pub(crate) fn char_count(pages: &[String], start: u32, end: u32) -> usize {
    if start == 0 || end < start {
        return 0;
    }
    let mut n = 0usize;
    for page in start..=end {
        if let Some(text) = pages.get((page as usize).wrapping_sub(1)) {
            n = n.saturating_add(text.chars().count());
        }
    }
    n
}

/// One reconstructed text line and the font size in effect when it was shown.
pub(crate) struct StyledLine {
    pub text: String,
    pub size: f32,
}

/// Lines from the page content stream, in paint order. Empty when the stream
/// cannot be read; heading detection then has no font-size signal.
pub(crate) fn all_styled_lines(doc: &Document) -> Vec<Vec<(String, f32)>> {
    let Ok(ids) = page_ids(doc) else {
        return Vec::new();
    };
    ids.into_iter()
        .map(|id| styled_lines(doc, id).into_iter().map(|line| (line.text, line.size)).collect())
        .collect()
}

fn styled_lines(doc: &Document, page_id: ObjectId) -> Vec<StyledLine> {
    let Ok(bytes) = doc.get_page_content(page_id) else {
        return Vec::new();
    };
    let Ok(content) = Content::decode(&bytes) else {
        return Vec::new();
    };
    lines_from_ops(&content.operations)
}

struct Pen {
    y: f32,
    size: f32,
    leading: f32,
    in_text: bool,
}

fn lines_from_ops(ops: &[lopdf::content::Operation]) -> Vec<StyledLine> {
    let mut pen = Pen { y: 0.0, size: 12.0, leading: 0.0, in_text: false };
    let mut lines: Vec<StyledLine> = Vec::new();
    let mut buf = String::new();
    let mut buf_size = 12.0f32;
    let flush = |lines: &mut Vec<StyledLine>, buf: &mut String, buf_size: f32| {
        let text = buf.trim().to_string();
        buf.clear();
        if !text.is_empty() {
            lines.push(StyledLine { text, size: buf_size });
        }
    };
    for op in ops {
        match op.operator.as_str() {
            "BT" => {
                flush(&mut lines, &mut buf, buf_size);
                pen.in_text = true;
                pen.y = 0.0;
            }
            "ET" => {
                flush(&mut lines, &mut buf, buf_size);
                pen.in_text = false;
            }
            "Tf" => {
                if let Some(size) = op.operands.get(1).and_then(operand_f32)
                    && size.is_finite()
                    && size.abs() > 0.1
                {
                    pen.size = size.abs();
                }
            }
            "Td" | "TD" => {
                let ty = op.operands.get(1).and_then(operand_f32).unwrap_or(0.0);
                if pen.in_text && ty.abs() > 0.5 {
                    flush(&mut lines, &mut buf, buf_size);
                }
                pen.y += ty;
                if op.operator == "TD" {
                    pen.leading = -ty;
                }
            }
            "Tm" => {
                if pen.in_text {
                    flush(&mut lines, &mut buf, buf_size);
                }
                if let Some(y) = op.operands.get(5).and_then(operand_f32) {
                    pen.y = y;
                }
            }
            "TL" => {
                if let Some(leading) = op.operands.first().and_then(operand_f32) {
                    pen.leading = leading;
                }
            }
            "Tj" | "'" | "\"" => {
                if op.operator != "Tj" {
                    flush(&mut lines, &mut buf, buf_size);
                    pen.y -= pen.leading;
                }
                let operand = if op.operator == "\"" { op.operands.get(2) } else { op.operands.first() };
                if pen.in_text
                    && let Some(text) = operand.and_then(operand_text)
                {
                    push_run(&mut buf, &mut buf_size, &pen, &text);
                }
            }
            "TJ" => {
                if pen.in_text
                    && let Some(array) = op.operands.first().and_then(|obj| obj.as_array().ok())
                {
                    for item in array {
                        if let Some(text) = operand_text(item) {
                            push_run(&mut buf, &mut buf_size, &pen, &text);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut lines, &mut buf, buf_size);
    lines
}

fn push_run(buf: &mut String, buf_size: &mut f32, pen: &Pen, text: &str) {
    if buf.is_empty() {
        *buf_size = pen.size;
    }
    buf.push_str(text);
}

fn operand_f32(obj: &Object) -> Option<f32> {
    obj.as_float().ok()
}

fn operand_text(obj: &Object) -> Option<String> {
    let bytes = obj.as_str().ok()?;
    Some(decode_pdf_text(bytes))
}

fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let units: Vec<u16> =
            bytes[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    bytes.iter().map(|b| *b as char).collect()
}

/// UTC timestamp with second precision, no extra crates.
pub(crate) fn now_rfc3339() -> String {
    let secs =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86_400;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let min = (tod % 3600) / 60;
    let sec = tod % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant's `civil_from_days`, days since 1970-01-01.
pub(crate) fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    }
}
