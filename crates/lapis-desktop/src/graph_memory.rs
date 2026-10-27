//! The board's memory: where every note sat when the layout last came to rest,
//! and where the operator pinned notes, kept per vault in the cache directory.
//!
//! It is a cache, never the vault: a missing, stale or unreadable file only
//! means the next open lays the graph out from scratch.
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bumped when the file shape or the layout physics changes; any other version
/// is ignored, so a map laid out by older forces is not reopened. 2: compact,
/// link-ordered start, so hubs gather their notes.
pub const VERSION: u32 = 2;

/// The layout starts warm only when the memory and the graph mostly agree: at
/// least this share of the graph's notes is remembered, and at least this
/// share of the remembered notes is still in the graph. A vault that lost half
/// its notes (say, to a new ignore rule) gets a fresh map, not the old one
/// with holes in it.
pub const WARM_COVERAGE: f32 = 0.5;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Remembered {
    pub version: u32,
    /// Node id to world position, as the layout last settled it.
    pub nodes: HashMap<String, [f32; 2]>,
    /// Node id to where the operator dropped it.
    pub pins: BTreeMap<String, [f32; 2]>,
}

impl Remembered {
    pub fn new(nodes: HashMap<String, [f32; 2]>, pins: BTreeMap<String, [f32; 2]>) -> Self {
        Self { version: VERSION, nodes, pins }
    }

    /// Share of `ids` this memory has a place for.
    pub fn coverage<'a>(&self, ids: impl ExactSizeIterator<Item = &'a str>) -> f32 {
        let total = ids.len();
        if total == 0 {
            return 0.0;
        }
        ids.filter(|id| self.nodes.contains_key(*id)).count() as f32 / total as f32
    }

    /// True when this memory fits a graph of `ids` well enough to start warm:
    /// see [`WARM_COVERAGE`].
    pub fn fits<'a>(&self, ids: impl ExactSizeIterator<Item = &'a str>) -> bool {
        let total = ids.len();
        if total == 0 || self.nodes.is_empty() {
            return false;
        }
        let known = ids.filter(|id| self.nodes.contains_key(*id)).count() as f32;
        known / total as f32 >= WARM_COVERAGE && known / self.nodes.len() as f32 >= WARM_COVERAGE
    }
}

/// `$XDG_CACHE_HOME/lapis/graph`, else `~/.cache/lapis/graph`.
pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("lapis").join("graph"))
}

/// The memory file for `vault` inside `dir`: one per vault, named by a hash of
/// its canonical path so two vaults never share one and no path is spelled out.
pub fn file_for(dir: &Path, vault: &Path) -> PathBuf {
    let canon = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canon.to_string_lossy().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    dir.join(format!("{h:016x}.json"))
}

/// What `dir` remembers for `vault`, if anything usable.
pub fn load_from(dir: &Path, vault: &Path) -> Option<Remembered> {
    let text = std::fs::read_to_string(file_for(dir, vault)).ok()?;
    serde_json::from_str::<Remembered>(&text).ok().filter(|r| r.version == VERSION)
}

/// Write `memory` for `vault` into `dir`: a temporary file, then a rename, so a
/// reader never sees half a file.
pub fn save_to(dir: &Path, vault: &Path, memory: &Remembered) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = file_for(dir, vault);
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec(memory).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &path)
}

/// [`load_from`] the default cache directory.
pub fn load(vault: &Path) -> Option<Remembered> {
    load_from(&cache_dir()?, vault)
}

/// [`save_to`] the default cache directory.
pub fn save(vault: &Path, memory: &Remembered) -> std::io::Result<()> {
    let dir = cache_dir().ok_or_else(|| std::io::Error::other("no cache directory"))?;
    save_to(&dir, vault, memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("lapis-memory-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("vault")).unwrap();
        (base.join("cache"), base.join("vault"))
    }

    #[test]
    fn a_saved_layout_comes_back_and_a_missing_one_is_none() {
        let (cache, vault) = dirs("roundtrip");
        assert_eq!(load_from(&cache, &vault), None, "nothing remembered yet");
        let mut nodes = HashMap::new();
        nodes.insert("notes/a.md".to_string(), [1.5, -2.0]);
        let mut pins = BTreeMap::new();
        pins.insert("notes/a.md".to_string(), [10.0, 10.0]);
        let memory = Remembered::new(nodes, pins);
        save_to(&cache, &vault, &memory).unwrap();
        assert_eq!(load_from(&cache, &vault), Some(memory));
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn a_damaged_or_foreign_file_is_ignored() {
        let (cache, vault) = dirs("damaged");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(file_for(&cache, &vault), b"{not json").unwrap();
        assert_eq!(load_from(&cache, &vault), None);
        std::fs::write(file_for(&cache, &vault), br#"{"version":99,"nodes":{},"pins":{}}"#).unwrap();
        assert_eq!(load_from(&cache, &vault), None, "another version is not read");
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }

    #[test]
    fn each_vault_has_its_own_file_and_coverage_counts_known_ids() {
        let (cache, vault) = dirs("files");
        let other = vault.with_file_name("other");
        std::fs::create_dir_all(&other).unwrap();
        assert_ne!(file_for(&cache, &vault), file_for(&cache, &other));
        let mut nodes = HashMap::new();
        nodes.insert("a".to_string(), [0.0, 0.0]);
        let memory = Remembered::new(nodes, BTreeMap::new());
        assert_eq!(memory.coverage(["a", "b"].into_iter()), 0.5);
        assert_eq!(memory.coverage(std::iter::empty()), 0.0);
        assert!(memory.fits(["a", "b"].into_iter()), "half the graph remembered, all the memory present");
        assert!(!memory.fits(["a", "b", "c"].into_iter()), "too little of the graph remembered");
        let mut big = memory.clone();
        for k in ["b", "c", "d"] {
            big.nodes.insert(k.to_string(), [0.0, 0.0]);
        }
        assert!(big.fits(["a", "b"].into_iter()), "half the memory still in the graph");
        assert!(!big.fits(["a"].into_iter()), "the graph shrank to a quarter of the memory");
        assert!(!Remembered::default().fits(["a"].into_iter()));
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }
}
