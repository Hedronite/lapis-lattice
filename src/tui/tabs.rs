//! A bounded tab strip with the active document always visible.

use ratatui::{style::Style, text::Span};
use std::ops::Range;

pub(crate) struct Slot {
    pub(crate) index: usize,
    pub(crate) cells: Range<u16>,
    pub(crate) label: String,
}

fn shorten(text: &str, width: u16) -> String {
    if Span::raw(text).width() <= width as usize {
        return text.into();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for glyph in Span::raw(text).styled_graphemes(Style::default()) {
        let size = Span::raw(glyph.symbol).width();
        if used + size >= width as usize {
            break;
        }
        out.push_str(glyph.symbol);
        used += size;
    }
    out.push('…');
    out
}

fn names(paths: &[(&str, bool)]) -> Vec<String> {
    paths
        .iter()
        .map(|(path, dirty)| {
            let parts: Vec<_> = path.split('/').collect();
            let mut take = 1;
            let name = loop {
                let suffix = parts[parts.len().saturating_sub(take)..].join("/");
                let duplicate = paths.iter().any(|(other, _)| {
                    other != path && (other == &suffix || other.ends_with(&format!("/{suffix}")))
                });
                if !duplicate || take >= parts.len() {
                    break suffix;
                }
                take += 1;
            };
            format!("{name}{}", if *dirty { " *" } else { "" })
        })
        .collect()
}

pub(crate) fn layout(paths: &[(&str, bool)], active: usize, width: u16) -> Vec<Slot> {
    if paths.is_empty() || width == 0 {
        return vec![];
    }
    let active = active.min(paths.len() - 1);
    let labels: Vec<_> =
        names(paths).iter().map(|s| shorten(s, width.saturating_sub(8).clamp(1, 30))).collect();
    let cost = |i: usize| (Span::raw(&labels[i]).width() + 2) as u16;
    let budget = width.saturating_sub(6);
    if budget < cost(active) {
        return vec![Slot { index: active, cells: 0..width, label: shorten(&labels[active], width) }];
    }
    let mut first = active;
    let mut used = cost(active);
    while first > 0 && used + cost(first - 1) <= budget {
        first -= 1;
        used += cost(first);
    }
    let mut last = active;
    while last + 1 < paths.len() && used + cost(last + 1) <= budget {
        last += 1;
        used += cost(last);
    }
    let mut out = vec![];
    let mut x = 0;
    if first > 0 {
        out.push(Slot { index: first - 1, cells: 0..3, label: " ‹ ".into() });
        x = 3;
    }
    for (index, label) in labels.iter().enumerate().take(last + 1).skip(first) {
        let end = x + cost(index);
        out.push(Slot { index, cells: x..end, label: format!(" {label} ") });
        x = end;
    }
    if last + 1 < paths.len() {
        out.push(Slot { index: last + 1, cells: x..x + 3, label: " › ".into() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_tab_is_visible_and_hit_ranges_match_rendered_cells() {
        let paths: Vec<_> = (0..12).map(|i| format!("folder-{i}/漢字.md")).collect();
        let files: Vec<_> = paths.iter().map(|p| (p.as_str(), true)).collect();
        for width in [8, 40, 80, 120] {
            for active in 0..paths.len() {
                let strip = layout(&files, active, width);
                assert!(strip.iter().any(|s| s.index == active));
                for slot in &strip {
                    assert!(slot.cells.end <= width);
                    assert!(Span::raw(&slot.label).width() <= slot.cells.len());
                }
                assert!(strip.windows(2).all(|p| p[0].cells.end <= p[1].cells.start));
            }
        }
    }

    #[test]
    fn duplicate_names_use_the_shortest_distinguishing_path() {
        assert_eq!(
            names(&[("a/shared/Note.md", false), ("b/shared/Note.md", true), ("a/Other.md", false)]),
            ["a/shared/Note.md", "b/shared/Note.md *", "Other.md"]
        );
        assert_eq!(shorten("漢字🙂", 5), "漢字…");
    }
}
