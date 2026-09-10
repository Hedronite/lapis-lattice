//! Embedded SQLite + FTS5 lattice for a Markdown vault.
//!
//! Files on disk are the write source of truth. This crate only writes
//! `<vault>/.lapis/lattice.sqlite`. Search is FTS5 BM25; vectors are not in 0.1.

mod analytics;
mod graph;
mod index;
mod sqlite;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

pub use analytics::{ANALYTICS_QUERIES, Analytics};
pub use graph::Neighbor;

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

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub status: String,
    pub documents_indexed: u64,
    pub edges: u64,
    pub dangling_links: u64,
    pub db_path: String,
    pub embedder: &'static str,
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
        Ok(Self { vault, conn })
    }

    pub fn db_path(&self) -> PathBuf {
        self.vault.join(".lapis/lattice.sqlite")
    }

    pub fn reindex(&mut self) -> Result<IndexReport> {
        index::reindex(&self.conn, &self.vault)
    }

    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Hit>> {
        sqlite::search(&self.conn, query, limit)
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

    fn vault() -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("lapis-lattice-{n}"));
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
        assert!(h.dangling_links >= 1);
        assert_eq!(h.embedder, "none");

        let tags = e.analytics("tags").unwrap();
        assert_eq!(tags.query, "tags");
        assert!(tags.rows.iter().any(|row| row.iter().any(|c| c.contains("intro"))));

        assert!(e.analytics("drop table documents").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
