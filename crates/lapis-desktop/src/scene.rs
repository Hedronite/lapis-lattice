//! Graph canvas data layer: nodes, edges and world positions.
//!
//! A [`Scene`] is what the window draws. Two things build one: the force sim on
//! a whole-vault snapshot ([`crate::graph_data::layout`], the default), and the
//! `/graph/ego` ring walk below, which is the debug view. Positions are in
//! canvas units (the ego ring radius is 1.0); [`crate::view`] owns the camera
//! that turns them into pixels and the paint list itself.

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
    /// In + out wikilinks across the whole vault. The disc radius reads it.
    pub degree: u32,
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

    /// Count each node's edges in this scene. The snapshot already knows a
    /// vault-wide degree; the ego walk does not, so it counts what it has.
    pub fn recompute_degrees(&mut self) {
        for n in self.nodes.iter_mut() {
            n.degree = 0;
        }
        for e in &self.edges {
            if let Some(n) = self.nodes.get_mut(e.from) {
                n.degree += 1;
            }
            if let Some(n) = self.nodes.get_mut(e.to) {
                n.degree += 1;
            }
        }
    }

    /// Move nodes the operator has dropped somewhere to where they were
    /// dropped. Pins survive a rebuild, which is what "pinned until unpin"
    /// means when the scene is rebuilt on every filter change.
    pub fn apply_pins(&mut self, pins: &BTreeMap<String, [f32; 2]>) {
        for n in self.nodes.iter_mut() {
            if let Some(p) = pins.get(&n.id) {
                n.x = p[0];
                n.y = p[1];
            }
        }
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
        degree: 0,
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
                degree: 0,
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
                    degree: 0,
                    dangling: !r.resolved,
                    is_seed: false,
                });
            }
            scene.edges.push(Edge { from: pi, to: index[&id], dir: r.dir.clone() });
        }
    }
    scene.recompute_degrees();
    scene
}

/// Parse the raw JSON body of `/graph/ego` (or the Lapis envelope's `data`).
pub fn from_json(json: &str) -> Result<Scene, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(json)?;
    let inner = if v.get("rows").is_some() { v } else { v.get("data").cloned().unwrap_or(v) };
    let ego: Ego = serde_json::from_value(inner)?;
    Ok(build(&ego))
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
        assert_eq!(seed.degree, 3, "the ego walk counts the edges it has");
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

        let ideas = s.filtered(true, Some("ideas"));
        assert!(ideas.seed().is_some(), "seed stays under a domain filter");
        assert!(ideas.nodes.iter().all(|n| n.is_seed || n.domain() == Some("ideas")));
        assert_eq!(ideas.nodes.iter().filter(|n| n.depth == 2).count(), 2);
    }

    #[test]
    fn pins_survive_a_rebuild() {
        let mut s = from_json(FIXTURE).unwrap();
        let pins = BTreeMap::from([("briefs/b.md".to_string(), [1.25f32, -0.5])]);
        s.apply_pins(&pins);
        let pinned = s.nodes.iter().find(|n| n.id == "briefs/b.md").unwrap();
        assert_eq!((pinned.x, pinned.y), (1.25, -0.5));
        let after = s.filtered(true, None);
        let still = after.nodes.iter().find(|n| n.id == "briefs/b.md").unwrap();
        assert_eq!((still.x, still.y), (1.25, -0.5), "a filter does not unpin");
    }

    #[test]
    fn truncated_and_empty() {
        let s = from_json(r#"{"path":"a.md","rows":[],"truncated":true}"#).unwrap();
        assert!(s.truncated && s.nodes.len() == 1 && s.edges.is_empty());
        assert!(from_json("{}").is_err());
    }
}
