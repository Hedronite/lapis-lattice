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
/// Most bytes a memory file is read from, and most notes it may hold: about
/// 90 bytes a note, so room for a vault several times the snapshot cap.
const READ_CAP: u64 = 32 << 20;
const NODE_CAP: usize = 200_000;

/// A memory file nobody has written for this long belongs to a vault that was
/// removed, renamed or long unopened. Files are named by a hash, never a path,
/// so age is how a gone vault is told apart; reopening one only costs a cold layout.
pub const FORGET_AFTER: std::time::Duration = std::time::Duration::from_secs(90 * 24 * 3600);
/// A temporary file this old was left by a save that died part way.
const TMP_FORGET_AFTER: std::time::Duration = std::time::Duration::from_secs(3600);

/// Remove memory files in `dir` not written for `age`, and temporary files a
/// failed save left behind. Only names this module writes are touched, and only
/// plain files (never a link). Returns how many went.
pub fn prune(dir: &Path, age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some((stem, rest)) = name.split_once('.') else { continue };
        if stem.len() != 16 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let limit = match rest {
            "json" => age,
            _ if rest.starts_with("json.") && rest.ends_with(".tmp") => TMP_FORGET_AFTER,
            _ => continue,
        };
        let Ok(meta) = entry.path().symlink_metadata() else { continue };
        let idle = meta.modified().ok().and_then(|m| now.duration_since(m).ok());
        if meta.is_file()
            && idle.is_some_and(|idle| idle > limit)
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

pub fn load_from(dir: &Path, vault: &Path) -> Option<Remembered> {
    let file = file_for(dir, vault);
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > READ_CAP {
        return None;
    }
    let text = std::fs::read_to_string(&file).ok()?;
    serde_json::from_str::<Remembered>(&text)
        .ok()
        .filter(|r| r.version == VERSION && r.nodes.len() <= NODE_CAP && r.pins.len() <= NODE_CAP)
}

/// Write `memory` for `vault` into `dir`. The file holds note paths, so the
/// folder is created 0700 and the file 0600, through a temporary file created
/// exclusively (a planted link is never written through) and renamed into
/// place, so a reader never sees half a file. A vault with no path is never
/// saved: every such vault would share one file.
pub fn save_to(dir: &Path, vault: &Path, memory: &Remembered) -> std::io::Result<()> {
    use std::io::Write;
    if vault.as_os_str().is_empty() {
        return Err(std::io::Error::other("no vault path to remember the layout under"));
    }
    let mut folder = std::fs::DirBuilder::new();
    folder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut folder, 0o700);
    folder.create(dir)?;
    let path = file_for(dir, vault);
    let nanos =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let tmp = path.with_extension(format!("json.{}.{nanos}.tmp", std::process::id()));
    let body = serde_json::to_vec(memory).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let placed =
        options.open(&tmp).and_then(|mut f| f.write_all(&body)).and_then(|()| std::fs::rename(&tmp, &path));
    if placed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    prune(dir, FORGET_AFTER);
    placed
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
    use std::time::{Duration, SystemTime};

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
    fn the_memory_file_is_owner_only_and_needs_a_vault_path() {
        let (cache, vault) = dirs("private");
        save_to(&cache, &vault, &Remembered::new(HashMap::new(), BTreeMap::new())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&file_for(&cache, &vault)), 0o600);
            assert_eq!(mode(&cache), 0o700);
        }
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "no temporary file left behind");
        assert!(save_to(&cache, Path::new(""), &Remembered::default()).is_err(), "no shared empty-path file");
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

    /// G-4: a vault that is gone stops leaving its note paths in the cache.
    #[test]
    fn a_save_forgets_long_idle_layouts_and_dead_temp_files_only() {
        let (cache, vault) = dirs("prune");
        save_to(&cache, &vault, &Remembered::new(Default::default(), Default::default())).unwrap();
        let mine = file_for(&cache, &vault);
        let old = SystemTime::now() - FORGET_AFTER - Duration::from_secs(60);
        let place = |name: &str, when: SystemTime| {
            let path = cache.join(name);
            std::fs::write(&path, "{}").unwrap();
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
            path
        };
        let gone = place("00000000000000aa.json", old);
        let recent = place("00000000000000bb.json", SystemTime::now() - Duration::from_secs(86_400));
        let dead_tmp = place("00000000000000cc.json.42.7.tmp", SystemTime::now() - Duration::from_secs(7200));
        let live_tmp = place("00000000000000dd.json.42.8.tmp", SystemTime::now());
        let foreign = place("notes.json", old);
        let odd = place("not-a-hash-here!.json", old);
        assert_eq!(prune(&cache, FORGET_AFTER), 2);
        for p in [&gone, &dead_tmp] {
            assert!(!p.exists(), "{} should be forgotten", p.display());
        }
        for p in [&mine, &recent, &live_tmp, &foreign, &odd] {
            assert!(p.exists(), "{} should stay", p.display());
        }
        #[cfg(unix)]
        {
            let target = place("00000000000000ee.json", old);
            let link = cache.join("00000000000000ff.json");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            std::fs::remove_file(&target).unwrap();
            prune(&cache, FORGET_AFTER);
            assert!(link.symlink_metadata().is_ok(), "a link is never followed or removed");
        }
        // A save prunes on the way out.
        let again = place("00000000000000aa.json", old);
        save_to(&cache, &vault, &Remembered::new(Default::default(), Default::default())).unwrap();
        assert!(!again.exists());
        let _ = std::fs::remove_dir_all(cache.parent().unwrap());
    }
}
