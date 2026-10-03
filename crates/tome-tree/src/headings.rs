//! Deterministic heading fallback. No model.
//!
//! A heading's level is markdown depth, dotted numbering (`1`, `1.1`,
//! `1.1.1`, at most 8 components), or, when the line has neither, its font
//! size (a larger size is shallower). A child nests under the nearest
//! preceding heading of a shallower level. Depth is capped at 8. An explicit
//! level past that cap, or a chain deeper than 8, is `parse`. Headings that
//! all share one level stay a flat list of roots.

use std::collections::{HashMap, HashSet};

use crate::error::{Result, parse};
use crate::outline::{assign_ends, normalize_title};
use crate::types::{NodeSource, RawNode};

/// Deepest heading level. A markdown or numbered marker past this is `parse`.
/// Font-size ranks past this share the cap instead of failing the book.
pub(crate) const MAX_HEADING_DEPTH: u8 = 8;

/// Headings in `pages[start-1..=end-1]`. Pages are 1-based and inclusive.
/// `sizes[i]` is the content-stream lines for `pages[i]`, used only as a font
/// signal. An empty `Ok` means detection found nothing.
pub(crate) fn detect(
    pages: &[String],
    sizes: &[Vec<(String, f32)>],
    start: u32,
    end: u32,
) -> Result<Vec<RawNode>> {
    if pages.is_empty() || start == 0 || end < start {
        return Ok(Vec::new());
    }
    let start_i = (start as usize) - 1;
    let end_i = (end as usize).min(pages.len());
    if start_i >= end_i {
        return Ok(Vec::new());
    }
    let slice = &pages[start_i..end_i];
    let headers = running_headers(slice);
    let mut hits: Vec<Hit> = Vec::new();
    for (offset, page) in slice.iter().enumerate() {
        let page_no = start + offset as u32;
        let runs = sizes.get(start_i + offset).map(Vec::as_slice).unwrap_or(&[]);
        hits.extend(headings_on_page(page, &headers, runs, page_no)?);
    }
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    assign_font_levels(&mut hits);
    let mut roots = nest(&hits);
    assign_ends(&mut roots, end.min(pages.len() as u32));
    validate(&roots, 1, &mut HashSet::new())?;
    Ok(roots)
}

struct Hit {
    page: u32,
    /// `Some` is markdown, numbering, or `CHAPTER`. `None` waits for font size.
    explicit: Option<u8>,
    size: Option<f32>,
    title: String,
}

