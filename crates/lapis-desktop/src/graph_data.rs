//! Where the canvas gets its graph.
//!
//! Straight from the vault's own SQLite index, so the desktop needs no daemon —
//! the same embedded default the CLI uses. The engine exposes hop-1
//! (`neighbors`); hop-2 is assembled here by walking the hop-1 set once, which
//! is the same DIY the CLI does and keeps the ego graph out of the engine until
//! it is genuinely needed there.

use std::path::Path;

use crate::scene::{Ego, EgoRow, Scene, build};

/// Build a scene for `seed`, to `hops` (1 or 2). Errors are strings because the
/// caller is a UI that wants to show them, not propagate them.
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
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        d
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
