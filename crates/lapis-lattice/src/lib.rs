//! Embedded SQLite + FTS5 lattice for a Markdown vault.
//!
//! Files on disk are the write source of truth. This crate only writes
//! `<vault>/.lapis/lattice.sqlite`. Search is FTS5 BM25; vectors are not in 0.2.
//!
//! The database is stamped `producer = "lapis-lattice"` at creation and the
//! stamp is asserted on open, so a path pointed at a foreign SQLite file (an
//! agent-memory store, say) fails loudly instead of quietly mixing corpora.

mod analytics;
mod graph;
mod index;
mod sqlite;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

pub use analytics::{ANALYTICS_QUERIES, Analytics};
pub use graph::Neighbor;

/// Written into `meta.producer` at creation; asserted on open.
pub const PRODUCER: &str = "lapis-lattice";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Open or create `<vault>/.lapis/lattice.sqlite`.
pub struct Engine {
    vault: PathBuf,
    conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub path: String,
    pub title: Option<String>,
    pub heading: Option<String>,
    pub snippet: Option<String>,
    pub rank: u32,
    pub score: f64,
    pub domain: Option<String>,
    pub doc_type: Option<String>,
}

/// Retrieval arm selection. The embedded engine is FTS-only in 0.2, so
/// `Hybrid` runs BM25 alone and reports that in [`SearchResult::modalities`];
/// `Vector` is refused rather than silently downgraded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Hybrid,
    Bm25,
    Vector,
}

/// Everything the CLI can ask of search. Flags must not silently no-op.
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub query: String,
    pub limit: u32,
    pub offset: u32,
    pub domain: Option<String>,
    /// Collapse to the best-scoring chunk per document.
    pub per_doc: bool,
    pub mode: Mode,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    /// Arms that actually ran. FTS-only today, so `["bm25"]`.
    pub modalities: Vec<String>,
}

/// Filters for the documents table (`lapis list`).
#[derive(Debug, Clone, Default)]
pub struct ListParams {
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// One row of the documents table. Wire casing belongs to the caller.
#[derive(Debug, Clone, Serialize)]
pub struct Document {
    pub path: String,
    pub kind: String,
    pub title: Option<String>,
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub tags: Vec<String>,
    /// File mtime in milliseconds.
    pub updated_at: Option<u64>,
    /// Content fingerprint, so an agent can plan `--if-hash` writes from one list call.
    pub hash: Option<String>,
}

/// Link-graph state. `built` is false before the first reindex, so "no edges
/// yet" is distinguishable from "a vault with no links".
#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub built: bool,
    pub edges: u64,
    pub dangling_links: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub status: String,
    pub documents_indexed: u64,
    pub db_path: String,
    pub embedder: &'static str,
    pub embed_model: Option<String>,
    pub embed_dim: Option<u32>,
    pub graph: Graph,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexReport {
    pub documents: u64,
    pub chunks: u64,
    pub edges: u64,
}

impl Engine {
    pub fn open(vault: impl AsRef<Path>) -> Result<Self> {
        let vault = vault.as_ref().canonicalize().unwrap_or_else(|_| vault.as_ref().to_path_buf());
        if !vault.is_dir() {
            return Err(Error::Usage(format!("vault is not a directory: {}", vault.display())));
        }
        let dir = vault.join(".lapis");
        std::fs::create_dir_all(&dir)?;
        let db = dir.join("lattice.sqlite");
        let conn = Connection::open(&db)?;
        sqlite::migrate(&conn)?;
        sqlite::assert_producer(&conn, &db)?;
        Ok(Self { vault, conn })
    }

    pub fn db_path(&self) -> PathBuf {
        self.vault.join(".lapis/lattice.sqlite")
    }

    /// Full-vault reindex. Use [`Engine::reindex_path`] after a single write.
    pub fn reindex(&mut self) -> Result<IndexReport> {
        index::reindex(&self.conn, &self.vault)
    }

    /// Reindex exactly one vault-relative path, in place. A removed file is
    /// dropped from the index. Costs one file read, not a vault walk.
    pub fn reindex_path(&mut self, rel: &str) -> Result<IndexReport> {
        index::reindex_path(&self.conn, &self.vault, rel)
    }

