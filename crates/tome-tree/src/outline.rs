//! Walk `/Outlines` and resolve each destination to a 1-based physical page.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::error::{Result, parse};
use crate::types::{NodeSource, RawNode};

/// `Ok(None)` means the file has no outline. `Err` means an outline was present
/// and could not be read — that is a parse failure, not "no structure".
pub(crate) fn extract(doc: &Document) -> Result<Option<Vec<RawNode>>> {
    let catalog = doc.catalog().map_err(parse)?;
    let Some(outlines_obj) = opt(catalog, b"Outlines") else {
        return Ok(None);
    };
    let outlines = deref_dict(doc, outlines_obj)?;
    let Some(first) = opt(&outlines, b"First").cloned() else {
        return Ok(None);
    };
    let page_of = page_index(doc);
    let page_count = page_of.len() as u32;
    if page_count == 0 {
        return Err(parse("pdf has no pages"));
    }
    let named = named_destinations(doc)?;
    let mut seen = HashSet::new();
    let nodes = siblings(doc, &first, &page_of, &named, page_count, &mut seen, 0)?;
    if nodes.is_empty() { Ok(None) } else { Ok(Some(nodes)) }
}

const MAX_DEPTH: u32 = 64;

/// Drop Cover / Contents front-matter and consecutive duplicate titles.
pub(crate) fn drop_front_matter(nodes: Vec<RawNode>) -> Vec<RawNode> {
    drop_front_matter_at(nodes, 0)
}

fn drop_front_matter_at(nodes: Vec<RawNode>, depth: u32) -> Vec<RawNode> {
    if depth > MAX_DEPTH {
        return nodes;
    }
    let mut out = Vec::new();
    for mut node in nodes {
        node.title = normalize_title(&node.title);
        node.children = drop_front_matter_at(node.children, depth + 1);
        if is_front_matter(&node.title) {
            out.extend(node.children);
            continue;
        }
        if node.title.is_empty() {
            node.title = "Untitled".to_string();
        }
        let dup = out
            .last()
            .is_some_and(|prev: &RawNode| prev.title == node.title && prev.page_start == node.page_start);
        if dup {
            continue;
        }
        out.push(node);
    }
    out
}

/// Fill `page_end` from the next sibling that starts later, capped by the parent.
pub(crate) fn assign_ends(nodes: &mut [RawNode], parent_end: u32) {
    assign_ends_at(nodes, parent_end, 0);
}

fn assign_ends_at(nodes: &mut [RawNode], parent_end: u32, depth: u32) {
    if depth > MAX_DEPTH {
        return;
    }
    for i in 0..nodes.len() {
        let start = nodes[i].page_start.min(parent_end).max(1);
        nodes[i].page_start = start;
        let mut end = parent_end.max(start);
        for other in nodes.iter().skip(i + 1) {
            if other.page_start > start {
                end = end.min(other.page_start - 1);
            }
        }
        if end < start {
            end = start;
        }
        nodes[i].page_end = end;
        assign_ends_at(&mut nodes[i].children, end, depth + 1);
    }
}

fn siblings(
    doc: &Document,
    first: &Object,
    page_of: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Object>,
    page_count: u32,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) -> Result<Vec<RawNode>> {
    if depth > 256 {
        return Ok(Vec::new());
    }
    let mut nodes = Vec::new();
    let mut current = Some(first.clone());
    let mut guard = 0u32;
    while let Some(obj) = current.take() {
        guard += 1;
        if guard > 10_000 {
            return Err(parse("outline longer than 10000 items"));
        }
        if let Object::Reference(id) = obj
            && !seen.insert(id)
        {
            return Err(parse("outline cycle"));
        }
        let dict = deref_dict(doc, &obj)?;
        nodes.extend(one_item(doc, &dict, page_of, named, page_count, seen, depth)?);
        current = opt(&dict, b"Next").cloned();
    }
    Ok(nodes)
}

