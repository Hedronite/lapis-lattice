//! Split leaves that run longer than 10 pages or ~20k tokens.

use crate::headings::detect;
use crate::outline::normalize_title;
use crate::pdf::char_count;
use crate::types::{NodeSource, RawNode, SPLIT_PAGES, SPLIT_TOKENS};

pub(crate) fn split_all(nodes: &mut [RawNode], pages: &[String]) {
    for node in nodes {
        split_node(node, pages);
    }
}

fn split_node(node: &mut RawNode, pages: &[String]) {
    for child in &mut node.children {
        split_node(child, pages);
    }
    if !node.children.is_empty() {
        return;
    }
    let span = node.page_end.saturating_sub(node.page_start).saturating_add(1);
    let tokens = char_count(pages, node.page_start, node.page_end) / 4;
    if span <= SPLIT_PAGES && tokens <= SPLIT_TOKENS {
        return;
    }
    let subs = subheadings(node, pages);
    if !subs.is_empty() {
        node.children = subs;
        for child in &mut node.children {
            split_node(child, pages);
        }
        return;
    }
    node.children = windows_under(node, pages);
}

fn subheadings(node: &RawNode, pages: &[String]) -> Vec<RawNode> {
    let mut found = detect(pages, node.page_start, node.page_end);
    let own = normalize_title(&node.title);
    found.retain(|h| !(h.page_start == node.page_start && normalize_title(&h.title) == own));
    if found.is_empty() {
        return Vec::new();
    }
    // A heading that only restates the start of the node does not divide it.
    if found.len() == 1 && found[0].page_start == node.page_start {
        return Vec::new();
    }
    found
}

fn windows_under(node: &RawNode, pages: &[String]) -> Vec<RawNode> {
    let mut children = Vec::new();
    let mut start = node.page_start;
    while start <= node.page_end {
        let mut end = (start + SPLIT_PAGES - 1).min(node.page_end);
        while end > start && char_count(pages, start, end) / 4 > SPLIT_TOKENS {
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
        split_all(&mut nodes, &pages);
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
        split_all(&mut nodes, &pages);
        assert!(nodes[0].children.len() >= 2);
        assert_eq!(nodes[0].children[0].page_start, 1);
        assert_eq!(nodes[0].children[0].page_end, 1);
    }
}
