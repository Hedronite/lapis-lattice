//! One call that brings the index up to date with the vault.
//!
//! [`refresh`] walks the vault through [`Scope`] — the same walk a full reindex
//! does — and compares every in-scope file with its row in `documents` by mtime
//! and size. Added files are indexed, changed files are re-chunked, and a row
//! whose file is gone, ignored, or now inside a nested vault is dropped. A run
//! with no diff writes nothing at all: edge resolution runs only when something
//! changed, and an unchanged file never touches its row.

use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::Result;

/// What a refresh did: added, updated and removed notes, and what the semantic
/// arm embedded this run (zero without an embedder).
#[derive(Debug, Clone, Serialize)]
pub struct RefreshReport {
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    /// Chunks given a vector this run. Zero without an embedder.
    pub embedded: u64,
    /// Why the semantic arm produced nothing. Refreshing still succeeded.
    pub embed_error: Option<String>,
}

/// Diff the vault against the index by mtime and size.
pub fn refresh(conn: &Connection, vault: &Path) -> Result<RefreshReport> {
    let found = crate::index::walk_checked(vault)?;
    let disk: HashMap<&str, &Path> = found.iter().map(|(r, a)| (r.as_str(), a.as_path())).collect();

    let mut stmt = conn.prepare("SELECT path, mtime, size FROM documents")?;
    let indexed: Vec<(String, i64, i64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                // A row from before the size column has none; -1 never matches a
                // disk size, so the row is refreshed once and then holds still.
                r.get::<_, Option<i64>>(2)?.unwrap_or(-1),
            ))
        })?
        .collect::<std::result::Result<Vec<_>, rusqlite::Error>>()?;
    drop(stmt);
    let in_index: HashSet<String> = indexed.iter().map(|(p, _, _)| p.clone()).collect();

    // The write phase is one transaction: it lands whole or not at all, so a
    // failure mid-walk can never leave a half-refreshed index behind. A clean
    // run issues no write statements, and a transaction that wrote nothing
    // commits no pages, so a second call still writes nothing.
    let tx = conn.unchecked_transaction()?;
    let mut added = 0u64;
    let mut updated = 0u64;
    let mut removed = 0u64;
    let mut changed = false;

    fn drop_row(conn: &Connection, rel: &str) -> Result<()> {
        crate::index::forget_path(conn, rel)?;
        // Links that pointed at the row's file become dangling again, exactly
        // as a full reindex would leave them.
        conn.execute("UPDATE edges SET dst_path = NULL, resolved = 0 WHERE dst_path = ?1", params![rel])?;
        Ok(())
    }

    for (rel, stored_mtime, stored_size) in &indexed {
        let abs = match disk.get(rel.as_str()) {
            Some(a) => *a,
            // Gone from disk, or out of scope now: a new ignore rule, a folder
            // that became its own vault, a link that now leaves the vault. The
            // walk already refused it, so the row goes.
            None => {
                drop_row(conn, rel)?;
                removed += 1;
                changed = true;
                continue;
            }
        };
        match stat_of(abs) {
            None => {
                drop_row(conn, rel)?;
                removed += 1;
                changed = true;
            }
            Some((mtime, size)) if *stored_mtime == mtime && *stored_size == size => {}
            // Changed: retired and re-chunked, the way a single-path reindex
            // does it, with the same chunking as a full reindex.
            Some(_) => {
                drop_row(conn, rel)?;
                crate::index::index_one(conn, abs, rel)?;
                updated += 1;
                changed = true;
            }
        }
    }
    for (rel, abs) in &found {
        if !in_index.contains(rel) {
            crate::index::index_one(conn, abs, rel)?;
            added += 1;
            changed = true;
        }
    }
    // Links only resolve when the file set or a body moved: a clean run must
    // not write, and `resolve_edges` only updates unresolved rows anyway. On a
    // first refresh of an unbuilt index the graph stamp still lands.
    if changed || crate::sqlite::meta_get(conn, "graph_built").as_deref() != Some("1") {
        let files: Vec<String> = found.iter().map(|(rel, _)| rel.clone()).collect();
        crate::index::resolve_edges(conn, &files)?;
        crate::sqlite::meta_set(conn, "graph_built", "1")?;
    }
    tx.commit()?;
    Ok(RefreshReport { added, updated, removed, embedded: 0, embed_error: None })
}

/// `(mtime in millis, size in bytes)` of one file. A vanished file returns
/// `None`, so a race between the walk and the stat drops the row instead of
/// failing the whole refresh.
fn stat_of(p: &Path) -> Option<(i64, i64)> {
    let m = fs::metadata(p).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some((mtime, m.len() as i64))
}

#[cfg(test)]
mod tests {
    use crate::{Engine, Error, ListParams};
    use std::path::PathBuf;
    use std::time::SystemTime;

    /// Unique per call, same shape as the other module tests.
    fn dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-refresh-{name}-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn vault() -> PathBuf {
        let d = dir("vault");
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(
            d.join("Welcome.md"),
            "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nSee [[Alpha]] if you must.\n",
        )
        .unwrap();
        std::fs::write(
            d.join("notes/Alpha.md"),
            "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nDahlia first, then back to [[Welcome]].\n",
        )
        .unwrap();
        d
    }