fn one_item(
    doc: &Document,
    dict: &Dictionary,
    page_of: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Object>,
    page_count: u32,
    seen: &mut HashSet<ObjectId>,
    depth: u32,
) -> Result<Vec<RawNode>> {
    if is_remote(doc, dict)? {
        return Ok(Vec::new());
    }
    let children = if let Some(first) = opt(dict, b"First") {
        siblings(doc, first, page_of, named, page_count, seen, depth + 1)?
    } else {
        Vec::new()
    };
    // No destination, or one we cannot resolve: drop this item and keep its
    // children. One broken bookmark must not fail the document.
    let page = match dest_page(doc, dict, page_of, named, page_count) {
        Ok(Some(page)) => page,
        Ok(None) | Err(_) => return Ok(children),
    };
    let title = dict
        .get(b"Title")
        .ok()
        .and_then(|obj| lopdf::decode_text_string(obj).ok())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Untitled".to_string());
    Ok(vec![RawNode { title, page_start: page, page_end: page, source: NodeSource::Outline, children }])
}

fn is_remote(doc: &Document, dict: &Dictionary) -> Result<bool> {
    let Some(action) = opt(dict, b"A") else {
        return Ok(false);
    };
    let act = deref_dict(doc, action)?;
    let kind = opt(&act, b"S").and_then(|o| o.as_name().ok()).unwrap_or(b"");
    Ok(kind == b"GoToR")
}

fn dest_page(
    doc: &Document,
    item: &Dictionary,
    page_of: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Object>,
    page_count: u32,
) -> Result<Option<u32>> {
    if let Some(dest) = opt(item, b"Dest") {
        return resolve_dest(doc, dest, page_of, named, page_count, 0).map(Some);
    }
    if let Some(action) = opt(item, b"A") {
        let act = deref_dict(doc, action)?;
        if let Some(dest) = opt(&act, b"D") {
            return resolve_dest(doc, dest, page_of, named, page_count, 0).map(Some);
        }
    }
    Ok(None)
}

fn resolve_dest(
    doc: &Document,
    dest: &Object,
    page_of: &HashMap<ObjectId, u32>,
    named: &HashMap<Vec<u8>, Object>,
    page_count: u32,
    hops: u8,
) -> Result<u32> {
    if hops > 8 {
        return Err(parse("destination chain too long"));
    }
    let owned = match dest {
        Object::Reference(id) => doc.get_object(*id).map_err(parse)?.clone(),
        other => other.clone(),
    };
    match owned {
        Object::Array(arr) => {
            let first = arr.first().ok_or_else(|| parse("empty destination"))?;
            page_from_target(first, page_of, page_count)
        }
        Object::Dictionary(dict) => {
            if let Some(inner) = opt(&dict, b"D") {
                return resolve_dest(doc, inner, page_of, named, page_count, hops + 1);
            }
            Err(parse("destination dictionary has no /D"))
        }
        Object::Name(key) | Object::String(key, _) => {
            let inner = named.get(&key).ok_or_else(|| parse("named destination not found"))?;
            resolve_dest(doc, inner, page_of, named, page_count, hops + 1)
        }
        Object::Reference(_) => Err(parse("unresolved destination reference")),
        _ => Err(parse("unsupported destination")),
    }
}

fn page_from_target(obj: &Object, page_of: &HashMap<ObjectId, u32>, page_count: u32) -> Result<u32> {
    match obj {
        Object::Reference(id) => page_of
            .get(id)
            .copied()
            .ok_or_else(|| parse(format!("destination page {id:?} is not in the page tree"))),
        Object::Integer(n) => {
            // Explicit destinations use a page reference. Some files store a
            // 0-based page index instead. `0` is page 1; any other in-range
            // integer is 0-based when it is below the page count.
            if *n == 0 {
                return Ok(1);
            }
            if *n > 0 && (*n as u32) < page_count {
                return Ok(*n as u32 + 1);
            }
            if *n > 0 && (*n as u32) <= page_count {
                return Ok(*n as u32);
            }
            Err(parse(format!("destination page index {n} out of range")))
        }
        _ => Err(parse("destination does not point at a page")),
    }
}

fn page_index(doc: &Document) -> HashMap<ObjectId, u32> {
    doc.get_pages().into_iter().map(|(n, id)| (id, n)).collect()
}

