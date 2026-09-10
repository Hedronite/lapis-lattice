//! Graph canvas data layer: `/graph/ego` JSON → a scene the renderer can draw.
//!
//! Layout is deterministic and dependency-free: the seed at the origin, depth-1
//! nodes on a ring, depth-2 nodes on a wider ring clustered near their `via`
//! parent. Positions are in canvas units (the seed ring radius is 1.0), so the
//! view scales them. `draw()` is the paint list the gpui-omarchy window consumes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One row of `GET /graph/ego`, as the lattice sends it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct EgoRow {
    pub depth: u32,
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub dst_raw: Option<String>,
    #[serde(default, deserialize_with = "int_or_bool")]
    pub resolved: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Ego {
    pub path: String,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub hops: u32,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub rows: Vec<EgoRow>,
}

fn int_or_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Bool(b) => b,
        serde_json::Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    /// Short label (file stem).
    pub label: String,
    pub depth: u32,
    pub x: f32,
    pub y: f32,
    pub dangling: bool,
    pub is_seed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    /// `out` = from → to in the vault; `in` = the neighbor links to us.
    pub dir: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Scene {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub truncated: bool,
}

fn stem(p: &str) -> String {
    let base = p.rsplit('/').next().unwrap_or(p);
    base.strip_suffix(".md").unwrap_or(base).to_string()
}

impl Node {
    /// First path segment (`notes/Alpha.md` → `notes`). Files at the vault root
    /// have no domain.
    pub fn domain(&self) -> Option<&str> {
        self.id.split_once('/').map(|(d, _)| d)
    }

    /// Gate C filters: seed always stays; dangling can be hidden; one domain at
    /// a time (or all).
    pub fn passes(&self, show_dangling: bool, domain: Option<&str>) -> bool {
        if self.is_seed {
            return true;
        }
        if self.dangling && !show_dangling {
            return false;
        }
        match domain {
            None => true,
            Some(d) => self.domain() == Some(d),
        }
    }
}

impl Scene {
    pub fn seed(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.is_seed)
    }
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.id == id)
    }

    /// Unique first-path-segment domains in the scene, sorted.
    pub fn domains(&self) -> Vec<String> {
        let mut d: Vec<String> = self.nodes.iter().filter_map(|n| n.domain().map(str::to_string)).collect();
        d.sort();
        d.dedup();
        d
    }

    /// Sub-scene after Gate C filters. Edge endpoints that did not survive are dropped.
    pub fn filtered(&self, show_dangling: bool, domain: Option<&str>) -> Scene {
        let mut remap = vec![None; self.nodes.len()];
        let mut nodes = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.passes(show_dangling, domain) {
                remap[i] = Some(nodes.len());
                nodes.push(n.clone());
            }
        }
        let edges = self
            .edges
            .iter()
            .filter_map(|e| Some(Edge { from: remap[e.from]?, to: remap[e.to]?, dir: e.dir.clone() }))
            .collect();
        Scene { nodes, edges, truncated: self.truncated }
    }
}

