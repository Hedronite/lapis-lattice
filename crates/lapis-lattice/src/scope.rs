//! Which files the index covers.
//!
//! Two rules on top of the fixed skip list. A folder below the vault root that
//! holds its own `.lapis/` or `.obsidian/` is another vault (a test fixture, a
//! nested project) and is left to its own index. And `.lapisignore` at the vault
//! root takes gitignore patterns, for generated output that lives inside the
//! vault without being notes. Search, the graph and the stale count all see the
//! same set, because every walk and every single-path reindex asks here.
use std::path::Path;

use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The vault-root file of gitignore patterns the index leaves out.
pub const IGNORE_FILE: &str = ".lapisignore";

/// Folders that mark a nested vault.
const VAULT_MARKERS: [&str; 2] = [".lapis", ".obsidian"];

/// The ignore rules for one vault, read once per walk.
pub struct Scope {
    ignore: Option<Gitignore>,
}

impl Scope {
    /// Read `vault/.lapisignore`. A missing file ignores nothing, and a line that
    /// is not a valid pattern is skipped rather than failing the index.
    pub fn load(vault: &Path) -> Self {
        let path = vault.join(IGNORE_FILE);
        if !path.is_file() {
            return Self { ignore: None };
        }
        let mut builder = GitignoreBuilder::new(vault);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for line in text.lines() {
            let _ = builder.add_line(Some(path.clone()), line);
        }
        Self { ignore: builder.build().ok() }
    }

    /// True when `rel` (vault-relative, `/`-separated) matches `.lapisignore`,
    /// itself or through a parent folder.
    pub fn ignores(&self, rel: &str, is_dir: bool) -> bool {
        self.ignore.as_ref().is_some_and(|g| g.matched_path_or_any_parents(rel, is_dir).is_ignore())
    }

    /// True when the file at `rel` belongs in the index: not ignored, and not
    /// inside a nested vault. For a single path; a walk prunes folders instead.
    pub fn covers(&self, vault: &Path, rel: &str) -> bool {
        if self.ignores(rel, false) {
            return false;
        }
        let mut dir = Path::new(rel).parent();
        while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            if is_nested_vault(&vault.join(d)) {
                return false;
            }
            dir = d.parent();
        }
        true
    }
}

/// True when `abs` resolves, through any links, to a path inside `vault`. A
/// note reached through a link out of the vault is never read into the index.
pub fn resolves_inside(vault: &Path, abs: &Path) -> bool {
    match (vault.canonicalize(), abs.canonicalize()) {
        (Ok(root), Ok(target)) => target.starts_with(root),
        _ => false,
    }
}

/// True when `dir` holds its own vault markers. Never asked of the vault root.
pub fn is_nested_vault(dir: &Path) -> bool {
    VAULT_MARKERS.iter().any(|m| dir.join(m).is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lapis-scope-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn no_ignore_file_covers_everything_outside_nested_vaults() {
        let v = vault("none");
        let s = Scope::load(&v);
        assert!(s.covers(&v, "notes/a.md"));
        std::fs::create_dir_all(v.join("fixtures/run1/.lapis")).unwrap();
        assert!(!s.covers(&v, "fixtures/run1/Note-1.md"), "a nested vault is its own index");
        assert!(!s.covers(&v, "fixtures/run1/deep/Note-2.md"), "all the way down");
        std::fs::create_dir_all(v.join("other/.obsidian")).unwrap();
        assert!(!s.covers(&v, "other/x.md"), "an Obsidian vault counts too");
        assert!(s.covers(&v, "fixtures/README.md"), "the folder above it is still indexed");
        let _ = std::fs::remove_dir_all(v);
    }

    #[test]
    fn the_index_leaves_out_nested_vaults_and_ignored_folders() {
        let v = vault("engine");
        let write = |rel: &str, text: &str| {
            let p = v.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("Welcome.md", "# Welcome\n\nSee [[Alpha]].\n");
        write("notes/Alpha.md", "# Alpha\n");
        write("fixtures/run1/vault/Note-1.md", "# Note 1\n\nSee [[Welcome]].\n");
        std::fs::create_dir_all(v.join("fixtures/run1/vault/.lapis")).unwrap();
        write("evidence/runtime/perf/Note-2.md", "# Note 2\n");
        write(IGNORE_FILE, "evidence/runtime/\n");

        let mut e = crate::Engine::open(&v).unwrap();
        let r = e.reindex().unwrap();
        assert_eq!(r.documents, 2, "only Welcome and Alpha are notes of this vault");
        let hits = e.search("note", 10).unwrap();
        assert!(
            hits.iter().all(|h| h.path.starts_with("notes/") || h.path == "Welcome.md"),
            "fixtures stay out"
        );
        let snap = e.graph_snapshot().unwrap();
        assert!(snap.nodes.iter().all(|n| !n.id.contains("fixtures") && !n.id.contains("evidence")));

        // Saving inside an ignored folder does not pull the file back in.
        let r = e.reindex_path("evidence/runtime/perf/Note-2.md").unwrap();
        assert_eq!(r.documents, 0);
        assert_eq!(e.reindex_path("fixtures/run1/vault/Note-1.md").unwrap().documents, 0);
        assert_eq!(e.reindex_path("notes/Alpha.md").unwrap().documents, 1, "a real note still indexes");
        let _ = std::fs::remove_dir_all(v);
    }

    #[cfg(unix)]
    #[test]
    fn the_walk_does_not_follow_links_out_of_the_vault_or_round_in_a_loop() {
        let v = vault("links");
        let outside = vault("links-outside");
        std::fs::write(outside.join("Secret.md"), "# Secret\n").unwrap();
        std::fs::create_dir_all(v.join("notes")).unwrap();
        std::fs::write(v.join("notes/Real.md"), "# Real\n").unwrap();
        let link = |to: &Path, at: &str| std::os::unix::fs::symlink(to, v.join(at)).unwrap();
        link(&outside, "away");
        link(&outside.join("Secret.md"), "notes/Leak.md");
        link(&v, "notes/loop");
        link(&v.join("notes/Real.md"), "Alias.md");

        let mut e = crate::Engine::open(&v).unwrap();
        e.reindex().unwrap();
        let mut ids: Vec<String> = e.graph_snapshot().unwrap().nodes.into_iter().map(|n| n.id).collect();
        ids.sort();
        assert_eq!(ids, ["Alias.md", "notes/Real.md"], "no way out, no loop; an inside link still counts");
        assert_eq!(e.reindex_path("notes/Leak.md").unwrap().documents, 0, "a single reindex refuses it too");
        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(v);
    }

    #[test]
    fn lapisignore_takes_gitignore_patterns() {
        let v = vault("patterns");
        std::fs::write(
            v.join(IGNORE_FILE),
            "# generated\nevidence/runtime/\n*.draft.md\n!keep.draft.md\n/root-only.md\n[unclosed\n",
        )
        .unwrap();
        let s = Scope::load(&v);
        assert!(s.ignores("evidence/runtime", true));
        assert!(!s.covers(&v, "evidence/runtime/perf-1/Note-1.md"), "a folder pattern covers its contents");
        assert!(s.covers(&v, "evidence/summary.md"));
        assert!(!s.covers(&v, "notes/plan.draft.md"));
        assert!(s.covers(&v, "notes/keep.draft.md"), "negation works");
        assert!(!s.covers(&v, "root-only.md"));
        assert!(s.covers(&v, "sub/root-only.md"), "an anchored pattern stays at the root");
        assert!(s.covers(&v, "notes/a.md"), "a bad line is skipped, not fatal");
        let _ = std::fs::remove_dir_all(v);
    }
}
