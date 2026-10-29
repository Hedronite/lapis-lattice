//! Where the index lives.
//!
//! The default is `<vault>/.lapis/`. [`Engine::open_in`] points the engine at a
//! caller-chosen directory instead, so a host that may not write the vault — a
//! phone app — still opens, builds, searches and reindexes it, and the vault
//! tree gains no file. Helpers that find the index go through [`db_file`] and
//! [`DEFAULT_DIR`] so the chosen directory and the default cannot drift apart.

use std::path::{Path, PathBuf};

/// The vault-relative directory of the default index, `<vault>/.lapis/`.
pub const DEFAULT_DIR: &str = ".lapis";

/// The SQLite file an index directory holds.
pub fn db_file(index_dir: &Path) -> PathBuf {
    index_dir.join("lattice.sqlite")
}

/// Create `dir` for a caller-chosen index, when missing, as `0700` on unix:
/// the index carries every note's text. Directories this crate creates on the
/// way are `0700` too; a directory that already exists keeps its mode, and
/// nothing else is touched. Windows keeps its default ACL.
pub fn create_dir_private(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        return Ok(());
    }
    #[allow(unused_mut)]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.recursive(true).create(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Embedder, Engine, Result};
    use std::path::Path as P;

    /// Unique per call, same shape as the lib tests: nanos plus a counter.
    fn dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-location-{name}-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn vault() -> PathBuf {
        let d = dir("vault");
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(
            d.join("Welcome.md"),
            "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nSee [[Alpha]].\n",
        )
        .unwrap();
        std::fs::write(
            d.join("notes/Alpha.md"),
            "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nBack to [[Welcome]].\n",
        )
        .unwrap();
        d
    }

    /// Every file with its bytes, sorted, so an added, removed or changed file
    /// — a `.lapis` directory included — shows up in the comparison.
    fn tree(root: &P) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &P, rel: &P, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let name = rel.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    walk(&entry.path(), &name, out);
                } else {
                    out.push((name.to_string_lossy().into_owned(), std::fs::read(entry.path()).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, P::new(""), &mut out);
        out.sort();
        out
    }

    struct Stub;

    impl Embedder for Stub {
        fn model(&self) -> &str {
            "stub-v1"
        }
        fn dim(&self) -> usize {
            4
        }
        fn provider(&self) -> &'static str {
            "onnx"
        }
        fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0, 0.0, 0.0]).collect())
        }
        fn embed_query(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![1.0, 1.0, 0.0, 0.0])
        }
    }

    /// G0a: the index builds, searches and reindexes in a caller-chosen
    /// directory, and the vault tree gains no file.
    #[test]
    fn an_index_outside_the_vault_never_touches_the_tree() {
        let v = vault();
        let idx = dir("index");
        let before = tree(&v);

        let mut e = Engine::open_in(&v, &idx, None).unwrap();
        let r = e.reindex().unwrap();
        assert_eq!(r.documents, 2, "builds the index outside the vault");
        assert!(!e.search("alpha", 5).unwrap().is_empty(), "searches over it");
        assert_eq!(e.db_path(), idx.join("lattice.sqlite"), "the db lives where asked");
        assert_eq!(e.reindex_path("notes/Alpha.md").unwrap().documents, 1, "single-path reindex works too");
        drop(e);
        assert_eq!(tree(&v), before, "the vault tree gains no file");

        // A reopen finds the stored index, and the stored-embedder lookup
        // honours the directory instead of falling back to the vault's `.lapis`.
        let mut e = Engine::open_in(&v, &idx, Some(std::sync::Arc::new(Stub))).unwrap();
        assert!(e.reindex().unwrap().embedded >= 1, "vectors land in the outside index");
        drop(e);
        let stored = Engine::stored_embedder_in(&idx).unwrap();
        assert_eq!(stored.model, "stub-v1");
        assert_eq!(stored.dim, 4);
        let conn = rusqlite::Connection::open(idx.join("lattice.sqlite")).unwrap();
        assert_eq!(crate::sqlite::meta_get(&conn, "embedder").unwrap(), "onnx".to_string());
        assert!(Engine::stored_embedder(&v).is_none(), "no index inside the vault, so nothing stored there");
        assert_eq!(tree(&v), before, "still nothing in the vault, embedder or not");
        let _ = std::fs::remove_dir_all(&v);
        let _ = std::fs::remove_dir_all(&idx);
    }

    /// The caller-chosen directory is created `0700` on unix, parents
    /// included; an existing directory keeps its mode.
    #[cfg(unix)]
    #[test]
    fn open_in_creates_a_private_index_dir() {
        use std::os::unix::fs::PermissionsExt;
        let v = vault();
        let idx = std::env::temp_dir().join("lapis-engine-mobile-private");
        let _ = std::fs::remove_dir_all(&idx);
        // A shared base that predates the engine stays as it is.
        std::fs::create_dir_all(&idx).unwrap();
        std::fs::set_permissions(&idx, std::fs::Permissions::from_mode(0o750)).unwrap();
        Engine::open_in(&v, &idx, None).unwrap();
        let mode = idx.metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750, "an existing directory keeps its mode");
        // A fresh chain: the leaf and the created parent are both private.
        let fresh = idx.join("new/deeper/index");
        Engine::open_in(&v, &fresh, None).unwrap();
        for d in [idx.join("new"), idx.join("new/deeper"), idx.join("new/deeper/index")] {
            let mode = d.metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "created directory is private: {}", d.display());
        }
        // The default path is untouched by the private helper: an existing
        // `.lapis` keeps its mode through `Engine::open`.
        let v2 = vault();
        std::fs::create_dir_all(v2.join(".lapis")).unwrap();
        std::fs::set_permissions(v2.join(".lapis"), std::fs::Permissions::from_mode(0o750)).unwrap();
        Engine::open(&v2).unwrap();
        let mode = v2.join(".lapis").metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o750, "the default path keeps its mode");
        for d in [&v, &v2] {
            let _ = std::fs::remove_dir_all(d);
        }
        let _ = std::fs::remove_dir_all(&idx);
    }

    /// G0b: the default callers keep the default path.
    #[test]
    fn the_default_open_still_uses_the_vaults_lapis_dir() {
        let v = vault();
        let v = v.canonicalize().unwrap();
        let e = Engine::open(&v).unwrap();
        assert_eq!(e.db_path(), v.join(".lapis/lattice.sqlite"));
        assert!(e.db_path().exists(), "open built the default index");
        drop(e);
        let e = Engine::open_with(&v, None).unwrap();
        assert_eq!(e.db_path(), v.join(".lapis/lattice.sqlite"), "open_with agrees");
        assert!(Engine::stored_embedder(&v).is_none(), "no embedder, nothing stored");
        assert_eq!(
            Engine::stored_embedder_in(e.db_path().parent().unwrap()),
            None,
            "the default lookup through the directory agrees too"
        );
        assert!(
            tree(&v).iter().any(|(p, _)| p.starts_with(".lapis/")),
            "the default index is inside the vault"
        );
        let _ = std::fs::remove_dir_all(&v);
    }
}
