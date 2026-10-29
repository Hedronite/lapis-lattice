//! Embedded SQLite + FTS5 lattice for a Markdown vault.
//!
//! Files on disk are the write source of truth. This crate only writes
//! `<vault>/.lapis/lattice.sqlite` by default; [`Engine::open_in`] can keep the
//! index in a caller-chosen directory instead. Search is FTS5 BM25; vectors are
//! not in 0.2.
//!
//! The database is stamped `producer = "lapis-lattice"` at creation and the
//! stamp is asserted on open, so a path pointed at a foreign SQLite file (an
//! agent-memory store, say) fails loudly instead of quietly mixing corpora.

mod analytics;
mod embed;
mod graph;
mod index;
mod kinds;
mod location;
mod refresh;
mod scope;
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
pub use refresh::RefreshReport;
pub use scope::Scope;

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

/// Open an index over a Markdown vault: `<vault>/.lapis/lattice.sqlite` by
/// default, or in a caller-chosen directory with [`Engine::open_in`].
pub struct Engine {
    vault: PathBuf,
    /// The SQLite file, wherever the default or [`Engine::open_in`] put it.
    db: PathBuf,
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
    /// Post-retrieve judge annotation. Never written to sqlite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<serde_json::Value>,
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
    /// Indexed paths missing on disk, new walkable files, or mtime drift.
    #[serde(skip_serializing_if = "is_zero")]
    pub stale_documents: u64,
    pub db_path: String,
    pub embedder: &'static str,
    pub embed_model: Option<String>,
    pub embed_dim: Option<u32>,
    /// Chunks in the index, and how many of them carry a vector.
    pub chunks: u64,
    pub vectors: u64,
    pub graph: Graph,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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

    /// Open with an optional [`Embedder`]. Transport lives in the caller. The
    /// index lives in `<vault>/.lapis/`, exactly as before.
    pub fn open_with(
        vault: impl AsRef<Path>,
        embedder: Option<std::sync::Arc<dyn Embedder>>,
    ) -> Result<Self> {
        Self::open_at(vault.as_ref(), None, embedder)
    }
    /// Open with the index in a caller-chosen `index_dir`, not the vault's
    /// `.lapis/`: build, search, reindex and every helper work the same, and the
    /// vault tree gains no file. The directory is created if needed, like
    /// `.lapis/` is. For a host that may not write the vault, a phone.
    pub fn open_in(
        vault: impl AsRef<Path>,
        index_dir: impl AsRef<Path>,
        embedder: Option<std::sync::Arc<dyn Embedder>>,
    ) -> Result<Self> {
        Self::open_at(vault.as_ref(), Some(index_dir.as_ref()), embedder)
    }
    fn open_at(
        vault: &Path,
        index_dir: Option<&Path>,
        embedder: Option<std::sync::Arc<dyn Embedder>>,
    ) -> Result<Self> {
        let vault = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
        if !vault.is_dir() {
            return Err(Error::Usage(format!("vault is not a directory: {}", vault.display())));
        }
        let dir = match index_dir {
            Some(d) => d.to_path_buf(),
            None => vault.join(location::DEFAULT_DIR),
        };
        std::fs::create_dir_all(&dir)?;
        let db = location::db_file(&dir);
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
        Ok(Self { vault, db, conn, embedder })
    }

    pub fn db_path(&self) -> PathBuf {
        self.db.clone()
    }

    /// Full-vault reindex, then embed anything missing a vector.
    pub fn reindex(&mut self) -> Result<IndexReport> {
        self.reindex_with_progress(&mut |_, _| {})
    }

    /// [`Engine::reindex`], calling `progress(files indexed, files total)` during the
    /// walk. Embedding missing vectors follows the last file and is not counted.
    pub fn reindex_with_progress(&mut self, progress: &mut dyn FnMut(u64, u64)) -> Result<IndexReport> {
        let mut r = index::reindex(&self.conn, &self.vault, progress)?;
        self.embed_into(&mut r, None);
        Ok(r)
    }

    /// Bring the index up to date with the vault: added, updated and removed
    /// notes since the last build, compared by mtime and size, with [`Scope`]
    /// and the same chunking as a full reindex. A second call with nothing
    /// changed writes nothing, so this is safe on every app launch.
    pub fn refresh(&mut self) -> Result<RefreshReport> {
        let mut r = refresh::refresh(&self.conn, &self.vault)?;
        let mut arm = IndexReport { documents: 0, chunks: 0, edges: 0, embedded: 0, embed_error: None };
        self.embed_into(&mut arm, None);
        r.embedded = arm.embedded;
        r.embed_error = arm.embed_error;
        Ok(r)
    }
    /// Reindex exactly one vault-relative path, in place. A removed file is
    /// dropped from the index. Costs one file read, not a vault walk, and
    /// embeds only that path's chunks.
    pub fn reindex_path(&mut self, rel: &str) -> Result<IndexReport> {
        let rel = rel.trim_start_matches("./").replace('\\', "/");
        let mut r = index::reindex_path(&self.conn, &self.vault, &rel)?;
        self.embed_into(&mut r, Some(&rel));
        Ok(r)
    }

    /// Best-effort embedding. A dead embedder must not fail a write — the note
    /// is already on disk and BM25 already finds it — so the error is carried
    /// in the report instead of returned.
    fn embed_into(&mut self, r: &mut IndexReport, only: Option<&str>) {
        let pending = match self.embedder.clone() {
            Some(e) => embed::embed_missing(&self.conn, e.as_ref(), 32, only),
            None => Ok(0),
        };
        match pending {
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
            Some(e) => embed::embed_missing(&self.conn, e.as_ref(), 32, None),
            None => Ok(0),
        }
    }

    /// The embedding space already recorded in a vault's index, without
    /// opening a full engine and without contacting any provider. This is what
    /// lets `auto` resolve once at init/doctor and stay resolved: ordinary
    /// commands read the answer instead of probing a daemon on every search.
    pub fn stored_embedder(vault: impl AsRef<Path>) -> Option<StoredEmbedder> {
        Self::stored_embedder_in(vault.as_ref().join(location::DEFAULT_DIR))
    }
    /// [`Engine::stored_embedder`] for an index kept in a caller-chosen
    /// directory, the one [`Engine::open_in`] opened.
    pub fn stored_embedder_in(index_dir: impl AsRef<Path>) -> Option<StoredEmbedder> {
        let db = location::db_file(index_dir.as_ref());
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
        let mut h = sqlite::health(&self.conn, &self.db_path())?;
        h.stale_documents = index::stale_count(&self.conn, &self.vault)?;
        Ok(h)
    }

    pub fn analytics(&self, query: &str) -> Result<Analytics> {
        analytics::run(&self.conn, query)
    }
}

#[cfg(test)]
mod tests;
