//! The sidebar's folder tree: listings load per folder, folders expand in place,
//! and the visible rows are a depth-first walk of what is expanded.
use crate::services::FileEntry;
use std::collections::{BTreeMap, BTreeSet};

/// Deeper rows are not walked. The lister already skips links back up the tree;
/// this is the backstop for any cycle it cannot see.
pub const MAX_DEPTH: usize = 48;

#[derive(Default)]
pub struct Tree {
    /// Loaded listings by folder path; `""` is the vault root.
    listings: BTreeMap<String, Vec<FileEntry>>,
    expanded: BTreeSet<String>,
    /// Folder → generation of its newest request. A reply for an older one is dropped.
    pending: BTreeMap<String, u64>,
    errors: BTreeMap<String, String>,
    generation: u64,
}

#[derive(Debug)]
pub struct Row<'a> {
    pub entry: &'a FileEntry,
    pub depth: usize,
    pub expanded: bool,
    /// A folder whose listing is on its way.
    pub loading: bool,
}

/// `a/b/c.md` → `["a", "a/b"]`: the folders that must be open for the path to show.
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(slash) = path[at..].find('/') {
        at += slash;
        out.push(path[..at].to_string());
        at += 1;
    }
    out
}

impl Tree {
    /// Start a request for `folder` and return its generation.
    pub fn request(&mut self, folder: &str) -> u64 {
        self.generation += 1;
        self.pending.insert(folder.to_string(), self.generation);
        self.generation
    }
    /// Apply a listing reply. Returns false when a newer request superseded it.
    pub fn receive(&mut self, folder: &str, generation: u64, result: Result<Vec<FileEntry>, String>) -> bool {
        if self.pending.get(folder) != Some(&generation) {
            return false;
        }
        self.pending.remove(folder);
        match result {
            Ok(entries) => {
                self.errors.remove(folder);
                self.listings.insert(folder.to_string(), entries);
            }
            Err(error) => {
                self.errors.insert(folder.to_string(), error);
            }
        }
        true
    }
    pub fn is_loaded(&self, folder: &str) -> bool {
        self.listings.contains_key(folder)
    }
    pub fn is_loading(&self, folder: &str) -> bool {
        self.pending.contains_key(folder)
    }
    pub fn error(&self, folder: &str) -> Option<&str> {
        self.errors.get(folder).map(String::as_str)
    }
    pub fn is_expanded(&self, folder: &str) -> bool {
        self.expanded.contains(folder)
    }
    /// Open or close a folder. Returns whether it is now open.
    pub fn toggle(&mut self, folder: &str) -> bool {
        if !self.expanded.remove(folder) {
            self.expanded.insert(folder.to_string());
        }
        self.is_expanded(folder)
    }
    pub fn expand(&mut self, folder: &str) {
        self.expanded.insert(folder.to_string());
    }
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }
    /// The root and every open folder: what a refresh reloads.
    pub fn visible_folders(&self) -> Vec<String> {
        std::iter::once(String::new()).chain(self.expanded.iter().cloned()).collect()
    }
    /// The rows the sidebar shows, depth first, folders opening beneath themselves.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut out = Vec::new();
        self.walk("", 0, &mut out);
        out
    }
    fn walk<'a>(&'a self, folder: &str, depth: usize, out: &mut Vec<Row<'a>>) {
        let Some(entries) = self.listings.get(folder) else { return };
        for entry in entries {
            let expanded = entry.directory && self.expanded.contains(&entry.path);
            out.push(Row {
                entry,
                depth,
                expanded,
                loading: entry.directory && self.is_loading(&entry.path),
            });
            if expanded && depth + 1 < MAX_DEPTH {
                self.walk(&entry.path, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, directory: bool) -> FileEntry {
        FileEntry { path: path.into(), name: path.rsplit('/').next().unwrap().into(), directory }
    }
    fn names(tree: &Tree) -> Vec<(String, usize)> {
        tree.rows().iter().map(|r| (r.entry.path.clone(), r.depth)).collect()
    }

    #[test]
    fn folders_open_beneath_themselves_and_close_again() {
        let mut tree = Tree::default();
        let g = tree.request("");
        assert!(tree.receive("", g, Ok(vec![entry("a", true), entry("z.md", false)])));
        assert_eq!(names(&tree), [("a".into(), 0), ("z.md".into(), 0)]);
        assert!(tree.toggle("a"));
        let g = tree.request("a");
        assert!(tree.rows()[0].loading);
        tree.receive("a", g, Ok(vec![entry("a/b", true), entry("a/n.md", false)]));
        assert_eq!(
            names(&tree),
            [("a".into(), 0), ("a/b".into(), 1), ("a/n.md".into(), 1), ("z.md".into(), 0)]
        );
        assert!(!tree.toggle("a"));
        assert_eq!(names(&tree), [("a".into(), 0), ("z.md".into(), 0)]);
        // The listing is kept, so reopening needs no request.
        assert!(tree.is_loaded("a"));
    }

    #[test]
    fn a_stale_reply_is_dropped() {
        let mut tree = Tree::default();
        let old = tree.request("");
        let new = tree.request("");
        assert!(!tree.receive("", old, Ok(vec![entry("old.md", false)])));
        assert!(tree.receive("", new, Err("offline".into())));
        assert_eq!(tree.error(""), Some("offline"));
        assert!(tree.rows().is_empty());
    }

    #[test]
    fn ancestors_are_the_folders_above_a_path() {
        assert_eq!(ancestors("a/b/c.md"), ["a", "a/b"]);
        assert!(ancestors("top.md").is_empty());
        let mut tree = Tree::default();
        tree.expand("a/b");
        assert_eq!(tree.visible_folders(), ["", "a/b"]);
    }

    #[test]
    fn a_cycle_the_lister_missed_stops_at_the_depth_cap() {
        let mut tree = Tree::default();
        let mut folder = String::new();
        for _ in 0..MAX_DEPTH + 10 {
            let child = if folder.is_empty() { "loop".to_string() } else { format!("{folder}/loop") };
            let g = tree.request(&folder);
            tree.receive(&folder, g, Ok(vec![entry(&child, true)]));
            tree.expand(&child);
            folder = child;
        }
        assert_eq!(tree.rows().len(), MAX_DEPTH);
    }
}
