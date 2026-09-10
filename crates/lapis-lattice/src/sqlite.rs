use std::path::Path;

use rusqlite::{Connection, params};

use crate::{Error, Health, Hit, Result};

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS documents (
            path TEXT PRIMARY KEY,
            title TEXT,
            domain TEXT,
            doc_type TEXT,
            status TEXT,
            priority TEXT,
            tags_json TEXT NOT NULL DEFAULT '[]',
            mtime INTEGER NOT NULL DEFAULT 0,
            hash TEXT,
            kind TEXT NOT NULL DEFAULT 'markdown'
        );
        CREATE TABLE IF NOT EXISTS chunks (
            chunk_id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL REFERENCES documents(path) ON DELETE CASCADE,
            chunk_index INTEGER NOT NULL,
            heading TEXT,
            text TEXT NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            text,
            path UNINDEXED,
            heading UNINDEXED,
            content='chunks',
            content_rowid='chunk_id',
            tokenize = 'porter'
        );
        CREATE TABLE IF NOT EXISTS edges (
            src TEXT NOT NULL,
            dst_raw TEXT NOT NULL,
            dst_path TEXT,
            alias TEXT,
            anchor TEXT,
            resolved INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS edges_src ON edges(src);
        CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst_path);
        INSERT OR IGNORE INTO meta(key, value) VALUES ('schema_version', '1');
        INSERT OR IGNORE INTO meta(key, value) VALUES ('embed_model', 'none');
        "#,
    )?;
    Ok(())
}

pub fn search(conn: &Connection, query: &str, limit: u32) -> Result<Vec<Hit>> {
    let q = query.trim();
    if q.is_empty() {
        return Err(Error::Usage("search query is required".into()));
    }
    let limit = limit.clamp(1, 50);
    // Quote for FTS5: keep alphanumerics, join with AND so "welcome vault" works.
    let fts: String = q
        .split_whitespace()
        .map(|w| {
            let t: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
            if t.is_empty() { w.to_string() } else { format!("\"{t}\"") }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if fts.trim().is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        r#"
        SELECT c.path, d.title, c.heading, snippet(chunks_fts, 0, '', '', '…', 12),
               bm25(chunks_fts), d.domain, d.doc_type
        FROM chunks_fts
        JOIN chunks c ON c.chunk_id = chunks_fts.rowid
        JOIN documents d ON d.path = c.path
        WHERE chunks_fts MATCH ?1
        ORDER BY bm25(chunks_fts)
        LIMIT ?2
        "#,
    )?;
    let mut rows = stmt.query(params![fts, limit as i64])?;
    let mut out = Vec::new();
    let mut rank = 0u32;
    while let Some(row) = rows.next()? {
        rank += 1;
        let score: f64 = row.get::<_, f64>(4).unwrap_or(0.0);
        out.push(Hit {
            path: row.get(0)?,
            title: row.get(1)?,
            heading: row.get(2)?,
            snippet: row.get(3)?,
            rank,
            // bm25() is lower-is-better; invert for a friendlier score.
            score: if score == 0.0 { 0.0 } else { 1.0 / (1.0 + score.abs()) },
            domain: row.get(5)?,
            doc_type: row.get(6)?,
        });
    }
    Ok(out)
}

pub fn health(conn: &Connection, db_path: &Path) -> Result<Health> {
    let documents_indexed: u64 =
        conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get::<_, i64>(0)).map(|n| n as u64)?;
    let edges: u64 =
        conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get::<_, i64>(0)).map(|n| n as u64)?;
    let dangling_links: u64 = conn
        .query_row("SELECT COUNT(*) FROM edges WHERE resolved = 0", [], |r| r.get::<_, i64>(0))
        .map(|n| n as u64)?;
    Ok(Health {
        status: "ok".into(),
        documents_indexed,
        edges,
        dangling_links,
        db_path: db_path.display().to_string(),
        embedder: "none",
    })
}