fn running_headers(pages: &[String]) -> HashSet<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for page in pages {
        let mut seen = HashSet::new();
        for line in non_empty_lines(page).into_iter().take(3) {
            let key = norm_key(&line);
            if key.chars().count() < 3 || is_page_number(&line) {
                continue;
            }
            if seen.insert(key.clone()) {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    let n = pages.len();
    counts.into_iter().filter(|(_, c)| *c >= 3 && c.saturating_mul(2) >= n).map(|(k, _)| k).collect()
}

fn headings_on_page(
    page: &str,
    headers: &HashSet<String>,
    runs: &[(String, f32)],
    page_no: u32,
) -> Result<Vec<Hit>> {
    let mut top = Vec::new();
    for line in non_empty_lines(page) {
        if headers.contains(&norm_key(&line)) || is_page_number(&line) {
            continue;
        }
        if is_body(&line) {
            break;
        }
        top.push(line);
        if top.len() == 6 {
            break;
        }
    }
    let mut found = Vec::new();
    let mut i = 0;
    while i < top.len() {
        let line = &top[i];
        if let Some(hit) = markdown_heading(line)? {
            found.push(hit_of(page_no, line, runs, Some(hit.0), hit.1));
        } else if let Some(title) = chapter_heading(line) {
            let mut title = title;
            if let Some(next) = top.get(i + 1)
                && chapter_heading(next).is_none()
                && numbered_heading(next)?.is_none()
                && markdown_heading(next)?.is_none()
                && is_titleish(next)
            {
                title = format!("{title} {next}");
                i += 1;
            }
            found.push(hit_of(page_no, line, runs, Some(1), normalize_title(&title)));
        } else if let Some((level, title)) = numbered_heading(line)? {
            found.push(hit_of(page_no, line, runs, Some(level), title));
        } else if found.is_empty() && is_titleish(line) {
            found.push(hit_of(page_no, line, runs, None, normalize_title(line)));
        }
        i += 1;
    }
    Ok(found)
}

fn hit_of(page: u32, line: &str, runs: &[(String, f32)], explicit: Option<u8>, title: String) -> Hit {
    Hit { page, explicit, size: font_size(line, runs), title }
}

fn font_size(line: &str, runs: &[(String, f32)]) -> Option<f32> {
    let key = norm_key(line);
    runs.iter().find(|(text, _)| norm_key(text) == key).map(|(_, size)| *size)
}

/// Rank distinct font sizes among headings that have no explicit level.
/// One size, or no size, leaves those headings at level 1 (a flat list).
fn assign_font_levels(hits: &mut [Hit]) {
    let mut ranks: Vec<i32> = hits
        .iter()
        .filter(|hit| hit.explicit.is_none())
        .filter_map(|hit| hit.size)
        .filter(|size| size.is_finite() && *size > 0.0)
        .map(size_bucket)
        .collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));
    ranks.dedup();
    for hit in hits.iter_mut() {
        let Some(level) = hit.explicit else {
            let level = match hit.size.map(size_bucket) {
                Some(bucket) if ranks.len() > 1 => {
                    let rank = ranks.iter().position(|item| *item == bucket).unwrap_or(0);
                    (rank as u8).saturating_add(1)
                }
                _ => 1,
            };
            hit.explicit = Some(level.min(MAX_HEADING_DEPTH));
            continue;
        };
        hit.explicit = Some(level.clamp(1, MAX_HEADING_DEPTH));
    }
}

fn size_bucket(size: f32) -> i32 {
    (size * 2.0).round() as i32
}

fn nest(hits: &[Hit]) -> Vec<RawNode> {
    let mut roots: Vec<RawNode> = Vec::new();
    let mut stack: Vec<(u8, RawNode)> = Vec::new();
    for hit in hits {
        let level = hit.explicit.unwrap_or(1).clamp(1, MAX_HEADING_DEPTH);
        let node = RawNode {
            title: hit.title.clone(),
            page_start: hit.page,
            page_end: hit.page,
            source: NodeSource::Heading,
            children: Vec::new(),
        };
        while stack.last().is_some_and(|(top, _)| *top >= level) {
            let (_, done) = stack.pop().unwrap();
            attach(&mut roots, &mut stack, done);
        }
        // A child of a level-8 heading would be level 9. Keep it at 8 so the
        // chain cannot grow past the cap; the two share a parent.
        let level = if stack.len() as u8 >= MAX_HEADING_DEPTH { MAX_HEADING_DEPTH } else { level };
        while stack.last().is_some_and(|(top, _)| *top >= level) {
            let (_, done) = stack.pop().unwrap();
            attach(&mut roots, &mut stack, done);
        }
        stack.push((level, node));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut roots, &mut stack, done);
    }
    roots
}

/// `parse` when a chain is deeper than [`MAX_HEADING_DEPTH`] or a heading
/// appears again inside its own ancestor chain.
fn validate(nodes: &[RawNode], depth: u8, ancestors: &mut HashSet<(String, u32)>) -> Result<()> {
    if depth > MAX_HEADING_DEPTH {
        return Err(parse("heading tree deeper than 8"));
    }
    for node in nodes {
        let key = (node.title.clone(), node.page_start);
        if !ancestors.insert(key.clone()) {
            return Err(parse("heading cycle"));
        }
        validate(&node.children, depth.saturating_add(1), ancestors)?;
        ancestors.remove(&key);
    }
    Ok(())
}

fn attach(roots: &mut Vec<RawNode>, stack: &mut [(u8, RawNode)], done: RawNode) {
    if let Some((_, parent)) = stack.last_mut() {
        parent.children.push(done);
    } else {
        roots.push(done);
    }
}