/// Build a scene from ego JSON. Duplicate targets collapse to one node; edges
/// keep every row. Depth-1 nodes sit on a unit ring; depth-2 nodes at radius 2
/// spread ±35° around their parent's angle.
pub fn build(ego: &Ego) -> Scene {
    let mut scene = Scene { truncated: ego.truncated, ..Default::default() };
    scene.nodes.push(Node {
        id: ego.path.clone(),
        label: stem(&ego.path),
        depth: 0,
        x: 0.0,
        y: 0.0,
        dangling: false,
        is_seed: true,
    });
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    index.insert(ego.path.clone(), 0);

    let node_id = |r: &EgoRow| -> String {
        r.path
            .clone()
            .or_else(|| r.dst_raw.clone().map(|d| format!("dangling:{d}")))
            .unwrap_or_else(|| "?".into())
    };

    // depth-1 ring
    let d1: Vec<&EgoRow> = ego.rows.iter().filter(|r| r.depth == 1).collect();
    let n1 = d1.len().max(1) as f32;
    let mut angle_of: BTreeMap<String, f32> = BTreeMap::new();
    for (i, r) in d1.iter().enumerate() {
        let id = node_id(r);
        let a = std::f32::consts::TAU * i as f32 / n1;
        if !index.contains_key(&id) {
            index.insert(id.clone(), scene.nodes.len());
            scene.nodes.push(Node {
                label: stem(r.path.as_deref().or(r.dst_raw.as_deref()).unwrap_or("?")),
                id: id.clone(),
                depth: 1,
                x: a.cos(),
                y: a.sin(),
                dangling: !r.resolved,
                is_seed: false,
            });
            angle_of.insert(id.clone(), a);
        }
        scene.edges.push(Edge { from: 0, to: index[&id], dir: r.dir.clone() });
    }
    // depth-2: cluster around the parent's angle
    let mut per_parent: BTreeMap<String, Vec<&EgoRow>> = BTreeMap::new();
    for r in ego.rows.iter().filter(|r| r.depth == 2) {
        per_parent.entry(r.via.clone().unwrap_or_default()).or_default().push(r);
    }
    for (via, rows) in per_parent {
        let Some(&pi) = index.get(&via) else { continue };
        let base = angle_of.get(&via).copied().unwrap_or(0.0);
        let k = rows.len() as f32;
        let spread = 35f32.to_radians();
        for (j, r) in rows.iter().enumerate() {
            let id = node_id(r);
            let t = if k <= 1.0 { 0.0 } else { j as f32 / (k - 1.0) * 2.0 - 1.0 };
            let a = base + t * spread;
            if !index.contains_key(&id) {
                index.insert(id.clone(), scene.nodes.len());
                scene.nodes.push(Node {
                    label: stem(r.path.as_deref().or(r.dst_raw.as_deref()).unwrap_or("?")),
                    id: id.clone(),
                    depth: 2,
                    x: 2.0 * a.cos(),
                    y: 2.0 * a.sin(),
                    dangling: !r.resolved,
                    is_seed: false,
                });
            }
            scene.edges.push(Edge { from: pi, to: index[&id], dir: r.dir.clone() });
        }
    }
    scene
}

/// Parse the raw JSON body of `/graph/ego` (or the Lapis envelope's `data`).
pub fn from_json(json: &str) -> Result<Scene, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let inner = if v.get("rows").is_some() { v } else { v.get("data").cloned().unwrap_or(v) };
    let ego: Ego = serde_json::from_value(inner)?;
    Ok(build(&ego))
}

/// Draw primitives the window paints: straight `Line`s (dashed when dangling),
/// `Disc`s, and stem `Label`s.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Prim {
    Line { x0: f32, y0: f32, x1: f32, y1: f32, dashed: bool },
    Disc { x: f32, y: f32, r: f32, seed: bool, dangling: bool },
    Label { x: f32, y: f32, text: String },
}

