//! Stale-index signal: notes on disk that the index has not caught up with.
//!
//! A note written by hand stays invisible to search until it is reindexed, and a
//! confident miss looks like an answer. `search` reports how many notes drifted
//! (`meta.stale`); `vault info` reports the counts and names the paths.
//!
//! The check lists the index through the same backend search uses (embedded or
//! HTTP, per `Ctx::backend`) and walks only the folders that hold indexed notes.
//! Markdown only. A note is stale when:
//! - its file is newer than its index entry (`changed`),
//! - it sits in a folder that holds indexed notes but has no entry itself (`unindexed`),
//! - its entry points at a file that is gone (`removed`).
//!
//! A folder with no indexed note is not walked: the index may leave it out on
//! purpose, and an HTTP lattice applies exclude rules this binary cannot see.
//! A first note in a brand-new folder is therefore not counted.
//!
//! Listing ~9k documents over HTTP takes most of a second, against ~0.2 s for a
//! keyword search, so `search` and `vault info` reuse a result up to [`TTL`] old;
//! `doctor` always checks afresh. The cache lives
//! in the user cache directory, never in the vault, and a reindex through this
//! binary drops it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::backend::Backend;
use crate::error::Result;
use crate::http::ListParams;

/// How long `search` trusts a previous check.
pub const TTL: Duration = Duration::from_secs(60);
/// `vault info` names at most this many stale paths.
const PATH_SAMPLE: usize = 20;
const PAGE: u32 = 1000;
/// Embedded entries store milliseconds; HTTP entries store float seconds.
const MTIME_SLACK: f64 = 0.001;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stale {
    /// `changed + unindexed + removed`.
    pub count: u64,
    pub changed: u64,
    pub unindexed: u64,
    pub removed: u64,
    /// The first stale paths in path order, at most 20.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Unix seconds when the check ran.
    pub checked_at: u64,
}

/// Index entries by path: markdown only, with the indexed mtime in seconds when
/// the backend reports one.
pub type Indexed = HashMap<String, Option<f64>>;

/// Run the check now, and refresh the cache.
pub async fn check(backend: &Backend, root: &Path) -> Result<Stale> {
    let indexed = list_indexed(backend).await?;
    let stale = compare(root, &indexed, now_secs());
    if let Some(file) = cache_file(root) {
        store(&file, &backend.source(), &stale);
    }
    Ok(stale)
}

/// What `search` and `vault info` report: a cached check up to [`TTL`] old, else a
/// new one. `None` when the check could not run; neither command fails over it.
pub async fn cached(backend: &Backend, root: &Path) -> Option<Stale> {
    if let Some(file) = cache_file(root)
        && let Some(s) = load(&file, &backend.source(), now_secs())
    {
        return Some(s);
    }
    check(backend, root).await.ok()
}

/// Drop the cached check after this binary reindexed something.
pub fn forget(root: &Path) {
    if let Some(file) = cache_file(root) {
        let _ = std::fs::remove_file(file);
    }
}

/// One stderr line for human `search` output; silent when nothing is stale.
pub fn warn(count: Option<u64>) {
    if let Some(n) = count.filter(|n| *n > 0) {
        eprintln!(
            "{n} notes changed since indexing; `lapis vault info` lists them, `lapis reindex PATH` fixes one, `lapis reindex --all` fixes all"
        );
    }
}

async fn list_indexed(backend: &Backend) -> Result<Indexed> {
    let mut out = Indexed::new();
    let mut offset = 0u32;
    loop {
        let page = backend
            .documents(&ListParams { limit: PAGE, offset, include_archives: true, ..Default::default() })
            .await?;
        let n = u32::try_from(page.len()).unwrap_or(u32::MAX);
        for d in page {
            if d.path.ends_with(".md") {
                out.insert(d.path, d.mtime);
            }
        }
        if n < PAGE {
            return Ok(out);
        }
        offset = offset.saturating_add(n);
    }
}

