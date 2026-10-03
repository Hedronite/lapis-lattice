//! Split leaves that run longer than 10 pages, ~20k tokens, or the open byte cap.

use crate::headings::detect;
use crate::outline::normalize_title;
use crate::pdf::char_count;
use crate::types::{NodeSource, OPEN_BYTE_CAP, RawNode, SPLIT_PAGES, SPLIT_TOKENS};

const MAX_DEPTH: u32 = 64;

pub(crate) fn split_all(nodes: &mut [RawNode], pages: &[String]) -> crate::error::Result<()> {
    for node in nodes {
        split_node(node, pages, 0)?;
    }
    Ok(())
}

fn split_node(node: &mut RawNode, pages: &[String], depth: u32) -> crate::error::Result<()> {
    if depth > MAX_DEPTH {
        return Err(crate::error::parse("split deeper than 64"));
    }
    for child in &mut node.children {
        split_node(child, pages, depth + 1)?;
    }
    if !node.children.is_empty() {
        return Ok(());
    }
    let span = node.page_end.saturating_sub(node.page_start).saturating_add(1);
    let tokens = char_count(pages, node.page_start, node.page_end) / 4;
    let bytes = byte_count(pages, node.page_start, node.page_end);
    if span <= SPLIT_PAGES && tokens <= SPLIT_TOKENS && bytes <= OPEN_BYTE_CAP {
        return Ok(());
    }
    let subs = subheadings(node, pages)?;
    if !subs.is_empty() {
        node.children = subs;
        for child in &mut node.children {
            // A child that still covers the whole node cannot be split again.
            if child.page_start == node.page_start && child.page_end == node.page_end {
                continue;
            }
            split_node(child, pages, depth + 1)?;
        }
        return Ok(());
    }
    node.children = windows_under(node, pages);
    Ok(())
}

fn subheadings(node: &RawNode, pages: &[String]) -> crate::error::Result<Vec<RawNode>> {
    let mut found = detect(pages, &[], node.page_start, node.page_end)?;
    let own = normalize_title(&node.title);
    found.retain(|h| !(h.page_start == node.page_start && normalize_title(&h.title) == own));
    if found.is_empty() {
        return Ok(Vec::new());
    }
    // A heading that only restates the start of the node does not divide it.
    if found.len() == 1 && found[0].page_start == node.page_start {
        return Ok(Vec::new());
    }
    Ok(found)
}

fn windows_under(node: &RawNode, pages: &[String]) -> Vec<RawNode> {
    let mut children = Vec::new();
    let mut start = node.page_start;
    while start <= node.page_end {
        let mut end = (start + SPLIT_PAGES - 1).min(node.page_end);
        while end > start
            && (char_count(pages, start, end) / 4 > SPLIT_TOKENS
                || byte_count(pages, start, end) > OPEN_BYTE_CAP)
        {
            end -= 1;
        }
        children.push(RawNode {
            title: format!("{} (pp. {start}–{end})", node.title),
            page_start: start,
            page_end: end,
            source: NodeSource::Window,
            children: Vec::new(),
        });
        start = end + 1;
    }
    children
}

fn byte_count(pages: &[String], start: u32, end: u32) -> usize {
    if start == 0 || end < start {
        return 0;
    }
    let mut n = 0usize;
    for page in start..=end {
        if let Some(text) = pages.get((page as usize).wrapping_sub(1)) {
            n = n.saturating_add(text.len());
        }
    }
    n
}

/// Explicit page-window tree. `split_all` divides it when the span is oversized.
pub(crate) fn window_tree(page_count: u32, title: &str) -> Vec<RawNode> {
    if page_count == 0 {
        return Vec::new();
    }
    vec![RawNode {
        title: title.to_string(),
        page_start: 1,
        page_end: page_count,
        source: NodeSource::Window,
        children: Vec::new(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(start: u32, end: u32) -> RawNode {
        RawNode {
            title: "Chapter".into(),
            page_start: start,
            page_end: end,
            source: NodeSource::Outline,
            children: Vec::new(),
        }
    }

    #[test]
    fn long_leaf_becomes_page_windows() {
        let pages = vec!["not a heading, just a sentence about the page.".into(); 25];
        let mut nodes = vec![leaf(1, 25)];
        split_all(&mut nodes, &pages).unwrap();
        assert_eq!(nodes[0].source, NodeSource::Outline);
        assert_eq!(nodes[0].children.len(), 3);
        assert_eq!(nodes[0].children[0].page_start, 1);
        assert_eq!(nodes[0].children[0].page_end, 10);
        assert_eq!(nodes[0].children[0].source, NodeSource::Window);
        assert!(nodes[0].children[0].title.contains("(pp. 1–10)"));
        assert_eq!(nodes[0].children[2].page_start, 21);
        assert_eq!(nodes[0].children[2].page_end, 25);
    }

    #[test]
    fn token_budget_shrinks_the_window() {
        let big = "a".repeat(80_000);
        let pages = vec![big, "short".into(), "short".into()];
        let mut nodes = vec![leaf(1, 3)];
        split_all(&mut nodes, &pages).unwrap();
        assert!(nodes[0].children.len() >= 2);
        assert_eq!(nodes[0].children[0].page_start, 1);
        assert_eq!(nodes[0].children[0].page_end, 1);
    }

    #[test]
    fn byte_cap_splits_a_dense_leaf() {
        let page = "d".repeat(6_000);
        let pages = vec![page; 10];
        let mut nodes = vec![leaf(1, 10)];
        split_all(&mut nodes, &pages).unwrap();
        assert!(nodes[0].children.len() >= 2, "10 dense pages must split under the open byte cap");
        for child in &nodes[0].children {
            assert!(byte_count(&pages, child.page_start, child.page_end) <= OPEN_BYTE_CAP);
        }
    }
}
