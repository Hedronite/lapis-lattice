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

use lapis_lattice::GraphSnapshot;

use crate::scene::{Ego, EgoRow, Node, Scene, build};
use crate::sim::{ForceParams, ForceSim};

/// Ticks a scene is settled for before the first frame. Enough for the fixture
/// vaults and small real ones to reach rest; the window keeps ticking after.
pub const SETTLE_TICKS: usize = 700;

/// A laid-out graph and the sim that is still laying it out. The window keeps
/// both: the scene is what gets drawn, the sim is what keeps moving it.
pub struct Laid {
    pub scene: Scene,
    pub sim: ForceSim,
}

/// Minimum gap between two discs after the settle, as a share of the link rest
/// length, and how many relaxation passes enforce it.
pub const SEPARATION: f32 = 0.25;
pub const SEPARATION_PASSES: usize = 60;

/// Depth range the local-mode slider offers.
pub const MIN_DEPTH: u32 = 1;
pub const MAX_DEPTH: u32 = 8;

/// Read the whole-vault snapshot from the embedded index.
pub fn snapshot_for(vault: &Path) -> Result<GraphSnapshot, String> {
    let engine = lapis_lattice::Engine::open(vault).map_err(|e| e.to_string())?;
    engine.graph_snapshot().map_err(|e| e.to_string())
}

/// The whole-vault graph, laid out by the force sim. This is what `lapis
/// desktop` opens with, `--path` or not.
///
/// `seed` only marks the active note; unlike the ego walk it does not decide
/// which nodes exist. A vault with no note at `seed` still draws in full.
pub fn global_scene(vault: &Path, seed: &str, ticks: usize) -> Result<Scene, String> {
    Ok(layout(&snapshot_for(vault)?, seed, ticks))
}

/// The global graph plus its live sim.
pub fn global_live(vault: &Path, seed: &str, ticks: usize) -> Result<Laid, String> {
    Ok(lay_out(&snapshot_for(vault)?, seed, ticks))
}

/// Local mode plus its live sim.
pub fn local_live(vault: &Path, seed: &str, depth: u32, ticks: usize) -> Result<Laid, String> {
    let snap = snapshot_for(vault)?;
    Ok(lay_out(&within(&snap, seed, depth), seed, ticks))
}

/// Local mode: everything within `depth` links of `seed`, undirected, cut out
/// of the same snapshot the global view uses.
///
/// This is a walk over rows already in memory. It never re-reads the vault and
/// never builds a second index, which is the difference between a depth slider
/// and a reindex.
pub fn local_scene(vault: &Path, seed: &str, depth: u32, ticks: usize) -> Result<Scene, String> {
    let snap = snapshot_for(vault)?;
    Ok(layout(&within(&snap, seed, depth), seed, ticks))
}