fn named_destinations(doc: &Document) -> Result<HashMap<Vec<u8>, Object>> {
    let mut out = HashMap::new();
    let catalog = doc.catalog().map_err(parse)?;
    if let Some(dests) = opt(catalog, b"Dests") {
        let dict = deref_dict(doc, dests)?;
        collect_name_dict(doc, &dict, &mut out)?;
    }
    if let Some(names) = opt(catalog, b"Names").and_then(|obj| deref_dict(doc, obj).ok())
        && let Some(dests) = opt(&names, b"Dests").cloned()
    {
        match dests {
            Object::Reference(id) => {
                let mut seen = HashSet::new();
                collect_name_tree(doc, id, &mut out, &mut seen, 0)?;
            }
            other => {
                let dict = deref_dict(doc, &other)?;
                if let Some(kids) = opt(&dict, b"Kids").and_then(|o| o.as_array().ok()).map(|k| k.to_vec()) {
                    let mut seen = HashSet::new();
                    for kid in kids {
                        let Ok(kid_id) = kid.as_reference() else {
                            continue;
                        };
                        collect_name_tree(doc, kid_id, &mut out, &mut seen, 1)?;
                    }
                }
                let dict = deref_dict(doc, &other)?;
                if let Some(pairs) = opt(&dict, b"Names").and_then(|o| o.as_array().ok()) {
                    collect_name_pairs(doc, pairs, &mut out)?;
                }
            }
        }
    }
    Ok(out)
}

fn collect_name_dict(doc: &Document, dict: &Dictionary, out: &mut HashMap<Vec<u8>, Object>) -> Result<()> {
    for (key, value) in dict.iter() {
        if key == b"Type" || key == b"Limits" || key == b"Kids" || key == b"Names" {
            continue;
        }
        let resolved = match value {
            Object::Reference(id) => doc.get_object(*id).map_err(parse)?.clone(),
            other => other.clone(),
        };
        out.insert(key.clone(), resolved);
    }
    Ok(())
}

fn collect_name_tree(
    doc: &Document,
    id: ObjectId,
    out: &mut HashMap<Vec<u8>, Object>,
    seen: &mut HashSet<ObjectId>,
    depth: u8,
) -> Result<()> {
    if !seen.insert(id) || depth > 32 {
        return Ok(());
    }
    let kids = {
        let Ok(node) = doc.get_dictionary(id) else {
            return Ok(());
        };
        opt(node, b"Kids").and_then(|o| o.as_array().ok()).map(|kids| kids.to_vec()).unwrap_or_default()
    };
    for kid in kids {
        let Ok(kid_id) = kid.as_reference() else {
            continue;
        };
        collect_name_tree(doc, kid_id, out, seen, depth + 1)?;
    }
    let names = {
        let Ok(node) = doc.get_dictionary(id) else {
            return Ok(());
        };
        opt(node, b"Names").and_then(|o| o.as_array().ok()).map(|names| names.to_vec())
    };
    if let Some(names) = names {
        collect_name_pairs(doc, &names, out)?;
    }
    Ok(())
}

fn collect_name_pairs(doc: &Document, names: &[Object], out: &mut HashMap<Vec<u8>, Object>) -> Result<()> {
    let mut it = names.iter();
    while let Some(key) = it.next() {
        let Some(val) = it.next() else {
            break;
        };
        let key = key.as_str().map_err(parse)?.to_vec();
        let resolved = match val {
            Object::Reference(id) => doc.get_object(*id).map_err(parse)?.clone(),
            other => other.clone(),
        };
        out.insert(key, resolved);
    }
    Ok(())
}

fn opt<'a>(dict: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    dict.get(key).ok()
}

fn deref_dict(doc: &Document, obj: &Object) -> Result<Dictionary> {
    match obj {
        Object::Dictionary(d) => Ok(d.clone()),
        Object::Reference(id) => doc.get_dictionary(*id).cloned().map_err(parse),
        _ => Err(parse("expected a dictionary")),
    }
}

pub(crate) fn normalize_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_front_matter(title: &str) -> bool {
    matches!(
        title.to_ascii_lowercase().as_str(),
        "table of contents" | "contents" | "cover" | "title page" | "toc"
    )
}
