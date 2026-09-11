use std::collections::BTreeMap;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Result;

/// Above this many nodes a snapshot keeps the highest-degree nodes and reports
/// `truncated: true`. Under it there is no cap: a 20k-node vault still draws
/// whole, and the renderer drops labels before it drops nodes.
pub const SNAPSHOT_NODE_CAP: usize = 20_000;

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

/// Whole-vault link graph, the shape of `schema/v0.3/graph-snapshot.schema.json`.
///
/// snake_case on the wire, the same casing as the search family. This is the
/// **global** graph: it is not the hop-ring ego walk, which stays a separate
/// call for local mode.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSnapshot {
    pub generated_at: String,
    pub vault: String,
    pub truncated: bool,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// One indexed document, or one link target that resolves to nothing.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GraphNode {
    /// Vault-relative path, or `dangling:<raw>` when nothing on disk answers.
    pub id: String,
    pub path: Option<String>,
    pub title: String,
    pub domain: Option<String>,
    pub kind: Option<String>,
    /// In + out wikilinks. The canvas sizes a disc from this.
    pub degree: u32,
    pub dangling: bool,
    /// Layout is not the index's job: the engine leaves these empty and the
    /// force sim fills them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GraphEdge {
    pub src: String,
    pub dst: String,
    pub dst_raw: Option<String>,
    pub resolved: bool,
}

fn stem(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".md").unwrap_or(base).to_string()
}