/// The sub-snapshot within `depth` hops of `seed`. Degree stays vault-wide, so
/// a hub looks like a hub even when most of its links are out of view.
pub fn within(snap: &GraphSnapshot, seed: &str, depth: u32) -> GraphSnapshot {
    let depth = depth.clamp(MIN_DEPTH, MAX_DEPTH);
    let index: std::collections::HashMap<&str, usize> =
        snap.nodes.iter().enumerate().map(|(i, n)| (n.id.as_str(), i)).collect();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); snap.nodes.len()];
    for e in &snap.edges {
        if let (Some(&a), Some(&b)) = (index.get(e.src.as_str()), index.get(e.dst.as_str())) {
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    let Some(&start) = index.get(seed) else {
        // No such note: an empty local graph is the honest answer, not the
        // whole vault relabelled.
        return GraphSnapshot {
            generated_at: snap.generated_at.clone(),
            vault: snap.vault.clone(),
            truncated: snap.truncated,
            nodes: Vec::new(),
            edges: Vec::new(),
        };
    };
    let hops = hops_from(&adj, Some(start));
    let keep: std::collections::BTreeSet<usize> =
        (0..snap.nodes.len()).filter(|&i| hops[i] <= depth).collect();
    let ids: std::collections::BTreeSet<&str> = keep.iter().map(|&i| snap.nodes[i].id.as_str()).collect();
    GraphSnapshot {
        generated_at: snap.generated_at.clone(),
        vault: snap.vault.clone(),
        truncated: snap.truncated,
        nodes: keep.iter().map(|&i| snap.nodes[i].clone()).collect(),
        edges: snap
            .edges
            .iter()
            .filter(|e| ids.contains(e.src.as_str()) && ids.contains(e.dst.as_str()))
            .cloned()
            .collect(),
    }
}

/// Snapshot plus forces to a drawable scene. Split out from the I/O so the
/// layout is testable without a vault.
pub fn layout(snap: &GraphSnapshot, seed: &str, ticks: usize) -> Scene {
    lay_out(snap, seed, ticks).scene
}

/// Snapshot plus forces to a drawable scene and the sim still driving it.
///
/// Positions are the sim's own world units. Nothing is rescaled per frame: a
/// graph that renormalised every tick would breathe as its extent changed.
/// Fitting is the camera's job ([`crate::view::Camera::fit`]).
pub fn lay_out(snap: &GraphSnapshot, seed: &str, ticks: usize) -> Laid {
    lay_out_streaming(snap, seed, ticks, &mut |_| {})
}

/// What a layout in progress hands the window, newest wins.
pub enum Unfold {
    /// The scene, laid on its starting spiral, before the first tick.
    Scene(Scene),
    /// Every scene node's position, part way through the settle.
    Positions(Vec<[f32; 2]>),
}

impl Unfold {
    /// Fold this update into `latest`, the slot the window reads each frame.
    pub fn into_slot(self, latest: &mut Unfolding) {
        match self {
            Unfold::Scene(scene) => {
                latest.scene = Some(scene);
                latest.positions = None;
            }
            Unfold::Positions(xy) => latest.positions = Some(xy),
        }
    }
}

/// The newest of a layout in progress that the window has not drawn yet.
/// Positions it has not drawn are replaced, never queued.
#[derive(Default)]
pub struct Unfolding {
    pub scene: Option<Scene>,
    pub positions: Option<Vec<[f32; 2]>>,
}

/// How often a layout in progress publishes positions: about thirty a second.
pub const UNFOLD_EVERY: std::time::Duration = std::time::Duration::from_millis(33);

/// [`lay_out`], publishing as it goes so the window can show the graph
/// unfolding instead of a card. The result is the same as [`lay_out`]'s.
pub fn lay_out_streaming(
    snap: &GraphSnapshot,
    seed: &str,
    ticks: usize,
    publish: &mut dyn FnMut(Unfold),
) -> Laid {
    let mut sim = ForceSim::from_snapshot(snap, ForceParams::default());
    let gap = sim.params.link_distance * SEPARATION;
    // Isolated notes feel no force, so the belt can follow the core as it grows
    // instead of waiting in the middle and jumping out at the end.
    sim.place_isolates(gap);
    let mut scene = scene_of(snap, seed, sim.positions());
    publish(Unfold::Scene(scene.clone()));
    let mut shown = std::time::Instant::now();
    for _ in 0..ticks {
        if !sim.tick() {
            break;
        }
        if shown.elapsed() >= UNFOLD_EVERY {
            sim.place_isolates(gap);
            publish(Unfold::Positions(sim.positions().to_vec()));
            shown = std::time::Instant::now();
        }
    }
    // Forces place the clusters; these two passes give every disc room of its own
    // and set isolated notes on a belt around the core.
    sim.separate(gap, SEPARATION_PASSES);
    sim.place_isolates(gap);
    sync_positions(&mut scene, &sim);
    Laid { scene, sim }
}

/// The drawable scene for `snap`, with nodes at `xy`.
fn scene_of(snap: &GraphSnapshot, seed: &str, xy: &[[f32; 2]]) -> Scene {
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
            degree: n.degree,
            dangling: n.dangling,
            is_seed: n.id == seed,
        })
        .collect();

    Scene { nodes, edges, truncated: snap.truncated }
}

/// Copy the sim's current positions onto the scene it laid out. The two are
/// index-parallel by construction, so this is the whole of "the graph moved".
pub fn sync_positions(scene: &mut Scene, sim: &ForceSim) {
    for (n, p) in scene.nodes.iter_mut().zip(sim.positions()) {
        n.x = p[0];
        n.y = p[1];
    }
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
    use std::io::Read;
    // The node id came from the index; it still has to land inside the vault, so
    // an in-vault symlink pointing out of it is not followed.
    let root = vault.canonicalize().map_err(|e| vault_relative(vault, e.to_string()))?;
    let abs = vault.join(rel).canonicalize().map_err(|e| format!("{rel}: {e}"))?;
    if !abs.starts_with(&root) {
        return Err(format!("{rel}: outside the vault"));
    }
    // Four bytes per char at most: read no more than the preview can show.
    let mut bytes = Vec::new();
    std::fs::File::open(&abs)
        .and_then(|f| f.take(max_chars as u64 * 4).read_to_end(&mut bytes))
        .map_err(|e| format!("{rel}: {e}"))?;
    Ok(String::from_utf8_lossy(&bytes).chars().take(max_chars).collect())
}

