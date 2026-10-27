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

/// Largest `.lapisignore` read. Patterns for a vault fit in a few kilobytes.
pub const IGNORE_CAP: u64 = 1 << 20;

/// The ignore rules for one vault, read once per walk.
pub struct Scope {
    ignore: Option<Gitignore>,
    problems: Vec<String>,
}

impl Scope {
    /// Read `vault/.lapisignore`. A missing file ignores nothing. A file that
    /// cannot be read, is over [`IGNORE_CAP`], or has lines that are not valid
    /// patterns still lets the index run on what it could read, and says so in
    /// [`Scope::problems`], so a broken file is never silently the same as none.
    pub fn load(vault: &Path) -> Self {
        let path = vault.join(IGNORE_FILE);
        let mut problems = Vec::new();
        let Ok(meta) = std::fs::metadata(&path) else { return Self { ignore: None, problems } };
        if !meta.is_file() {
            problems.push(format!("{IGNORE_FILE} is not a file; nothing is ignored"));
            return Self { ignore: None, problems };
        }
        if meta.len() > IGNORE_CAP {
            problems.push(format!(
                "{IGNORE_FILE} is over {IGNORE_CAP} bytes and was not read; nothing is ignored"
            ));
            return Self { ignore: None, problems };
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("{IGNORE_FILE} could not be read ({e}); nothing is ignored"));
                return Self { ignore: None, problems };
            }
        };
        let mut builder = GitignoreBuilder::new(vault);
        for (n, line) in text.lines().enumerate() {
            if let Err(e) = builder.add_line(Some(path.clone()), line) {
                problems.push(format!("{IGNORE_FILE} line {}: {e}; the line is skipped", n + 1));
            }
        }
        match builder.build() {
            Ok(g) => Self { ignore: Some(g), problems },
            Err(e) => {
                problems.push(format!("{IGNORE_FILE} could not be compiled ({e}); nothing is ignored"));
                Self { ignore: None, problems }
            }
        }
    }

    /// What went wrong reading `.lapisignore`, for `doctor` and `reindex --all`.
    /// Empty when the file is fine or absent.
    pub fn problems(&self) -> &[String] {
        &self.problems
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

    #[test]
    fn an_oversized_ignore_file_is_reported_not_read() {
        let v = vault("big-ignore");
        std::fs::write(v.join(IGNORE_FILE), "x".repeat(IGNORE_CAP as usize + 1)).unwrap();
        let s = Scope::load(&v);
        assert!(s.covers(&v, "x"), "nothing is ignored");
        assert!(s.problems()[0].contains("not read"));
        let _ = std::fs::remove_dir_all(v);
    }

    #[test]
    fn a_note_excluded_after_indexing_counts_as_stale_until_a_full_reindex() {
        let v = vault("late-ignore");
        std::fs::create_dir_all(v.join("gen")).unwrap();
        std::fs::write(v.join("notes.md"), "# Notes\n").unwrap();
        std::fs::write(v.join("gen/out.md"), "# Out\n").unwrap();
        let mut e = crate::Engine::open(&v).unwrap();
        e.reindex().unwrap();
        assert_eq!(e.health().unwrap().stale_documents, 0);
        std::fs::write(v.join(IGNORE_FILE), "gen/\n").unwrap();
        assert_eq!(e.health().unwrap().stale_documents, 1, "the excluded note is still indexed, and flagged");
        e.reindex().unwrap();
        assert_eq!(e.health().unwrap().stale_documents, 0, "a full reindex drops it");
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
        assert!(s.problems().is_empty(), "a lenient pattern is no problem: {:?}", s.problems());
        assert!(Scope::load(&v.join("nowhere")).problems().is_empty(), "no file, no problem");
        std::fs::write(v.join(IGNORE_FILE), [0xff, 0xfe, b'\n']).unwrap();
        let unreadable = Scope::load(&v);
        assert!(unreadable.covers(&v, "notes/a.md"), "an unreadable file ignores nothing");
        assert!(unreadable.problems()[0].contains("could not be read"), "{:?}", unreadable.problems());
        let _ = std::fs::remove_dir_all(v);
    }
}
