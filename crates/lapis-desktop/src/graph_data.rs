//! Where the canvas gets its graph.
//!
//! Straight from the vault's own SQLite index, so the desktop needs no daemon —
//! the same embedded default the CLI uses.
//!
//! Two shapes live here and they are not interchangeable:
//!
//! * [`global_scene`] is the **default**. `Engine::graph_snapshot` hands over
//!   the whole vault, [`crate::sim`] settles it, and the positions the canvas
//!   draws are the ones the forces produced.
//! * [`scene_for`] is the old hop-1/hop-2 ego walk, assembled here out of
//!   `neighbors` calls. Its ring layout is a debug view now, not the default.

use std::collections::VecDeque;
use std::path::Path;

use crate::scene::{Ego, EgoRow, Node, Scene, build};
use crate::sim::{ForceParams, ForceSim};

/// Ticks a scene is settled for before the first frame. Enough for the fixture
/// vaults and small real ones to reach rest; the window keeps ticking after.
pub const SETTLE_TICKS: usize = 600;

/// Canvas half-extent in scene units. `scene::draw` and the window's `map()`
/// expect roughly -2..2.
const HALF_EXTENT: f32 = 2.0;

/// The whole-vault graph, laid out by the force sim.
///
/// `seed` only marks the active note; unlike the ego walk it does not decide
/// which nodes exist. A vault with no note at `seed` still draws in full.
pub fn global_scene(vault: &Path, seed: &str, ticks: usize) -> Result<Scene, String> {
    let engine = lapis_lattice::Engine::open(vault).map_err(|e| e.to_string())?;
    let snap = engine.graph_snapshot().map_err(|e| e.to_string())?;
    Ok(layout(&snap, seed, ticks))
}

/// Snapshot plus forces to a drawable scene. Split out from the I/O so the
/// layout is testable without a vault.
pub fn layout(snap: &lapis_lattice::GraphSnapshot, seed: &str, ticks: usize) -> Scene {
    let mut sim = ForceSim::from_snapshot(snap, ForceParams::default());
    sim.settle(ticks);
    let xy = sim.normalized(HALF_EXTENT);

    let index: std::collections::HashMap<&str, usize> =
        snap.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let mut edges = Vec::with_capacity(snap.edges.len());
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); snap.nodes.len()];
    for e in &snap.edges {
        let (Some(&from), Some(&to)) = (index.get(e.src.as_str()), index.get(e.dst.as_str())) else {
            continue;
        };
        adj[from].push(to);
        adj[to].push(from);
        edges.push(crate::scene::Edge { from, to, dir: "out".into() });
    }

    let depth = hops_from(&adj, index.get(seed).copied());
    let nodes = snap
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| Node {
            label: n.title.clone(),
            id: n.id.clone(),
            depth: depth[i],
            x: xy[i][0],
            y: xy[i][1],
            dangling: n.dangling,
            is_seed: n.id == seed,
        })
        .collect();

    Scene { nodes, edges, truncated: snap.truncated }
}

/// Link distance from the active note, ignoring direction. Nodes in another
/// component get [`u32::MAX`]: on a global graph "unreachable" is a real answer,
/// not depth zero.
fn hops_from(adj: &[Vec<usize>], seed: Option<usize>) -> Vec<u32> {
    let mut d = vec![u32::MAX; adj.len()];
    let Some(s) = seed else { return d };
    d[s] = 0;
    let mut q = VecDeque::from([s]);
    while let Some(i) = q.pop_front() {
        for &j in &adj[i] {
            if d[j] == u32::MAX {
                d[j] = d[i] + 1;
                q.push_back(j);
            }
        }
    }
    d
}

/// Hop-ring ego walk for `seed`, to `hops` (1 or 2) — the v0.2 debug view, kept
/// behind a toggle. Errors are strings because the caller is a UI that wants to
/// show them, not propagate them.
pub fn scene_for(vault: &Path, seed: &str, hops: u32, resolved_only: bool) -> Result<Scene, String> {
    let engine = lapis_lattice::Engine::open(vault).map_err(|e| e.to_string())?;
    let mut rows: Vec<EgoRow> = Vec::new();
    let mut seen: Vec<String> = vec![seed.to_string()];

    let hop1 = engine.neighbors(seed, "both").map_err(|e| e.to_string())?;
    for n in &hop1 {
        if resolved_only && !n.resolved {
            continue;
        }
        rows.push(EgoRow {
            depth: 1,
            via: Some(seed.to_string()),
            dir: n.dir.clone(),
            path: n.path.clone(),
            dst_raw: Some(n.dst_raw.clone()),
            resolved: n.resolved,
        });
        if let Some(p) = &n.path {
            seen.push(p.clone());
        }
    }

    if hops >= 2 {
        // one pass over the resolved hop-1 set; anything already seen is skipped
        // so the ring stays a tree rather than folding back on itself
        let parents: Vec<String> = hop1.iter().filter_map(|n| n.path.clone()).collect();
        for parent in parents {
            let Ok(kids) = engine.neighbors(&parent, "both") else { continue };
            for k in kids {
                if resolved_only && !k.resolved {
                    continue;
                }
                let id = k.path.clone().unwrap_or_else(|| format!("dangling:{}", k.dst_raw));
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id);
                rows.push(EgoRow {
                    depth: 2,
                    via: Some(parent.clone()),
                    dir: k.dir.clone(),
                    path: k.path.clone(),
                    dst_raw: Some(k.dst_raw.clone()),
                    resolved: k.resolved,
                });
            }
        }
    }

    Ok(build(&Ego { path: seed.to_string(), direction: "both".into(), hops, truncated: false, rows }))
}