fn parent(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Compare the index against the disk under `root`. Pure apart from the walk.
pub fn compare(root: &Path, indexed: &Indexed, checked_at: u64) -> Stale {
    let mut parents: HashSet<&str> = HashSet::new();
    let mut walked: HashSet<&str> = HashSet::new();
    for rel in indexed.keys() {
        let mut dir = parent(rel);
        parents.insert(dir);
        while walked.insert(dir) && !dir.is_empty() {
            dir = parent(dir);
        }
    }
    let mut walk = Walk {
        indexed,
        parents: &parents,
        walked: &walked,
        seen: HashSet::new(),
        changed: Vec::new(),
        unindexed: Vec::new(),
    };
    if !indexed.is_empty() {
        walk.dir(root, "");
    }
    let mut removed = Vec::new();
    for rel in indexed.keys() {
        if !walk.seen.contains(rel.as_str()) && !root.join(rel).is_file() {
            removed.push(rel.clone());
        }
    }
    let (changed, unindexed) = (walk.changed.len() as u64, walk.unindexed.len() as u64);
    let removed_n = removed.len() as u64;
    let mut paths: Vec<String> = walk.changed.into_iter().chain(walk.unindexed).chain(removed).collect();
    paths.sort();
    paths.truncate(PATH_SAMPLE);
    Stale {
        count: changed + unindexed + removed_n,
        changed,
        unindexed,
        removed: removed_n,
        paths,
        checked_at,
    }
}

struct Walk<'a> {
    indexed: &'a Indexed,
    parents: &'a HashSet<&'a str>,
    walked: &'a HashSet<&'a str>,
    seen: HashSet<String>,
    changed: Vec<String>,
    unindexed: Vec<String>,
}

impl Walk<'_> {
    fn dir(&mut self, abs: &Path, rel: &str) {
        let Ok(entries) = std::fs::read_dir(abs) else { return };
        let notes_here = self.parents.contains(rel);
        for ent in entries.flatten() {
            let name = ent.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let child = if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") };
            let Ok(ft) = ent.file_type() else { continue };
            if ft.is_dir() {
                if self.walked.contains(child.as_str()) {
                    self.dir(&ent.path(), &child);
                }
            } else if ft.is_file() && notes_here && name.ends_with(".md") {
                match self.indexed.get(&child) {
                    None => self.unindexed.push(child.clone()),
                    Some(Some(at)) if disk_mtime(&ent).is_some_and(|m| m > at + MTIME_SLACK) => {
                        self.changed.push(child.clone());
                    }
                    Some(_) => {}
                }
                self.seen.insert(child);
            }
        }
    }
}

fn disk_mtime(ent: &std::fs::DirEntry) -> Option<f64> {
    let t = ent.metadata().ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Serialize, Deserialize)]
struct Cached {
    source: String,
    stale: Stale,
}

