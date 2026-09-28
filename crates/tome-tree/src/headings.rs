//! Deterministic heading fallback. No model.

use std::collections::{HashMap, HashSet};

use crate::outline::{assign_ends, normalize_title};
use crate::types::{NodeSource, RawNode};

/// Headings in `pages[start-1..=end-1]`. Pages are 1-based and inclusive.
/// An empty vec means detection failed; the caller decides fail-closed vs windows.
pub(crate) fn detect(pages: &[String], start: u32, end: u32) -> Vec<RawNode> {
    if pages.is_empty() || start == 0 || end < start {
        return Vec::new();
    }
    let start_i = (start as usize) - 1;
    let end_i = (end as usize).min(pages.len());
    if start_i >= end_i {
        return Vec::new();
    }
    let slice = &pages[start_i..end_i];
    let headers = running_headers(slice);
    let mut flat: Vec<(u32, u8, String)> = Vec::new();
    for (offset, page) in slice.iter().enumerate() {
        let page_no = start + offset as u32;
        for (level, title) in headings_on_page(page, &headers) {
            flat.push((page_no, level, title));
        }
    }
    if flat.is_empty() {
        return Vec::new();
    }
    let mut roots = nest(&flat);
    assign_ends(&mut roots, end.min(pages.len() as u32));
    roots
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

fn headings_on_page(page: &str, headers: &HashSet<String>) -> Vec<(u8, String)> {
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
        if let Some(title) = chapter_heading(line) {
            let mut title = title;
            if let Some(next) = top.get(i + 1)
                && chapter_heading(next).is_none()
                && numbered_heading(next).is_none()
                && is_titleish(next)
            {
                title = format!("{title} {next}");
                i += 1;
            }
            found.push((1, normalize_title(&title)));
        } else if let Some((level, title)) = numbered_heading(line) {
            found.push((level, title));
        } else if found.is_empty() && is_titleish(line) {
            found.push((1, normalize_title(line)));
        }
        i += 1;
    }
    found
}

fn nest(flat: &[(u32, u8, String)]) -> Vec<RawNode> {
    let mut roots: Vec<RawNode> = Vec::new();
    let mut stack: Vec<(u8, RawNode)> = Vec::new();
    for (page, level, title) in flat {
        let node = RawNode {
            title: title.clone(),
            page_start: *page,
            page_end: *page,
            source: NodeSource::Heading,
            children: Vec::new(),
        };
        while stack.last().is_some_and(|(top, _)| *top >= *level) {
            let (_, done) = stack.pop().unwrap();
            attach(&mut roots, &mut stack, done);
        }
        stack.push((*level, node));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut roots, &mut stack, done);
    }
    roots
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

fn numbered_heading(line: &str) -> Option<(u8, String)> {
    let t = line.trim();
    let (num, rest) = t.split_once(char::is_whitespace)?;
    let num = num.trim_end_matches(['.', ':']);
    if num.is_empty() || num.len() > 16 {
        return None;
    }
    let bits: Vec<&str> = num.split('.').filter(|s| !s.is_empty()).collect();
    if bits.is_empty() || bits.len() > 4 || !bits.iter().all(|b| b.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    // A bare "1" with no dot and a long rest is too easy to confuse with a list.
    // Require either a dotted number (`1.2`) or a short title after a single number.
    let title = rest.trim();
    if title.is_empty() || title.chars().count() > 80 || title.ends_with('.') {
        return None;
    }
    let first = title.chars().next()?;
    if !first.is_uppercase() {
        return None;
    }
    if bits.len() == 1 && title.split_whitespace().count() > 8 {
        return None;
    }
    let level = bits.len() as u8;
    Some((level, normalize_title(&format!("{num} {title}"))))
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

    #[test]
    fn chapter_and_numbered_nest_and_running_header_is_dropped() {
        let pages = vec![
            "LATTICE HANDBOOK\nCHAPTER 1\nIntroduction\nThis chapter introduces the lattice index.".into(),
            "LATTICE HANDBOOK\nThe index uses fts5 for lexical search over markdown files.".into(),
            "LATTICE HANDBOOK\nCHAPTER 2\nStorage\nPages live beside the sqlite file.".into(),
            "LATTICE HANDBOOK\n1.1 Page records\nEach page is one jsonl row.".into(),
        ];
        let tree = detect(&pages, 1, 4);
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
        assert!(detect(&pages, 1, 2).is_empty());
    }
}