fn chapter_heading(line: &str) -> Option<String> {
    let t = line.trim();
    let mut parts = t.split_whitespace();
    let first = parts.next()?;
    if !first.eq_ignore_ascii_case("chapter") {
        return None;
    }
    let num = parts.next()?.trim_matches(|c: char| matches!(c, ':' | '.' | '-' | '–' | '—' | ')'));
    if !is_number_token(num) && !is_roman(num) {
        return None;
    }
    Some(t.to_string())
}

fn markdown_heading(line: &str) -> Result<Option<(u8, String)>> {
    let t = line.trim();
    let hashes = t.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 {
        return Ok(None);
    }
    let after = &t[hashes..];
    let Some(rest) =
        after.strip_prefix(' ').or_else(|| after.strip_prefix('\t')).map(str::trim).filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    if hashes > MAX_HEADING_DEPTH as usize {
        return Err(parse("heading deeper than 8"));
    }
    if rest.chars().count() > 80 || is_body(rest) {
        return Ok(None);
    }
    Ok(Some((hashes as u8, normalize_title(rest))))
}

fn numbered_heading(line: &str) -> Result<Option<(u8, String)>> {
    let t = line.trim();
    let Some((num, rest)) = t.split_once(char::is_whitespace) else {
        return Ok(None);
    };
    let num = num.trim_end_matches(['.', ':']);
    if num.is_empty() || num.len() > 32 {
        return Ok(None);
    }
    let bits: Vec<&str> = num.split('.').filter(|s| !s.is_empty()).collect();
    if bits.is_empty() || !bits.iter().all(|b| b.chars().all(|c| c.is_ascii_digit())) {
        return Ok(None);
    }
    if bits.len() > MAX_HEADING_DEPTH as usize {
        return Err(parse("heading deeper than 8"));
    }
    // A bare "1" with no dot and a long rest is too easy to confuse with a list.
    // Require either a dotted number (`1.2`) or a short title after a single number.
    let title = rest.trim();
    if title.is_empty() || title.chars().count() > 80 || title.ends_with('.') {
        return Ok(None);
    }
    let Some(first) = title.chars().next() else {
        return Ok(None);
    };
    if !first.is_uppercase() {
        return Ok(None);
    }
    if bits.len() == 1 && title.split_whitespace().count() > 8 {
        return Ok(None);
    }
    let level = bits.len() as u8;
    Ok(Some((level, normalize_title(&format!("{num} {title}")))))
}

fn is_titleish(line: &str) -> bool {
    let t = line.trim();
    let chars = t.chars().count();
    if !(4..=80).contains(&chars) || is_body(t) {
        return false;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.is_empty() || words.len() > 12 {
        return false;
    }
    const SMALL: &[&str] = &["a", "an", "the", "of", "and", "or", "in", "on", "for", "to", "with", "from"];
    let mut significant = 0;
    let mut capped = 0;
    for (i, word) in words.iter().enumerate() {
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
        if bare.is_empty() {
            continue;
        }
        if i > 0 && SMALL.contains(&bare.to_ascii_lowercase().as_str()) {
            continue;
        }
        significant += 1;
        if bare.chars().next().is_some_and(|c| c.is_uppercase()) {
            capped += 1;
        }
    }
    significant > 0 && capped * 2 >= significant
}

fn is_body(line: &str) -> bool {
    let t = line.trim();
    if t.ends_with(['.', '?', '!', ';', ',']) {
        return true;
    }
    t.chars().count() > 90 || t.split_whitespace().count() > 14
}

fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    let bare = lower.strip_prefix("page ").unwrap_or(&lower);
    bare.chars().all(|c| c.is_ascii_digit()) || is_roman(bare)
}

fn is_number_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn is_roman(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    !t.is_empty() && t.chars().all(|c| matches!(c, 'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm')) && t.len() <= 8
}