/// Build the whole-vault snapshot from the index alone: one node per document,
/// one per distinct dangling target, one edge per wikilink row.
pub fn snapshot(conn: &Connection, vault: &str, cap: usize) -> Result<GraphSnapshot> {
    let generated_at: String = conn
        .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ','now')", [], |r| r.get(0))
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"));

    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut degree: BTreeMap<String, u32> = BTreeMap::new();
    let mut dangling: BTreeMap<String, String> = BTreeMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT src, dst_path, dst_raw, resolved FROM edges ORDER BY src, dst_raw")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)? != 0,
            ))
        })?;
        for row in rows {
            let (src, dst_path, dst_raw, resolved) = row?;
            // `resolved = 1` always carries a `dst_path`, but an index written by
            // an older build might not, so the id falls back rather than panics.
            let (dst, is_dangling) = match dst_path.filter(|_| resolved) {
                Some(p) => (p, false),
                None => (format!("dangling:{dst_raw}"), true),
            };
            if is_dangling {
                dangling.insert(dst.clone(), dst_raw.clone());
            }
            *degree.entry(src.clone()).or_insert(0) += 1;
            *degree.entry(dst.clone()).or_insert(0) += 1;
            edges.push(GraphEdge { src, dst, dst_raw: Some(dst_raw), resolved: !is_dangling });
        }
    }

    let mut nodes: Vec<GraphNode> = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT path, title, domain, kind FROM documents ORDER BY path")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (path, title, domain, kind) = row?;
            nodes.push(GraphNode {
                title: title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| stem(&path)),
                degree: degree.get(&path).copied().unwrap_or(0),
                id: path.clone(),
                path: Some(path),
                domain,
                kind,
                dangling: false,
                x: None,
                y: None,
            });
        }
    }
    for (id, raw) in dangling {
        nodes.push(GraphNode {
            degree: degree.get(&id).copied().unwrap_or(0),
            title: raw,
            id,
            path: None,
            domain: None,
            kind: None,
            dangling: true,
            x: None,
            y: None,
        });
    }

    let mut truncated = false;
    if cap > 0 && nodes.len() > cap {
        // Keep the busiest nodes: a hub carries the shape of the vault, a leaf
        // carries almost none of it.
        truncated = true;
        nodes.sort_by(|a, b| b.degree.cmp(&a.degree).then_with(|| a.id.cmp(&b.id)));
        nodes.truncate(cap);
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let kept: std::collections::BTreeSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        edges.retain(|e| kept.contains(e.src.as_str()) && kept.contains(e.dst.as_str()));
    }

    Ok(GraphSnapshot { generated_at, vault: vault.to_string(), truncated, nodes, edges })
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
    use std::path::PathBuf;

    /// Unique per call: see the note on the same helper in `lib.rs`. Nanos plus
    /// pid is not enough when tests share a thread pool.
    fn vault() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-lattice-graph-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(d.join("Welcome.md"), "# Welcome\n\nSee [[Alpha]] and [[ghost]].\n").unwrap();
        std::fs::write(d.join("notes/Alpha.md"), "# Alpha\n\nOn to [[Beta]].\n").unwrap();
        std::fs::write(d.join("notes/Beta.md"), "# Beta\n\nLeaf.\n").unwrap();
        // Links to nobody and nobody links it: an ego walk would never reach it,
        // so its presence is what makes this snapshot whole-vault.
        std::fs::write(d.join("Orphan.md"), "# Orphan\n\nAlone.\n").unwrap();
        d
    }

    /// Enough of JSON Schema to check the shipped snapshot against the shipped
    /// schema file: `required`, declared `type` (including nullable unions),
    /// `minimum`, and recursion through `properties` / `items`. Pulling a
    /// validator crate into the engine for one test would cost more than it buys.
    fn validate(
        schema: &serde_json::Value,
        inst: &serde_json::Value,
        at: &str,
    ) -> std::result::Result<(), String> {
        if let Some(t) = schema.get("type") {
            let want: Vec<&str> = match t {
                serde_json::Value::String(s) => vec![s.as_str()],
                serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str()).collect(),
                _ => vec![],
            };
            let ok = want.iter().any(|w| match *w {
                "object" => inst.is_object(),
                "array" => inst.is_array(),
                "string" => inst.is_string(),
                "boolean" => inst.is_boolean(),
                "integer" => inst.is_i64() || inst.is_u64(),
                "number" => inst.is_number(),
                "null" => inst.is_null(),
                _ => true,
            });
            if !ok {
                return Err(format!("{at}: expected {want:?}, got {inst}"));
            }
        }
        if let (Some(min), Some(n)) = (schema.get("minimum").and_then(|m| m.as_f64()), inst.as_f64())
            && n < min
        {
            return Err(format!("{at}: {n} < minimum {min}"));
        }
        if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
            for k in req.iter().filter_map(|k| k.as_str()) {
                if inst.get(k).is_none() {
                    return Err(format!("{at}: missing required `{k}`"));
                }
            }
        }
        if let (Some(props), Some(obj)) =
            (schema.get("properties").and_then(|p| p.as_object()), inst.as_object())
        {
            for (k, sub) in props {
                if let Some(v) = obj.get(k) {
                    validate(sub, v, &format!("{at}.{k}"))?;
                }
            }
        }
        if let (Some(items), Some(arr)) = (schema.get("items"), inst.as_array()) {
            for (i, v) in arr.iter().enumerate() {
                validate(items, v, &format!("{at}[{i}]"))?;
            }
        }
        Ok(())
    }

    #[test]
    fn snapshot_is_whole_vault_and_validates_against_the_schema() {
        let d = vault();
        let mut e = crate::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        let snap = e.graph_snapshot().unwrap();

        let ids: Vec<&str> = snap.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["Orphan.md", "Welcome.md", "notes/Alpha.md", "notes/Beta.md", "dangling:ghost"],
            "every document plus every dangling target, not an ego ring"
        );
        assert!(!snap.truncated);

        let ghost = snap.nodes.iter().find(|n| n.id == "dangling:ghost").unwrap();
        assert!(ghost.dangling, "a link that resolves to nothing is dangling: true");
        assert!(ghost.path.is_none());
        assert_eq!(ghost.title, "ghost");
        assert_eq!(ghost.degree, 1);

        let orphan = snap.nodes.iter().find(|n| n.id == "Orphan.md").unwrap();
        assert_eq!(orphan.degree, 0, "unlinked, and still in the snapshot");
        assert!(!orphan.dangling);

        // degree is in + out
        let by = |id: &str| snap.nodes.iter().find(|n| n.id == id).unwrap().degree;
        assert_eq!(by("Welcome.md"), 2, "out: Alpha, ghost");
        assert_eq!(by("notes/Alpha.md"), 2, "in: Welcome; out: Beta");
        assert_eq!(by("notes/Beta.md"), 1);

        assert_eq!(snap.edges.len(), 3);
        assert!(snap.edges.iter().any(|e| e.src == "Welcome.md" && e.dst == "notes/Alpha.md" && e.resolved));
        let dead = snap.edges.iter().find(|e| e.dst == "dangling:ghost").unwrap();
        assert!(!dead.resolved);
        assert_eq!(dead.dst_raw.as_deref(), Some("ghost"));
        assert!(snap.nodes.iter().all(|n| n.x.is_none() && n.y.is_none()), "layout is the sim's job");

        let text = serde_json::to_string(&snap).unwrap();
        assert!(text.contains("\"generated_at\""), "snake_case, like the search family");
        assert!(!text.contains("generatedAt"));
        assert!(text.contains("\"dangling\":true"));

        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../testdata/graph-snapshot.schema.json")).unwrap();
        let inst: serde_json::Value = serde_json::from_str(&text).unwrap();
        validate(&schema, &inst, "$").unwrap();

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn snapshot_cap_keeps_the_busiest_and_says_so() {
        let d = vault();
        let mut e = crate::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        let snap = snapshot(&e.conn, "v", 2).unwrap();
        assert!(snap.truncated);
        assert_eq!(snap.nodes.len(), 2);
        let ids: Vec<&str> = snap.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["Welcome.md", "notes/Alpha.md"], "degree 2 each, then id order");
        assert!(
            snap.edges.iter().all(|e| ids.contains(&e.src.as_str()) && ids.contains(&e.dst.as_str())),
            "an edge never points at a node the cap dropped"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

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