    /// A corrupt row is an error, never a silent drop (Vini G1 nits): a row
    /// `documents` cannot read must not pretend to be absent, or the diff pass
    /// would add or remove notes on the strength of garbage.
    #[test]
    fn an_unreadable_row_fails_the_refresh() {
        let v = vault();
        let mut e = Engine::open(&v).unwrap();
        e.reindex().unwrap();
        // A TEXT value inside the INTEGER size column: SQLite accepts it,
        // the row read cannot convert it, and the refresh must refuse rather
        // than silently pretend the row is absent. (mtime is NOT NULL, so
        // corruption lands here first.)
        e.conn.execute_batch("UPDATE documents SET size = 'not-a-number';").unwrap();
        match e.refresh() {
            Err(err) => {
                assert!(
                    matches!(err, Error::Sqlite(_)),
                    "expected the row read error to surface, got {err:?}"
                )
            }
            Ok(r) => panic!("a corrupt row was silently skipped: {r:?}"),
        }
        assert_eq!(
            e.health().unwrap().documents_indexed,
            2,
            "nothing was written while the read was refused"
        );
        let _ = std::fs::remove_dir_all(&v);
    }

    fn paths(e: &Engine, token: &str) -> Vec<String> {
        e.search(token, 10).unwrap().into_iter().map(|h| h.path).collect()
    }

    fn counts(r: &crate::RefreshReport) -> (u64, u64, u64) {
        (r.added, r.updated, r.removed)
    }

    /// G1a: add, edit and delete are caught, and search answers after each step.
    #[test]
    fn a_refresh_covers_add_edit_and_delete() {
        let v = vault();
        let mut e = Engine::open(&v).unwrap();
        e.reindex().unwrap();

        // Nothing moved yet: a clean diff, and the connection wrote no rows.
        let before = e.conn.total_changes();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 0), "a run with no changes does nothing");
        assert_eq!(e.conn.total_changes(), before, "a no-op refresh writes no rows");
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 0));

        // Added.
        std::fs::write(v.join("notes/Beta.md"), "# Beta\n\nTangerine grove.\n").unwrap();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (1, 0, 0), "the new note is added: {r:?}");
        assert_eq!(paths(&e, "tangerine"), ["notes/Beta.md"], "search answers for the added note");
        assert_eq!(e.health().unwrap().stale_documents, 0, "nothing is stale after a refresh");

        // Edited: a new body word, same path.
        std::fs::write(
            v.join("Welcome.md"),
            "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nA kiwi arrived.\n",
        )
        .unwrap();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 1, 0), "the edited note is updated: {r:?}");
        assert_eq!(paths(&e, "kiwi"), ["Welcome.md"], "search answers for the edited note");
        assert!(paths(&e, "if you must").is_empty(), "the old body is gone from the index");

        // Deleted.
        std::fs::remove_file(v.join("notes/Alpha.md")).unwrap();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 1), "the deleted note is removed: {r:?}");
        assert!(paths(&e, "dahlia").is_empty(), "search no longer answers for the deleted note");
        assert_eq!(e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap().len(), 2);
        assert_eq!(e.health().unwrap().stale_documents, 0);

        // Still quiet after the storm: nothing left to catch.
        let before = e.conn.total_changes();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 0));
        assert_eq!(e.conn.total_changes(), before);
        let _ = std::fs::remove_dir_all(&v);
    }

    /// G1b: ignore rules and nested vaults hold, and a clean refresh is silent.
    #[test]
    fn a_refresh_honours_scope_and_stays_silent_without_changes() {
        let v = vault();
        std::fs::create_dir_all(v.join("gen")).unwrap();
        std::fs::write(v.join("gen/Quartz.md"), "# Quartz\n\nA generated file.\n").unwrap();
        std::fs::create_dir_all(v.join("fixtures/run1/vault/.lapis")).unwrap();
        std::fs::write(v.join("fixtures/run1/vault/Note.md"), "# Note\n\nAmber hoard.\n").unwrap();

        let mut e = Engine::open(&v).unwrap();
        e.reindex().unwrap();
        assert_eq!(
            paths(&e, "quartz"),
            ["gen/Quartz.md"],
            "generated notes are indexed before the rule lands"
        );
        assert!(paths(&e, "amber").is_empty(), "a nested vault is never walked");

        // A new ignore rule puts the indexed file out of scope: dropped.
        std::fs::write(v.join(".lapisignore"), "gen/\n").unwrap();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 1), "the excluded row is removed: {r:?}");
        assert!(paths(&e, "quartz").is_empty(), "the ignored file is no longer searchable");
        assert_eq!(e.health().unwrap().stale_documents, 0, "an excluded file is not stale, it is gone");
        assert!(paths(&e, "amber").is_empty(), "the nested vault still keeps its own index");

        // Quiet again: the diff is clean, and no row is written.
        let before = e.conn.total_changes();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 0));
        assert_eq!(e.conn.total_changes(), before, "the silent refresh wrote no rows");
        let _ = std::fs::remove_dir_all(&v);
    }

    /// Rows from before the size column have none: the first refresh fills them
    /// in and the second one holds still.
    #[test]
    fn old_rows_without_a_size_refresh_once_then_stay_still() {
        let v = vault();
        let mut e = Engine::open(&v).unwrap();
        e.reindex().unwrap();
        // A pre-Pendant index: mtime and hash, no size.
        e.conn.execute_batch("UPDATE documents SET size = NULL;").unwrap();

        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 2, 0), "every row is refreshed exactly once: {r:?}");
        assert!(!paths(&e, "dahlia").is_empty(), "the index still answers after the fill-in");

        let before = e.conn.total_changes();
        let r = e.refresh().unwrap();
        assert_eq!(counts(&r), (0, 0, 0));
        assert_eq!(e.conn.total_changes(), before, "no further writes once size is known");
        let _ = std::fs::remove_dir_all(&v);
    }
}
