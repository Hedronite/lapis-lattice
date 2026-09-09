//! Hop-1 neighbors pane (`Space g`): one row per [`Neighbor`] from
//! `GET /neighbors`, direction arrow first, alias / anchor as a dim detail.

use ratatui::text::{Line, Span};

use super::theme;
use crate::lattice::Neighbor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// `◀ ` for inbound, `▶ ` for outbound.
    pub arrow: &'static str,
    pub path: String,
    /// `as alias` / `#anchor` / `unresolved`, joined by ` · `; empty when none.
    pub detail: String,
    pub resolved: bool,
}

pub fn row(n: &Neighbor) -> Row {
    let arrow = if n.direction == "in" { "◀ " } else { "▶ " };
    let mut parts: Vec<String> = Vec::new();
    if let Some(a) = n.alias.as_deref().filter(|a| !a.is_empty()) {
        parts.push(format!("as {a}"));
    }
    if let Some(a) = n.anchor.as_deref().filter(|a| !a.is_empty()) {
        parts.push(format!("#{a}"));
    }
    if !n.resolved {
        parts.push("unresolved".into());
    }
    Row { arrow, path: n.label().to_string(), detail: parts.join(" · "), resolved: n.resolved }
}

pub fn rows(ns: &[Neighbor]) -> Vec<Row> {
    ns.iter().map(row).collect()
}

pub fn line(r: &Row) -> Line<'static> {
    let mut spans = vec![Span::styled(r.arrow, theme::chrome())];
    if r.resolved {
        spans.push(Span::raw(r.path.clone()));
    } else {
        spans.push(Span::styled(r.path.clone(), theme::dim()));
    }
    if !r.detail.is_empty() {
        spans.push(Span::styled(format!("  {}", r.detail), theme::dim()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(path: &str, dir: &str) -> Neighbor {
        Neighbor {
            path: Some(path.into()),
            dst_raw: None,
            alias: None,
            anchor: None,
            resolved: true,
            direction: dir.into(),
        }
    }

    #[test]
    fn neighbor_struct_to_rows() {
        let mut out = n("foundry/lapis/SPEC.md", "out");
        out.alias = Some("the spec".into());
        out.anchor = Some("goals".into());
        let mut inn = n("notes/link.md", "in");
        inn.resolved = false;
        let rows = rows(&[out, inn]);
        assert_eq!(rows[0].arrow, "▶ ");
        assert_eq!(rows[0].path, "foundry/lapis/SPEC.md");
        assert_eq!(rows[0].detail, "as the spec · #goals");
        assert_eq!(rows[1].arrow, "◀ ");
        assert_eq!(rows[1].detail, "unresolved");
        assert!(!rows[1].resolved);
        assert_eq!(line(&rows[0]).to_string(), "▶ foundry/lapis/SPEC.md  as the spec · #goals");
    }

    #[test]
    fn dangling_row_shows_raw_target() {
        let n: Neighbor = serde_json::from_str(
            r#"{"path":null,"dst_raw":"aes_schema_genesis_canon","resolved":0,"dir":"out"}"#,
        )
        .unwrap();
        let r = row(&n);
        assert_eq!(r.path, "aes_schema_genesis_canon");
        assert_eq!(r.detail, "unresolved");
        assert!(!r.resolved);
    }

    #[test]
    fn deserialized_neighbor_row() {
        let n: Neighbor = serde_json::from_str(r#"{"path":"a.md","dir":"in","resolved":1}"#).unwrap();
        let r = row(&n);
        assert_eq!((r.arrow, r.path.as_str(), r.detail.as_str()), ("◀ ", "a.md", ""));
    }
}