pub fn draw(scene: &Scene) -> Vec<Prim> {
    let mut out = Vec::with_capacity(scene.edges.len() + scene.nodes.len() * 2);
    for e in &scene.edges {
        let (a, b) = (&scene.nodes[e.from], &scene.nodes[e.to]);
        out.push(Prim::Line { x0: a.x, y0: a.y, x1: b.x, y1: b.y, dashed: b.dangling || a.dangling });
    }
    for n in &scene.nodes {
        out.push(Prim::Disc {
            x: n.x,
            y: n.y,
            r: if n.is_seed { 0.12 } else { 0.07 },
            seed: n.is_seed,
            dangling: n.dangling,
        });
        out.push(Prim::Label { x: n.x, y: n.y + 0.14, text: n.label.clone() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"path":"Cross-References/Manual.md","direction":"both","hops":2,"count":5,"truncated":false,"rows":[
      {"depth":1,"via":"Cross-References/Manual.md","dir":"out","path":"Cross-References/Capital.md","dst_raw":"Capital","resolved":1},
      {"depth":1,"via":"Cross-References/Manual.md","dir":"in","path":"briefs/b.md","dst_raw":"Manual","resolved":1},
      {"depth":1,"via":"Cross-References/Manual.md","dir":"out","path":null,"dst_raw":"aes_schema_genesis_canon","resolved":0},
      {"depth":2,"via":"Cross-References/Capital.md","dir":"out","path":"ideas/capital/x.md","dst_raw":"x","resolved":1},
      {"depth":2,"via":"Cross-References/Capital.md","dir":"out","path":"ideas/capital/y.md","dst_raw":"y","resolved":1}]}"#;

    #[test]
    fn scene_from_ego_fixture() {
        let s = from_json(FIXTURE).unwrap();
        assert_eq!(s.nodes.len(), 6);
        assert_eq!(s.edges.len(), 5);
        assert!(!s.truncated);
        let seed = s.seed().unwrap();
        assert_eq!((seed.x, seed.y, seed.depth, seed.label.as_str()), (0.0, 0.0, 0, "Manual"));
        let ring1: Vec<&Node> = s.nodes.iter().filter(|n| n.depth == 1).collect();
        assert_eq!(ring1.len(), 3);
        for n in &ring1 {
            assert!(((n.x * n.x + n.y * n.y).sqrt() - 1.0).abs() < 1e-4, "depth-1 on the unit ring");
        }
        let dang = s.nodes.iter().find(|n| n.dangling).unwrap();
        assert_eq!(
            (dang.id.as_str(), dang.label.as_str()),
            ("dangling:aes_schema_genesis_canon", "aes_schema_genesis_canon")
        );
        let cap = s.index_of("Cross-References/Capital.md").unwrap();
        let kids: Vec<&Edge> = s.edges.iter().filter(|e| e.from == cap).collect();
        assert_eq!(kids.len(), 2);
        for e in kids {
            let n = &s.nodes[e.to];
            assert_eq!(n.depth, 2);
            assert!(((n.x * n.x + n.y * n.y).sqrt() - 2.0).abs() < 1e-4, "depth-2 on radius 2");
        }
        assert!(s.edges.iter().any(|e| e.dir == "in"));
        let prims = draw(&s);
        let discs = prims.iter().filter(|p| matches!(p, Prim::Disc { .. })).count();
        let lines = prims.iter().filter(|p| matches!(p, Prim::Line { .. })).count();
        let labels = prims.iter().filter(|p| matches!(p, Prim::Label { .. })).count();
        assert_eq!(discs, s.nodes.len(), "one Disc per node");
        assert_eq!(lines, s.edges.len(), "one Line per edge");
        assert_eq!(labels, s.nodes.len(), "one Label per node");
        assert_eq!(prims.len(), 5 + 6 * 2);
        assert!(prims.iter().any(|p| matches!(p, Prim::Line { dashed: true, .. })));
        assert!(prims.iter().any(|p| matches!(p, Prim::Disc { seed: true, .. })));
        // Lapis envelope wrapping is accepted too
        let wrapped = format!(r#"{{"ok":true,"data":{FIXTURE},"error":null,"meta":{{}}}}"#);
        assert_eq!(from_json(&wrapped).unwrap().nodes.len(), 6);
    }

    #[test]
    fn filters_hide_dangling_and_other_domains() {
        let s = from_json(FIXTURE).unwrap();
        assert_eq!(s.domains(), vec!["Cross-References".to_string(), "briefs".into(), "ideas".into()]);
        let no_dang = s.filtered(false, None);
        assert_eq!(no_dang.nodes.len(), 5, "seed + 4 resolved; dangling dropped");
        assert!(!no_dang.nodes.iter().any(|n| n.dangling));
        assert_eq!(draw(&no_dang).iter().filter(|p| matches!(p, Prim::Line { dashed: true, .. })).count(), 0);

        let ideas = s.filtered(true, Some("ideas"));
        assert!(ideas.seed().is_some(), "seed stays under a domain filter");
        assert!(ideas.nodes.iter().all(|n| n.is_seed || n.domain() == Some("ideas")));
        assert_eq!(ideas.nodes.iter().filter(|n| n.depth == 2).count(), 2);
    }

    #[test]
    fn truncated_and_empty() {
        let s = from_json(r#"{"path":"a.md","rows":[],"truncated":true}"#).unwrap();
        assert!(s.truncated && s.nodes.len() == 1 && s.edges.is_empty());
        assert!(from_json("{}").is_err());
    }
}
