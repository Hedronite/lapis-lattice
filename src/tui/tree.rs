//! Sidebar tree: lazy per-folder listing with the task scan-set skips, so the
//! media tree is never read. Folders first, case-insensitive.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::{notes, tasks};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub rel: String,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Default)]
pub struct Tree {
    pub children: HashMap<String, Vec<Entry>>,
    pub expanded: HashSet<String>,
}

pub fn list_dir(root: &Path, rel: &str) -> Vec<Entry> {
    let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(ft) = e.file_type() else { continue };
        let is_dir = ft.is_dir();
        if tasks::excluded(&child, is_dir, &name) {
            continue;
        }
        if !is_dir
            && !matches!(notes::kind_of(&name), notes::Kind::Markdown | notes::Kind::Pdf | notes::Kind::Html)
        {
            continue;
        }
        out.push(Entry { rel: child, name, is_dir });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

impl Tree {
    pub fn load(&mut self, root: &Path, rel: &str) {
        if !self.children.contains_key(rel) {
            let kids = list_dir(root, rel);
            self.children.insert(rel.to_string(), kids);
        }
    }
    pub fn reload(&mut self, root: &Path, rel: &str) {
        self.children.remove(rel);
        self.load(root, rel);
    }
    /// Flattened visible rows: (depth, entry).
    pub fn visible(&self) -> Vec<(usize, Entry)> {
        let mut out = Vec::new();
        self.walk("", 0, &mut out);
        out
    }
    fn walk(&self, rel: &str, depth: usize, out: &mut Vec<(usize, Entry)>) {
        if let Some(kids) = self.children.get(rel) {
            for k in kids {
                out.push((depth, k.clone()));
                if k.is_dir && self.expanded.contains(&k.rel) {
                    self.walk(&k.rel, depth + 1, out);
                }
            }
        }
    }
    /// Expand every ancestor folder of `rel` (used to reveal an opened note).
    pub fn reveal(&mut self, root: &Path, rel: &str) -> Option<usize> {
        let mut acc = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.load(root, &acc);
            self.expanded.insert(acc.clone());
        }
        self.visible().iter().position(|(_, e)| e.rel == rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_lists_lazily_and_skips_excluded() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let v = std::env::temp_dir().join(format!("lapis-tui-{}-{n}-{seq}", std::process::id()));
        for d in ["notes", "agents", "Aeon/notes/media", ".obsidian", "_archives"] {
            std::fs::create_dir_all(v.join(d)).unwrap();
        }
        std::fs::write(v.join("notes/SPEC.md"), "x").unwrap();
        std::fs::write(v.join("notes/pic.png"), "x").unwrap();
        std::fs::write(v.join("Aeon/notes/media/a.md"), "x").unwrap();
        std::fs::write(v.join("README.md"), "x").unwrap();
        let mut t = Tree::default();
        t.load(&v, "");
        let names: Vec<String> = t.visible().iter().map(|(_, e)| e.name.clone()).collect();
        assert_eq!(names, ["Aeon", "agents", "notes", "README.md"]);
        let pos = t.reveal(&v, "notes/SPEC.md").unwrap();
        let rows = t.visible();
        assert_eq!(rows[pos].1.name, "SPEC.md");
        assert_eq!(rows[pos].0, 1);
        assert!(!rows.iter().any(|(_, e)| e.name == "pic.png"));
        assert!(list_dir(&v, "Aeon/notes/media").is_empty(), "media never listed");
        let _ = std::fs::remove_dir_all(&v);
    }
}
