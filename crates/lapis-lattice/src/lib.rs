//! Embedded SQLite + FTS5 lattice for a Markdown vault.
//!
//! Files on disk are the write source of truth. This crate only writes
//! `<vault>/.lapis/lattice.sqlite`. Search is FTS5 BM25; vectors are not in 0.2.
//!
//! The database is stamped `producer = "lapis-lattice"` at creation and the
//! stamp is asserted on open, so a path pointed at a foreign SQLite file (an
//! agent-memory store, say) fails loudly instead of quietly mixing corpora.

mod analytics;
mod embed;
mod graph;
mod index;
mod kinds;
mod sqlite;

#[cfg(test)]
mod search_ids;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub use analytics::{ANALYTICS_QUERIES, Analytics};
pub use embed::Embedder;
pub use graph::{GraphEdge, GraphNode, GraphSnapshot, Neighbor, SNAPSHOT_NODE_CAP};
pub use kinds::{HTML, MARKDOWN, YAML, kind_for};

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
    embedder: Option<std::sync::Arc<dyn Embedder>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub path: String,
    /// `markdown` | `html` | `yaml`, as recorded at index time rather than
    /// re-derived from the extension by every consumer.
    pub kind: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub rank: u32,
    pub score: f64,
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i64>,
}

/// Retrieval arm selection. The embedded engine is FTS-only in 0.2, so
/// `Hybrid` runs BM25 alone and reports that in [`SearchResult::modalities`];
/// `Vector` is refused rather than silently downgraded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Hybrid,
    Bm25,
    Vector,
}

/// Everything a surface can ask of search. Flags must not silently no-op.
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub query: String,
    pub limit: u32,
    pub offset: u32,
    pub domain: Option<String>,
    /// Collapse to the best-scoring chunk per document.
    pub per_doc: bool,
    pub mode: Mode,
    /// HTTP lattice only; the embedded index has no MMR arm.
    pub mmr: bool,
    /// HTTP lattice only; the embedded walk already skips `_archives/`.
    pub include_archives: bool,
    /// `none` skips the vector arm for this query (no embedder round-trip).
    pub embedder: Option<String>,
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

/// The embedding space recorded in an index, read without a provider round-trip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredEmbedder {
    pub provider: String,
    pub model: String,
    pub dim: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexReport {
    pub documents: u64,
    pub chunks: u64,
    pub edges: u64,
    /// Chunks given a vector this run. Zero without an embedder.
    pub embedded: u64,
    /// Why the semantic arm produced nothing. Indexing still succeeded: files
    /// are the source of truth and BM25 still answers.
    pub embed_error: Option<String>,
}

impl Engine {
    /// Open with no semantic arm: FTS-only, which is the first-run default.
    pub fn open(vault: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(vault, None)
    }

    /// Open with an optional [`Embedder`]. Transport lives in the caller.
    pub fn open_with(
        vault: impl AsRef<Path>,
        embedder: Option<std::sync::Arc<dyn Embedder>>,
    ) -> Result<Self> {
        let vault = vault.as_ref().canonicalize().unwrap_or_else(|_| vault.as_ref().to_path_buf());
        if !vault.is_dir() {
            return Err(Error::Usage(format!("vault is not a directory: {}", vault.display())));
        }
        let dir = vault.join(".lapis");
        std::fs::create_dir_all(&dir)?;
        let db = dir.join("lattice.sqlite");
        // Before the connection exists: sqlite-vec registers as an auto-extension,
        // so `vec0` is present on this connection and every later one without a
        // shared object to find and without enabling `load_extension`.
        sqlite::register_vec();
        let conn = Connection::open(&db)?;
        sqlite::migrate(&conn)?;
        sqlite::assert_producer(&conn, &db)?;
        // A change of model or dimension wipes stored vectors: keeping rows from
        // a different embedding space would compare incomparable numbers.
        embed::reconcile_space(&conn, embedder.as_deref())?;
        Ok(Self { vault, conn, embedder })
    }

    pub fn db_path(&self) -> PathBuf {
        self.vault.join(".lapis/lattice.sqlite")
    }

    /// Full-vault reindex, then embed anything missing a vector.
    pub fn reindex(&mut self) -> Result<IndexReport> {
        let mut r = index::reindex(&self.conn, &self.vault)?;
        self.embed_into(&mut r);
        Ok(r)
    }