/// `message` with the vault's location replaced, so an error shown on the board
/// names notes by their vault path and never says where the vault lives on disk.
pub fn vault_relative(vault: &Path, message: String) -> String {
    let mut message = message;
    for root in [vault.canonicalize().ok(), Some(vault.to_path_buf())].into_iter().flatten() {
        let root = root.to_string_lossy();
        if !root.is_empty() {
            message = message.replace(&format!("{root}/"), "").replace(root.as_ref(), "<vault>");
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Camera, Highlight, Prim, paint};
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
        // a chain deep enough that a depth slider has something to slide over
        std::fs::write(d.join("notes/Beta.md"), "# Beta\n\nOn to [[Gamma]].\n").unwrap();
        std::fs::write(d.join("notes/Gamma.md"), "# Gamma\n\nOn to [[Delta]].\n").unwrap();
        std::fs::write(d.join("notes/Delta.md"), "# Delta\n\nLeaf.\n").unwrap();
        std::fs::write(d.join("Orphan.md"), "# Orphan\n\nAlone.\n").unwrap();
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        d
    }

    #[test]
    fn local_mode_is_a_depth_walk_over_the_same_snapshot() {
        let d = vault();
        let snap = snapshot_for(&d).unwrap();
        let whole = snap.nodes.len();
        assert_eq!(whole, 7);

        // Welcome -> Alpha -> Beta -> Gamma -> Delta, plus a dangling ghost at
        // depth 1 and an orphan in another component that never arrives.
        let counts: Vec<usize> = (1..=6).map(|k| within(&snap, "Welcome.md", k).nodes.len()).collect();
        assert_eq!(counts, vec![3, 4, 5, 6, 6, 6], "the slider changes the node count");
        assert!(counts.iter().all(|&n| n < whole), "the orphan is never within reach of the seed");

        let d1 = within(&snap, "Welcome.md", 1);
        let ids: Vec<&str> = d1.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["Welcome.md", "notes/Alpha.md", "dangling:ghost"], "snapshot order kept");
        assert!(
            d1.edges.iter().all(|e| ids.contains(&e.src.as_str()) && ids.contains(&e.dst.as_str())),
            "no edge dangles off the cut"
        );
        assert_eq!(
            d1.nodes.iter().find(|n| n.id == "notes/Alpha.md").unwrap().degree,
            2,
            "degree stays vault-wide, so a hub still looks like a hub in local mode"
        );

        // Out of range clamps rather than surprising the caller.
        assert_eq!(within(&snap, "Welcome.md", 0).nodes.len(), 3);
        assert_eq!(within(&snap, "Welcome.md", 99).nodes.len(), 6);
        assert!(within(&snap, "no/such.md", 3).nodes.is_empty(), "an unknown seed is empty, not whole");

        // And it is a walk over rows already read, not a second index.
        let scene = local_scene(&d, "Welcome.md", 2, 200).unwrap();
        assert_eq!(scene.nodes.len(), 4);
        assert!(scene.nodes.iter().any(|n| n.is_seed && n.id == "Welcome.md"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn snapshot_degree_reaches_the_canvas() {
        let d = vault();
        let s = global_scene(&d, "Welcome.md", SETTLE_TICKS).unwrap();
        let by = |id: &str| s.nodes.iter().find(|n| n.id == id).unwrap().degree;
        assert_eq!(by("Welcome.md"), 2);
        assert_eq!(by("notes/Beta.md"), 2);
        assert_eq!(by("notes/Delta.md"), 1);
        assert_eq!(by("Orphan.md"), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_streaming_layout_shows_the_scene_first_and_lands_where_lay_out_does() {
        let d = vault();
        let snap = snapshot_for(&d).unwrap();
        let mut seen = Unfolding::default();
        let mut first_was_scene = None;
        let laid = lay_out_streaming(&snap, "Welcome.md", SETTLE_TICKS, &mut |u| {
            first_was_scene.get_or_insert(matches!(u, Unfold::Scene(_)));
            if let Unfold::Positions(xy) = &u {
                assert_eq!(xy.len(), snap.nodes.len(), "every frame places every node");
            }
            u.into_slot(&mut seen);
        });
        assert_eq!(first_was_scene, Some(true), "the board gets the scene before any positions");
        let shown = seen.scene.expect("the scene was published");
        assert_eq!(shown.nodes.len(), laid.scene.nodes.len());
        assert_eq!(shown.edges.len(), laid.scene.edges.len());
        let plain = lay_out(&snap, "Welcome.md", SETTLE_TICKS);
        let xy = |s: &Scene| s.nodes.iter().map(|n| [n.x, n.y]).collect::<Vec<_>>();
        assert_eq!(xy(&laid.scene), xy(&plain.scene), "publishing does not change the result");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_newer_unfold_replaces_one_not_yet_drawn() {
        let mut slot = Unfolding::default();
        Unfold::Positions(vec![[1.0, 1.0]]).into_slot(&mut slot);
        Unfold::Positions(vec![[2.0, 2.0]]).into_slot(&mut slot);
        assert_eq!(slot.positions, Some(vec![[2.0, 2.0]]));
        Unfold::Scene(Scene::default()).into_slot(&mut slot);
        assert!(slot.scene.is_some() && slot.positions.is_none(), "a new scene drops stale positions");
    }

    #[test]
    fn global_scene_is_the_whole_vault_at_sim_positions() {
        let d = vault();
        let s = global_scene(&d, "Welcome.md", SETTLE_TICKS).unwrap();

        let mut ids: Vec<&str> = s.nodes.iter().map(|n| n.id.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "Orphan.md",
                "Welcome.md",
                "dangling:ghost",
                "notes/Alpha.md",
                "notes/Beta.md",
                "notes/Delta.md",
                "notes/Gamma.md"
            ],
            "the orphan proves this is not an ego walk"
        );
        assert_eq!(s.edges.len(), 5);
        assert!(s.nodes.iter().any(|n| n.is_seed && n.id == "Welcome.md"));
        assert!(s.nodes.iter().any(|n| n.dangling && n.label == "ghost"));

        // Positions come from the forces, not from a ring: no node sits on the
        // origin and no two share a point.
        assert!(s.nodes.iter().all(|n| n.x.is_finite() && n.y.is_finite()));
        let mut pts: Vec<String> = s.nodes.iter().map(|n| format!("{:.4},{:.4}", n.x, n.y)).collect();
        pts.sort();
        pts.dedup();
        assert_eq!(pts.len(), s.nodes.len(), "the sim separates every node");
        // Positions are the sim's own units; the camera is what frames them.
        let mut cam = crate::view::Camera::default();
        cam.fit(&s, [800.0, 600.0]);
        assert!(
            s.nodes.iter().all(|n| {
                let p = cam.to_screen([n.x, n.y], [800.0, 600.0]);
                (0.0..=800.0).contains(&p[0]) && (0.0..=600.0).contains(&p[1])
            }),
            "fit frames every node inside the board"
        );

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
        // The ring walk lays out in its own small units, so the camera frames
        // it exactly as it frames a sim layout.
        let board = [800.0f32, 600.0];
        let mut cam = Camera::default();
        cam.fit(&hop2, board);
        let prims = paint(&hop2, &cam, board, &Highlight::default(), &|_| false, &[]);
        let discs = prims.iter().filter(|p| matches!(p, Prim::Disc { .. })).count();
        let strokes = prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).count();
        assert_eq!(discs, hop2.nodes.len());
        assert!(strokes > 0 && strokes <= hop2.edges.len(), "rim-trimmed strokes, no dotted quads");

        let resolved = scene_for(&d, "Welcome.md", 2, true).unwrap();
        assert!(!resolved.nodes.iter().any(|n| n.dangling));
        assert!(resolved.nodes.iter().any(|n| n.id == "notes/Beta.md"));

        let text = peek(&d, "notes/Alpha.md", 20).unwrap();
        assert!(text.starts_with("# Alpha"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn peek_stays_inside_the_vault_and_errors_hide_its_location() {
        let d = vault();
        let outside = d.with_extension("outside.md");
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("notes/leak.md")).unwrap();
        assert!(peek(&d, "notes/leak.md", 20).unwrap_err().contains("outside the vault"));
        let raw = format!("unable to open {}/.lapis/lattice.sqlite", d.display());
        assert_eq!(vault_relative(&d, raw), "unable to open .lapis/lattice.sqlite");
        assert_eq!(vault_relative(&d, format!("vault {} is gone", d.display())), "vault <vault> is gone");
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(&d);
    }
}