    /// Convenience wrapper kept for the 0.1 API.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Hit>> {
        Ok(self.search_with(&SearchParams { query: query.to_string(), limit, ..Default::default() })?.hits)
    }

    pub fn search_with(&self, p: &SearchParams) -> Result<SearchResult> {
        sqlite::search(&self.conn, p)
    }

    /// `lapis list`: the documents table, filtered and paged.
    pub fn documents(&self, p: &ListParams) -> Result<Vec<Document>> {
        sqlite::documents(&self.conn, p)
    }

    pub fn neighbors(&self, path: &str, direction: &str) -> Result<Vec<Neighbor>> {
        graph::neighbors(&self.conn, path, direction)
    }

    pub fn health(&self) -> Result<Health> {
        sqlite::health(&self.conn, &self.db_path())
    }

    pub fn analytics(&self, query: &str) -> Result<Analytics> {
        analytics::run(&self.conn, query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique per call. `as_nanos()` alone is not: macOS clock granularity is
    /// coarser than a nanosecond, so two tests starting together get the same
    /// value, share a directory, and clobber each other's fixtures. Tests in a
    /// crate run in parallel threads, so pid does not separate them either —
    /// hence the counter.
    fn vault() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-lattice-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(
            d.join("Welcome.md"),
            "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nSee [[Alpha]] and [[Missing]].\n",
        )
        .unwrap();
        std::fs::write(
            d.join("notes/Alpha.md"),
            "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nBack to [[Welcome]].\n\n- [ ] a task\n",
        )
        .unwrap();
        d
    }

    #[test]
    fn index_search_graph_analytics() {
        let d = vault();
        let mut e = Engine::open(&d).unwrap();
        let r = e.reindex().unwrap();
        assert_eq!(r.documents, 2);
        assert!(r.chunks >= 2);
        assert!(r.edges >= 3, "Welcome→Alpha, Welcome→Missing, Alpha→Welcome");

        let hits = e.search("welcome", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].path, "Welcome.md");
        assert_eq!(hits[0].rank, 1);

        let both = e.neighbors("Welcome.md", "both").unwrap();
        assert!(both.iter().any(|n| n.dst_raw == "Alpha" && n.resolved));
        assert!(both.iter().any(|n| n.dst_raw == "Missing" && !n.resolved && n.path.is_none()));

        let h = e.health().unwrap();
        assert_eq!(h.documents_indexed, 2);
        assert!(h.graph.built, "reindex stamps graph_built");
        assert!(h.graph.dangling_links >= 1);
        assert_eq!(h.embedder, "none");
        assert_eq!(h.status, "ok");

        let tags = e.analytics("tags").unwrap();
        assert_eq!(tags.query, "tags");
        assert!(tags.rows.iter().any(|row| row.iter().any(|c| c.contains("intro"))));

        assert!(e.analytics("drop table documents").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// B3: every CLI flag reaches the engine. None of them may silently no-op.
    #[test]
    fn search_params_are_honoured() {
        let d = vault();
        let mut e = Engine::open(&d).unwrap();
        e.reindex().unwrap();

        // domain is the first path segment; Alpha.md lives under notes/
        let scoped = e
            .search_with(&SearchParams {
                query: "alpha".into(),
                limit: 10,
                domain: Some("notes".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(scoped.hits.iter().all(|h| h.path.starts_with("notes/")));
        let missed = e
            .search_with(&SearchParams {
                query: "alpha".into(),
                limit: 10,
                domain: Some("nope".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(missed.hits.is_empty(), "domain filter actually filters");

        // per_doc collapses to one row per path
        let all = e
            .search_with(&SearchParams { query: "welcome".into(), limit: 50, ..Default::default() })
            .unwrap();
        let collapsed = e
            .search_with(&SearchParams {
                query: "welcome".into(),
                limit: 50,
                per_doc: true,
                ..Default::default()
            })
            .unwrap();
        let mut paths: Vec<&str> = collapsed.hits.iter().map(|h| h.path.as_str()).collect();
        let before = paths.len();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), before, "per_doc leaves no duplicate paths");
        assert!(collapsed.hits.len() <= all.hits.len());

        // offset pages, and rank stays absolute across pages
        let page1 =
            e.search_with(&SearchParams { query: "welcome".into(), limit: 1, ..Default::default() }).unwrap();
        let page2 = e
            .search_with(&SearchParams { query: "welcome".into(), limit: 1, offset: 1, ..Default::default() })
            .unwrap();
        assert_eq!(page1.hits[0].rank, 1);
        if let Some(h) = page2.hits.first() {
            assert_eq!(h.rank, 2, "offset keeps ranks absolute");
            assert_ne!(h.path.as_str(), page1.hits[0].path.as_str());
        }

        // hybrid runs BM25 alone and says so; vector is refused, not downgraded
        assert_eq!(page1.modalities, ["bm25"]);
        let v = e.search_with(&SearchParams {
            query: "welcome".into(),
            limit: 5,
            mode: Mode::Vector,
            ..Default::default()
        });
        assert!(matches!(v, Err(Error::Usage(m)) if m.contains("FTS-only")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// B10: documents query with filters and paging, carrying hash.
    #[test]
    fn documents_filter_and_page() {
        let d = vault();
        let mut e = Engine::open(&d).unwrap();
        e.reindex().unwrap();

        let all = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|r| r.hash.is_some()), "hash lets an agent plan --if-hash writes");
        assert!(all.iter().all(|r| r.updated_at.is_some()));
        assert_eq!(all[0].kind, "markdown");

        let scoped = e
            .documents(&ListParams { prefix: Some("notes/".into()), limit: 50, ..Default::default() })
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].path, "notes/Alpha.md");
        assert_eq!(scoped[0].priority.as_deref(), Some("high"));

        let tagged =
            e.documents(&ListParams { tag: Some("graph".into()), limit: 50, ..Default::default() }).unwrap();
        assert_eq!(tagged.len(), 1);
        assert_eq!(
            e.documents(&ListParams { tag: Some("nope".into()), limit: 50, ..Default::default() })
                .unwrap()
                .len(),
            0
        );

        let page = e.documents(&ListParams { limit: 1, offset: 1, ..Default::default() }).unwrap();
        assert_eq!(page.len(), 1);
        assert_ne!(page[0].path, all[0].path);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// B4: one file in, one file out, without a vault walk re-reading everything.
    #[test]
    fn reindex_path_is_incremental() {
        let d = vault();
        let mut e = Engine::open(&d).unwrap();
        e.reindex().unwrap();
        assert!(e.search("kumquat", 10).unwrap().is_empty());

        // append to an existing note: the new word becomes searchable
        std::fs::write(
            d.join("notes/Alpha.md"),
            "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nkumquat harvest.\n",
        )
        .unwrap();
        let r = e.reindex_path("notes/Alpha.md").unwrap();
        assert_eq!(r.documents, 1);
        let hits = e.search("kumquat", 10).unwrap();
        assert_eq!(hits.len(), 1, "FTS rows were replaced, not duplicated");
        assert_eq!(hits[0].path, "notes/Alpha.md");
        assert_eq!(e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap().len(), 2);

        // a brand-new note resolves someone else's dangling link
        std::fs::write(d.join("Missing.md"), "---\nname: Missing\n---\n# Missing\n").unwrap();
        e.reindex_path("Missing.md").unwrap();
        let out = e.neighbors("Welcome.md", "out").unwrap();
        assert!(
            out.iter().any(|n| n.dst_raw == "Missing" && n.resolved),
            "new file resolves an existing dangling edge"
        );

        // deleting the file drops it from the index
        std::fs::remove_file(d.join("Missing.md")).unwrap();
        let r = e.reindex_path("Missing.md").unwrap();
        assert_eq!(r.documents, 0);
        assert!(
            e.documents(&ListParams { limit: 50, ..Default::default() })
                .unwrap()
                .iter()
                .all(|x| x.path != "Missing.md")
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The producer stamp is what keeps this file from ever being an agent-memory store.
    #[test]
    fn producer_stamp_refuses_a_foreign_database() {
        let d = vault();
        {
            let e = Engine::open(&d).unwrap();
            assert!(e.db_path().exists());
        }
        let db = d.join(".lapis/lattice.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('producer','mnemopi')", []).unwrap();
        drop(conn);
        match Engine::open(&d) {
            Err(Error::Usage(m)) => assert!(m.contains("mnemopi") && m.contains("foreign")),
            Err(e) => panic!("expected a usage refusal, got {e:?}"),
            Ok(_) => panic!("expected a refusal, opened a foreign database"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