fn non_empty_lines(page: &str) -> Vec<String> {
    page.lines().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn norm_key(line: &str) -> String {
    normalize_title(line).to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn chapter_and_numbered_nest_and_running_header_is_dropped() {
        let pages = vec![
            "LATTICE HANDBOOK\nCHAPTER 1\nIntroduction\nThis chapter introduces the lattice index.".into(),
            "LATTICE HANDBOOK\nThe index uses fts5 for lexical search over markdown files.".into(),
            "LATTICE HANDBOOK\nCHAPTER 2\nStorage\nPages live beside the sqlite file.".into(),
            "LATTICE HANDBOOK\n1.1 Page records\nEach page is one jsonl row.".into(),
        ];
        let tree = detect(&pages, &[], 1, 4).unwrap();
        assert_eq!(tree.len(), 2, "{tree:?}");
        assert_eq!(tree[0].title, "CHAPTER 1 Introduction");
        assert_eq!(tree[0].page_start, 1);
        assert_eq!(tree[0].page_end, 2);
        assert!(tree[0].children.is_empty());
        assert_eq!(tree[1].title, "CHAPTER 2 Storage");
        assert_eq!(tree[1].page_start, 3);
        assert_eq!(tree[1].page_end, 4);
        assert_eq!(tree[1].children.len(), 1);
        assert_eq!(tree[1].children[0].title, "1.1 Page records");
        assert_eq!(tree[1].children[0].page_start, 4);
        assert!(tree.iter().all(|n| n.source == NodeSource::Heading));
    }

    #[test]
    fn prose_without_a_heading_is_empty() {
        let pages = vec![
            "the quick brown fox jumps over the lazy dog and keeps talking about nothing.".into(),
            "another lowercase sentence follows and still does not look like a title.".into(),
        ];
        assert!(detect(&pages, &[], 1, 2).unwrap().is_empty());
    }

    #[test]
    fn numbered_levels_nest_three_deep() {
        let pages = vec![
            "1 Foundations\nThe part introduces the index.".into(),
            "1.1 Storage\nPages live beside the sqlite file.".into(),
            "1.1.1 Page records\nEach page is one jsonl row.".into(),
        ];
        let tree = detect(&pages, &[], 1, 3).unwrap();
        assert_eq!(tree.len(), 1, "{tree:?}");
        assert_eq!(tree[0].title, "1 Foundations");
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].title, "1.1 Storage");
        assert_eq!(tree[0].children[0].children.len(), 1);
        assert_eq!(tree[0].children[0].children[0].title, "1.1.1 Page records");
    }

    #[test]
    fn font_size_nests_and_one_size_stays_flat() {
        let pages = vec![
            "Part One\nOpening prose of the part.".into(),
            "Section Two\nOpening prose of the section.".into(),
            "Detail Three\nOpening prose of the detail.".into(),
        ];
        let sizes = vec![
            vec![("Part One".into(), 22.0)],
            vec![("Section Two".into(), 16.0)],
            vec![("Detail Three".into(), 12.0)],
        ];
        let tree = detect(&pages, &sizes, 1, 3).unwrap();
        assert_eq!(tree.len(), 1, "{tree:?}");
        assert_eq!(tree[0].title, "Part One");
        assert_eq!(tree[0].children[0].title, "Section Two");
        assert_eq!(tree[0].children[0].children[0].title, "Detail Three");

        let flat = detect(&pages, &[], 1, 3).unwrap();
        assert_eq!(flat.len(), 3, "{flat:?}");
        assert!(flat.iter().all(|node| node.children.is_empty()));
    }

    #[test]
    fn markdown_nests_and_a_ninth_hash_is_parse() {
        let pages = vec!["# Part\n## Section\n### Detail\nBody follows the headings.".into()];
        let tree = detect(&pages, &[], 1, 1).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children[0].children[0].title, "Detail");
        let err = detect(&["######### Too deep\nShort.".into()], &[], 1, 1).unwrap_err();
        assert_eq!(err.code(), "parse");
    }

    #[test]
    fn a_chain_deeper_than_eight_is_parse() {
        let mut node = RawNode {
            title: "L9".into(),
            page_start: 1,
            page_end: 1,
            source: NodeSource::Heading,
            children: Vec::new(),
        };
        for i in (1..9).rev() {
            node = RawNode {
                title: format!("L{i}"),
                page_start: 1,
                page_end: 1,
                source: NodeSource::Heading,
                children: vec![node],
            };
        }
        let err = validate(&[node], 1, &mut HashSet::new()).unwrap_err();
        assert!(err.to_string().contains("deeper than 8"), "{err}");
    }
}
