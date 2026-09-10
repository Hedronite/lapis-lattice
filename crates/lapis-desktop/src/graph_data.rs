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
