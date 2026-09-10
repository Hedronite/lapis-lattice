use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Neighbor {
    pub path: Option<String>,
    pub dst_raw: String,
    pub alias: Option<String>,
    pub anchor: Option<String>,
    pub resolved: bool,
    pub dir: String,
}

#[derive(Debug, Clone)]
pub struct WikiLink {
    pub target: String,
    pub alias: Option<String>,
    pub anchor: Option<String>,
}

pub fn parse_wikilinks(body: &str) -> Vec<WikiLink> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("[[") {
        rest = &rest[i + 2..];
        let Some(end) = rest.find("]]") else { break };
        let inner = &rest[..end];
        rest = &rest[end + 2..];
        if inner.is_empty() {
            continue;
        }
        let (rest_t, alias) = match inner.split_once('|') {
            Some((t, a)) => (t, Some(a.trim().to_string()).filter(|s| !s.is_empty())),
            None => (inner, None),
        };
        let (target, anchor) = match rest_t.split_once('#') {
            Some((t, a)) => (t, Some(a.trim().to_string()).filter(|s| !s.is_empty())),
            None => (rest_t, None),
        };
        let target = target.trim().trim_end_matches(".md").trim().to_string();
        if !target.is_empty() {
            out.push(WikiLink { target, alias, anchor });
        }
    }
    out
}

pub fn neighbors(conn: &Connection, path: &str, direction: &str) -> Result<Vec<Neighbor>> {
    let dir = match direction {
        "in" | "out" | "both" => direction,
        _ => "both",
    };
    let mut out = Vec::new();
    if dir == "out" || dir == "both" {
        let mut stmt =
            conn.prepare("SELECT dst_path, dst_raw, alias, anchor, resolved FROM edges WHERE src = ?1")?;
        let rows = stmt.query_map(params![path], |r| {
            Ok(Neighbor {
                path: r.get(0)?,
                dst_raw: r.get(1)?,
                alias: r.get(2)?,
                anchor: r.get(3)?,
                resolved: r.get::<_, i64>(4)? != 0,
                dir: "out".into(),
            })
        })?;
        for n in rows {
            out.push(n?);
        }
    }
    if dir == "in" || dir == "both" {
        let mut stmt =
            conn.prepare("SELECT src, dst_raw, alias, anchor, resolved FROM edges WHERE dst_path = ?1")?;
        let rows = stmt.query_map(params![path], |r| {
            Ok(Neighbor {
                path: Some(r.get(0)?),
                dst_raw: r.get(1)?,
                alias: r.get(2)?,
                anchor: r.get(3)?,
                resolved: r.get::<_, i64>(4)? != 0,
                dir: "in".into(),
            })
        })?;
        for n in rows {
            out.push(n?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alias_and_anchor() {
        let v = parse_wikilinks("See [[Alpha|A]] and [[Welcome#top]] and [[Missing]].");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].target, "Alpha");
        assert_eq!(v[0].alias.as_deref(), Some("A"));
        assert_eq!(v[1].anchor.as_deref(), Some("top"));
        assert_eq!(v[2].target, "Missing");
    }
}