/// `$XDG_CACHE_HOME/lapis/stale-<vault hash>.json`, else under `~/.cache`.
/// Tests never touch the user's cache.
fn cache_file(root: &Path) -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let base = match std::env::var("XDG_CACHE_HOME") {
        Ok(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
        _ => std::env::home_dir()?.join(".cache"),
    };
    Some(base.join("lapis").join(format!("stale-{:016x}.json", fnv1a(root.to_string_lossy().as_bytes()))))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn load(file: &Path, source: &str, now: u64) -> Option<Stale> {
    let c: Cached = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let fresh = c.source == source && now.saturating_sub(c.stale.checked_at) < TTL.as_secs();
    fresh.then_some(c.stale)
}

fn store(file: &Path, source: &str, stale: &Stale) {
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(body) = serde_json::to_vec(&Cached { source: source.to_string(), stale: stale.clone() }) else {
        return;
    };
    // Write then rename, so a concurrent `search` never reads half a file.
    let tmp = file.with_extension(format!("json.{}", std::process::id()));
    if std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lapis-stale-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes/deep")).unwrap();
        std::fs::create_dir_all(d.join("excluded")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        d
    }

    fn mtime(root: &Path, rel: &str) -> f64 {
        std::fs::metadata(root.join(rel))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    #[test]
    fn counts_changed_unindexed_and_removed_notes() {
        let d = vault("kinds");
        for rel in
            ["notes/a.md", "notes/b.md", "notes/deep/c.md", "excluded/x.md", ".hidden/h.md", "notes/n.pdf"]
        {
            std::fs::write(d.join(rel), "# x\n").unwrap();
        }
        let mut idx = Indexed::new();
        idx.insert("notes/a.md".into(), Some(mtime(&d, "notes/a.md")));
        // indexed before its last edit
        idx.insert("notes/b.md".into(), Some(mtime(&d, "notes/b.md") - 5.0));
        idx.insert("notes/deep/c.md".into(), Some(mtime(&d, "notes/deep/c.md")));
        idx.insert("notes/gone.md".into(), Some(1.0));
        std::fs::write(d.join("notes/new.md"), "# new\n").unwrap();

        let s = compare(&d, &idx, 7);
        assert_eq!((s.changed, s.unindexed, s.removed, s.count), (1, 1, 1, 3), "{s:?}");
        assert_eq!(s.paths, ["notes/b.md", "notes/gone.md", "notes/new.md"]);
        assert_eq!(s.checked_at, 7);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_fresh_index_reports_nothing_and_an_empty_index_skips_the_walk() {
        let d = vault("fresh");
        std::fs::write(d.join("notes/a.md"), "# a\n").unwrap();
        let mut idx = Indexed::new();
        idx.insert("notes/a.md".into(), Some(mtime(&d, "notes/a.md")));
        assert_eq!(compare(&d, &idx, 0).count, 0);
        // No mtime from the backend: drift cannot be judged, so it is not counted.
        idx.insert("notes/a.md".into(), None);
        assert_eq!(compare(&d, &idx, 0).count, 0);
        // Nothing indexed yet: no folder holds an indexed note, so nothing is unindexed.
        assert_eq!(compare(&d, &Indexed::new(), 0).count, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn root_notes_count_when_the_root_holds_indexed_notes() {
        let d = vault("root");
        std::fs::write(d.join("Welcome.md"), "# w\n").unwrap();
        std::fs::write(d.join("Later.md"), "# l\n").unwrap();
        let mut idx = Indexed::new();
        idx.insert("Welcome.md".into(), Some(mtime(&d, "Welcome.md")));
        let s = compare(&d, &idx, 0);
        assert_eq!((s.unindexed, s.paths.as_slice()), (1, ["Later.md".to_string()].as_slice()));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_cache_is_per_backend_and_expires() {
        let dir = std::env::temp_dir().join(format!("lapis-stale-cache-{}", std::process::id()));
        let file = dir.join("lapis/stale-test.json");
        let s = Stale { count: 2, changed: 2, checked_at: 100, ..Stale::default() };
        store(&file, "http://lattice", &s);
        assert_eq!(load(&file, "http://lattice", 100 + TTL.as_secs() - 1), Some(s));
        assert_eq!(load(&file, "http://lattice", 100 + TTL.as_secs()), None, "expired");
        assert_eq!(load(&file, "/vault/.lapis/lattice.sqlite", 100), None, "another backend");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_search_count_lists_the_embedded_index() {
        let d = vault("embedded");
        std::fs::write(d.join("notes/a.md"), "# a\n").unwrap();
        let mut engine = lapis_lattice::Engine::open(&d).unwrap();
        engine.reindex().unwrap();
        let backend = Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(engine)));
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert_eq!(rt.block_on(cached(&backend, &d)).map(|s| s.count), Some(0));
        std::fs::write(d.join("notes/hand-written.md"), "# written by hand\n").unwrap();
        let s = rt.block_on(check(&backend, &d)).unwrap();
        assert_eq!((s.unindexed, s.paths.as_slice()), (1, ["notes/hand-written.md".to_string()].as_slice()));
        let _ = std::fs::remove_dir_all(&d);
    }
}