/// First lines of a note, for the pane that opens when a node is clicked.
pub fn peek(vault: &Path, rel: &str, max_chars: usize) -> Result<String, String> {
    let abs = vault.join(rel);
    let text = std::fs::read_to_string(&abs).map_err(|e| format!("{rel}: {e}"))?;
    Ok(text.chars().take(max_chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::draw;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn vault() -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-desktop-graph-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(d.join("Welcome.md"), "# Welcome\n\nSee [[Alpha]] and [[ghost]].\n").unwrap();
        std::fs::write(d.join("notes/Alpha.md"), "# Alpha\n\nOn to [[Beta]].\n").unwrap();
        std::fs::write(d.join("notes/Beta.md"), "# Beta\n\nLeaf.\n").unwrap();
        std::fs::write(d.join("Orphan.md"), "# Orphan\n\nAlone.\n").unwrap();
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        d
    }

    #[test]
    fn global_scene_is_the_whole_vault_at_sim_positions() {
        let d = vault();
        let s = global_scene(&d, "Welcome.md", SETTLE_TICKS).unwrap();

        let mut ids: Vec<&str> = s.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["Orphan.md", "Welcome.md", "dangling:ghost", "notes/Alpha.md", "notes/Beta.md"],
            "the orphan proves this is not an ego walk"
        );
        assert_eq!(s.edges.len(), 3);
        assert!(s.nodes.iter().any(|n| n.is_seed && n.id == "Welcome.md"));
        assert!(s.nodes.iter().any(|n| n.dangling && n.label == "ghost"));

        // Positions come from the forces, not from a ring: no node sits on the
        // origin and no two share a point.
        assert!(s.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
        let mut pts: Vec<String> = s.nodes.iter().map(|n| format!("{:.4},{:.4}", n.x, n.y)).collect();
        pts.sort();
        pts.dedup();
        assert_eq!(pts.len(), s.nodes.len(), "the sim separates every node");
        assert!(s.nodes.iter().all(|n| n.x.abs() <= 2.001 && n.y.abs() <= 2.001), "fits the board");

        let beta = s.nodes.iter().find(|n| n.id == "notes/Beta.md").unwrap();
        assert_eq!(beta.depth, 2, "hop distance from the active note, not a ring index");
        let orphan = s.nodes.iter().find(|n| n.id == "Orphan.md").unwrap();
        assert_eq!(orphan.depth, u32::MAX, "another component is unreachable, not depth zero");

        // The old ring layout is still reachable and is a different answer.
        let rings = scene_for(&d, "Welcome.md", 2, false).unwrap();
        assert!(rings.nodes.len() < s.nodes.len(), "the ego ring is a subset of the vault");
        let seed_ring = rings.seed().unwrap();
        assert_eq!((seed_ring.x, seed_ring.y), (0.0, 0.0), "rings pin the seed; the sim does not");
        let seed_sim = s.seed().unwrap();
        assert!(seed_sim.x != 0.0 || seed_sim.y != 0.0);

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hop1_and_hop2_from_embedded_index() {
        let d = vault();
        let hop1 = scene_for(&d, "Welcome.md", 1, false).unwrap();
        assert_eq!(hop1.nodes.iter().filter(|n| n.depth == 1).count(), 2, "Alpha + dangling ghost");
        assert!(hop1.nodes.iter().any(|n| n.dangling && n.label == "ghost"));
        assert!(hop1.nodes.iter().all(|n| n.depth <= 1));

        let hop2 = scene_for(&d, "Welcome.md", 2, false).unwrap();
        assert!(hop2.nodes.iter().any(|n| n.id == "notes/Beta.md" && n.depth == 2));
        let prims = draw(&hop2);
        let discs = prims.iter().filter(|p| matches!(p, crate::scene::Prim::Disc { .. })).count();
        let lines = prims.iter().filter(|p| matches!(p, crate::scene::Prim::Line { .. })).count();
        assert_eq!(discs, hop2.nodes.len());
        assert_eq!(lines, hop2.edges.len());

        let resolved = scene_for(&d, "Welcome.md", 2, true).unwrap();
        assert!(!resolved.nodes.iter().any(|n| n.dangling));
        assert!(resolved.nodes.iter().any(|n| n.id == "notes/Beta.md"));

        let text = peek(&d, "notes/Alpha.md", 20).unwrap();
        assert!(text.starts_with("# Alpha"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