    /// Reindex exactly one vault-relative path, in place. A removed file is
    /// dropped from the index. Costs one file read, not a vault walk.
    pub fn reindex_path(&mut self, rel: &str) -> Result<IndexReport> {
        let mut r = index::reindex_path(&self.conn, &self.vault, rel)?;
        self.embed_into(&mut r);
        Ok(r)
    }

    /// Best-effort embedding. A dead embedder must not fail a write — the note
    /// is already on disk and BM25 already finds it — so the error is carried
    /// in the report instead of returned.
    fn embed_into(&mut self, r: &mut IndexReport) {
        match self.embed_pending() {
            Ok(n) => r.embedded = n,
            Err(e) => r.embed_error = Some(e.to_string()),
        }
    }

    /// Convenience wrapper kept for the 0.1 API.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Hit>> {
        Ok(self.search_with(&SearchParams { query: query.to_string(), limit, ..Default::default() })?.hits)
    }

    pub fn search_with(&self, p: &SearchParams) -> Result<SearchResult> {
        let mut p = p.clone();
        if p.embedder.as_deref() == Some("none") {
            p.mode = Mode::Bm25;
        }
        sqlite::search(&self.conn, &p, self.embedder.as_deref())
    }

    /// Embed any chunk that has no vector yet. No-op without an embedder.
    /// An embedder error surfaces here rather than storing a placeholder.
    pub fn embed_pending(&mut self) -> Result<u64> {
        match self.embedder.clone() {
            Some(e) => embed::embed_missing(&self.conn, e.as_ref(), 32),
            None => Ok(0),
        }
    }

    /// The embedding space already recorded in a vault's index, without
    /// opening a full engine and without contacting any provider. This is what
    /// lets `auto` resolve once at init/doctor and stay resolved: ordinary
    /// commands read the answer instead of probing a daemon on every search.
    pub fn stored_embedder(vault: impl AsRef<Path>) -> Option<StoredEmbedder> {
        let db = vault.as_ref().join(".lapis/lattice.sqlite");
        let conn = Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let provider = sqlite::meta_get(&conn, "embedder")?;
        if provider == "none" {
            return None;
        }
        Some(StoredEmbedder {
            model: sqlite::meta_get(&conn, "embed_model")?,
            dim: sqlite::meta_get(&conn, "embed_dim")?.parse().ok()?,
            provider,
        })
    }

    /// Which arm is live: `none` | `ollama` | `onnx`.
    pub fn embedder_name(&self) -> &'static str {
        self.embedder.as_ref().map(|e| e.provider()).unwrap_or("none")
    }

    /// `lapis list`: the documents table, filtered and paged.
    pub fn documents(&self, p: &ListParams) -> Result<Vec<Document>> {
        sqlite::documents(&self.conn, p)
    }

    /// Whole-vault graph for the canvas: one node per indexed document plus one
    /// per dangling link target, one edge per wikilink, degree = in + out.
    ///
    /// This is the **global** graph. [`Engine::neighbors`] is hop-1 and the
    /// desktop's hop-ring walk is a local view; neither is this.
    pub fn graph_snapshot(&self) -> Result<GraphSnapshot> {
        graph::snapshot(&self.conn, &self.vault.display().to_string(), SNAPSHOT_NODE_CAP)
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
        assert!(matches!(v, Err(Error::Usage(m)) if m.contains("needs an embedder")));
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

    /// A test double, not a product fallback. Deterministic and content-dependent
    /// so cosine ordering is real: different text genuinely gets a different
    /// vector. The product never synthesises vectors — see the test below.
    struct BagEmbedder {
        model: &'static str,
        dim: usize,
    }

    impl Embedder for BagEmbedder {
        fn model(&self) -> &str {
            self.model
        }
        fn dim(&self) -> usize {
            self.dim
        }
        fn provider(&self) -> &'static str {
            "onnx"
        }
        fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| bag(t, self.dim)).collect())
        }
        fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            Ok(bag(text, self.dim))
        }
    }

    fn bag(text: &str, dim: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        for c in text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()) {
            v[(c as usize - 'a' as usize) % dim] += 1.0;
        }
        v
    }

    /// An embedder that is present but broken — the Ollama-is-down case.
    struct DeadEmbedder;

    impl Embedder for DeadEmbedder {
        fn model(&self) -> &str {
            "dead-model"
        }
        fn dim(&self) -> usize {
            8
        }
        fn provider(&self) -> &'static str {
            "ollama"
        }
        fn embed_documents(&self, _: &[String]) -> Result<Vec<Vec<f32>>> {
            Err(Error::Usage("ollama unreachable".into()))
        }
        fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
            Err(Error::Usage("ollama unreachable".into()))
        }
    }

    /// C1/C2/C3: a vault that is not only markdown. One HTML page, one YAML
    /// note, and the skip-list directories that must never be walked.
    #[test]
    fn html_and_yaml_are_indexed_with_their_kind() {
        let d = vault();
        std::fs::write(
            d.join("page.html"),
            "<!doctype html><html><head><title>Release notes</title>\
             <style>b{color:red}</style><script>alert('no')</script></head>\
             <body><h1>Shipping</h1><p>kumquat harvest</p></body></html>",
        )
        .unwrap();
        std::fs::write(
            d.join("deploy.yml"),
            "name: Deploy plan\nstatus: draft\nsteps:\n  - build kumquat\n  - ship\n",
        )
        .unwrap();
        // must never be walked, whatever extension they hold
        for skip in ["node_modules", ".venv"] {
            std::fs::create_dir_all(d.join(skip)).unwrap();
            std::fs::write(d.join(skip).join("junk.yml"), "name: junk\n").unwrap();
            std::fs::write(d.join(skip).join("junk.html"), "<p>junk</p>").unwrap();
        }

        let mut e = Engine::open(&d).unwrap();
        let r = e.reindex().unwrap();
        assert_eq!(r.documents, 4, "two markdown, one html, one yaml — nothing from the skip list");

        let rows = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
        let kind_of = |p: &str| rows.iter().find(|r| r.path == p).map(|r| r.kind.clone());
        assert_eq!(kind_of("page.html").as_deref(), Some("html"));
        assert_eq!(kind_of("deploy.yml").as_deref(), Some("yaml"));
        assert_eq!(kind_of("Welcome.md").as_deref(), Some("markdown"));
        assert!(rows.iter().all(|r| !r.path.contains("node_modules") && !r.path.contains(".venv")));

        // titles come from the format's own idea of a title
        let title = |p: &str| rows.iter().find(|r| r.path == p).and_then(|r| r.title.clone());
        assert_eq!(title("page.html").as_deref(), Some("Release notes"), "<title> wins for html");
        assert_eq!(title("deploy.yml").as_deref(), Some("Deploy plan"), "yaml is its own frontmatter");

        // C3: hits carry the kind recorded at index time
        let hits = e.search("kumquat", 10).unwrap();
        let kinds: std::collections::BTreeSet<&str> = hits.iter().map(|h| h.kind.as_str()).collect();
        assert_eq!(kinds, ["html", "yaml"].into_iter().collect(), "both formats are searchable");
        assert!(hits.iter().any(|h| h.path == "page.html" && h.heading.as_deref() == Some("Shipping")));

        // chrome never reaches the index
        assert!(e.search("alert", 10).unwrap().is_empty(), "script bodies are not searchable");
        assert!(e.search("color", 10).unwrap().is_empty(), "style bodies are not searchable");

        // yaml stays retrievable by its own top-level key
        let by_key = e.search("steps", 10).unwrap();
        assert!(by_key.iter().any(|h| h.path == "deploy.yml"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// B9, the named one. A failing embedder must drop the vector arm and write
    /// nothing. Storing a placeholder would give cosine 1.0 against every query,
    /// so recall would look perfect and be meaningless — worse than no vectors.
    #[test]
    fn embedder_failure_omits_vector_arm_and_never_writes_a_dummy() {
        let d = vault();
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(DeadEmbedder))).unwrap();

        // Indexing still succeeds: files are the source of truth and BM25 answers.
        let r = e.reindex().unwrap();
        assert_eq!(r.documents, 2);
        assert_eq!(r.embedded, 0, "nothing was embedded");
        assert!(r.embed_error.is_some(), "and the failure is reported, not swallowed");

        // The decisive assertion: no placeholder rows.
        let stored: i64 =
            e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).expect("count vectors");
        assert_eq!(stored, 0, "a failed embedder must write no vectors at all");

        // Search still works, and says truthfully that only BM25 ran.
        let res = e
            .search_with(&SearchParams { query: "welcome".into(), limit: 10, ..Default::default() })
            .unwrap();
        assert!(!res.hits.is_empty(), "BM25 still answers");
        assert_eq!(res.modalities, ["bm25"], "the vector arm is omitted, not faked");
        assert!(
            res.hits.iter().all(|h| h.score < 1.0),
            "no hit carries a cosine-1.0 score from a synthesised vector"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// v0.4: the extension is linked into this binary, not loaded from a file.
    /// If registration ever stopped happening, every vector path would fail at
    /// `CREATE VIRTUAL TABLE` instead of here.
    #[test]
    fn sqlite_vec_is_linked_and_vec0_is_queryable() {
        let d = vault();
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();

        let version: String = e.conn.query_row("SELECT vec_version()", [], |r| r.get(0)).unwrap();
        assert!(version.starts_with('v'), "sqlite-vec answered: {version}");

        // Nothing was loaded from disk to get here, and nothing could be: the
        // C API that enables `load_extension` was never called, so the SQL
        // function refuses.
        let loaded = e.conn.query_row("SELECT load_extension('nonexistent')", [], |r| r.get::<_, String>(0));
        assert!(loaded.is_err(), "load_extension is off, so vec0 can only be here by linking");

        e.reindex().unwrap();
        // The table is a real vec0 virtual table holding the chunk vectors.
        let sql: String = e
            .conn
            .query_row("SELECT sql FROM sqlite_master WHERE name = 'chunk_vec'", [], |r| r.get(0))
            .unwrap();
        assert!(sql.contains("USING vec0"), "chunk_vec is a vec0 table: {sql}");
        assert!(sql.contains("float[8]"), "the column is as wide as the embedder: {sql}");
        assert!(sql.contains("distance_metric=cosine"), "cosine, as the Rust scan was: {sql}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// v0.4: ranking is a KNN `MATCH` inside SQLite, not every stored blob
    /// dragged into Rust and scored there.
    #[test]
    fn vector_rank_is_a_knn_match_not_a_blob_scan() {
        let d = vault();
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        e.reindex().unwrap();

        // The planner's own account of the query: a virtual table lookup on
        // chunk_vec, which is what a `MATCH` constraint compiles to.
        let mut stmt = e.conn.prepare(&format!("EXPLAIN QUERY PLAN {}", embed::KNN_SQL)).unwrap();
        let probe = embed::to_blob(&[0.0f32; 8]);
        let plan: Vec<String> = stmt
            .query_map(rusqlite::params![probe, 5i64], |r| r.get::<_, String>(3))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        let plan = plan.join(" | ");
        assert!(plan.contains("chunk_vec"), "the plan reads chunk_vec: {plan}");
        assert!(
            plan.to_lowercase().contains("virtual table"),
            "and reads it as a virtual table, which is where the KNN happens: {plan}"
        );

        // And the query really is a MATCH with a k, not a scan with a LIMIT.
        assert!(embed::KNN_SQL.contains("MATCH"), "{}", embed::KNN_SQL);
        assert!(embed::KNN_SQL.contains("k = ?2"), "{}", embed::KNN_SQL);

        // It answers, and it answers in distance order.
        let ids =
            embed::vector_rank(&e.conn, &BagEmbedder { model: "bag-v1", dim: 8 }, "welcome", 10).unwrap();
        assert!(!ids.is_empty(), "the KNN arm returns neighbours");
        assert!(ids.len() <= 10);
        let distinct: std::collections::BTreeSet<i64> = ids.iter().copied().collect();
        assert_eq!(distinct.len(), ids.len(), "no chunk is returned twice");

        // `k` is honoured as the neighbour count.
        let two =
            embed::vector_rank(&e.conn, &BagEmbedder { model: "bag-v1", dim: 8 }, "welcome", 2).unwrap();
        assert_eq!(two.len(), 2.min(ids.len()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// v0.4: a database written by v0.3 has a blob table. Its vectors are worth
    /// keeping, because re-embedding a vault costs a model run per chunk.
    #[test]
    fn a_pre_v04_blob_table_is_adopted_then_retired() {
        let d = vault();
        {
            // Build the index and let v0.4 write its vectors, then stage the old
            // shape: same rows, in the table an older build would have used.
            let mut e =
                Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
                    .unwrap();
            e.reindex().unwrap();
            let ids: Vec<i64> = {
                let mut s = e.conn.prepare("SELECT chunk_id FROM chunks ORDER BY chunk_id").unwrap();
                s.query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect()
            };
            assert!(ids.len() >= 2, "the fixture has chunks to carry over");
            e.conn
                .execute_batch(
                    "DROP TABLE IF EXISTS chunk_vec;
                     CREATE TABLE embeddings (chunk_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);",
                )
                .unwrap();
            for (n, id) in ids.iter().enumerate() {
                // One row of the wrong width, to prove it is judged not trusted.
                let wide = if n == 0 { 8 } else { 4 };
                let v: Vec<f32> = (0..wide).map(|i| i as f32).collect();
                e.conn
                    .execute(
                        "INSERT INTO embeddings(chunk_id, vec) VALUES(?1, ?2)",
                        rusqlite::params![id, embed::to_blob(&v)],
                    )
                    .unwrap();
            }
        }
        // Reopening in the same embedding space adopts what fits and retires the
        // old table either way.
        let e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        let moved: i64 = e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).unwrap();
        assert_eq!(moved, 1, "only the row of the right width came across");
        let legacy: i64 = e
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'embeddings'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(legacy, 0, "the blob table is gone, so nothing writes to a store nothing reads");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Chunk rows and their vectors go together. `vec0` has no foreign key, so
    /// without this a reindex would leave neighbours pointing at nothing.
    #[test]
    fn vectors_follow_their_chunks_on_reindex_and_forget() {
        let d = vault();
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        let r = e.reindex().unwrap();
        let count = |e: &Engine| -> i64 {
            e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(count(&e), r.chunks as i64);

        // A second full reindex does not double the store.
        let again = e.reindex().unwrap();
        assert_eq!(count(&e), again.chunks as i64, "a reindex replaces vectors, it does not stack them");

        // Every vector still names a live chunk.
        let orphans: i64 = e
            .conn
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec WHERE chunk_id NOT IN (SELECT chunk_id FROM chunks)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);

        // Removing one note takes its vectors with it.
        std::fs::remove_file(d.join("notes/Alpha.md")).unwrap();
        e.reindex_path("notes/Alpha.md").unwrap();
        let left: i64 = e
            .conn
            .query_row(
                "SELECT COUNT(*) FROM chunk_vec WHERE chunk_id NOT IN (SELECT chunk_id FROM chunks)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "forgetting a path forgets its vectors");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The working path: vectors are stored, fusion reports both arms, and a
    /// change of embedding space wipes rather than mixing incomparable rows.
    #[test]
    fn vector_arm_fuses_and_wipes_on_model_change() {
        let d = vault();
        // None when there is no vector table at all, which is what a database
        // with no embedder should look like: not an empty store, no store.
        let vectors = |e: &Engine| -> Option<i64> {
            e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).ok()
        };
        let count = |e: &Engine| -> i64 { vectors(e).expect("vector table exists") };
        {
            let mut e =
                Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
                    .unwrap();
            let r = e.reindex().unwrap();
            assert!(r.embed_error.is_none());
            assert!(r.embedded > 0 && r.embedded == r.chunks, "every chunk got a vector");
            assert_eq!(count(&e), r.chunks as i64);

            let res = e
                .search_with(&SearchParams { query: "welcome".into(), limit: 10, ..Default::default() })
                .unwrap();
            assert_eq!(res.modalities, ["bm25", "vector"], "both arms ran and are reported");
            assert!(!res.hits.is_empty());

            let h = e.health().unwrap();
            assert_eq!(h.embedder, "onnx");
            assert_eq!(h.embed_model.as_deref(), Some("bag-v1"));
            assert_eq!(h.embed_dim, Some(8));
        }
        // Same model, reopened: vectors survive.
        {
            let e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
                .unwrap();
            assert!(count(&e) > 0, "reopening with the same space keeps vectors");
        }
        // Different dimension: the space changed, so stored vectors are dropped.
        {
            let e =
                Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 12 })))
                    .unwrap();
            assert_eq!(count(&e), 0, "a dimension change wipes incomparable rows");
        }
        // Dropping the embedder entirely also clears the space and reports none.
        {
            let e = Engine::open(&d).unwrap();
            assert_eq!(vectors(&e), None, "no embedder means no vector table, not an empty one");
            assert_eq!(e.health().unwrap().embedder, "none");
            let res = e
                .search_with(&SearchParams { query: "welcome".into(), limit: 5, ..Default::default() })
                .unwrap();
            assert_eq!(res.modalities, ["bm25"]);
        }
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
