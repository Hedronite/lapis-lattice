use std::path::Path;

use rusqlite::{Connection, params};

use rusqlite::types::Value as SqlValue;

use crate::{
    Document, Error, Graph, Health, Hit, ListParams, Mode, PRODUCER, Result, SearchParams, SearchResult,
};

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

/// Stamp the database as ours, or refuse it. A 0.1 database has no stamp yet,
/// so an absent key is backfilled rather than rejected; a *different* producer
/// is a hard error, which is what keeps a Lapis vault index and an agent-memory
/// store from ever being opened as one another.
pub fn assert_producer(conn: &Connection, db_path: &Path) -> Result<()> {
    let found: Option<String> =
        conn.query_row("SELECT value FROM meta WHERE key = 'producer'", [], |r| r.get(0)).ok();
    match found.as_deref() {
        Some(PRODUCER) => Ok(()),
        None => {
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES ('producer', ?1)",
                params![PRODUCER],
            )?;
            Ok(())
        }
        Some(other) => Err(Error::Usage(format!(
            "{} was produced by '{other}', not '{PRODUCER}'; refusing to open a foreign index",
            db_path.display()
        ))),
    }
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).ok()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES (?1, ?2)", params![key, value])?;
    Ok(())
}

/// Turn a user query into an FTS5 MATCH expression: keep word characters,
/// quote each term, AND them together.
fn fts_expr(q: &str) -> String {
    q.split_whitespace()
        .map(|w| {
            let t: String = w.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect();
            if t.is_empty() { w.to_string() } else { format!("\"{t}\"") }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn search(conn: &Connection, p: &SearchParams) -> Result<SearchResult> {
    let q = p.query.trim();
    if q.is_empty() {
        return Err(Error::Usage("search query is required".into()));
    }
    if p.mode == Mode::Vector {
        // Refuse rather than quietly returning BM25 rows and calling them vector hits.
        return Err(Error::Usage(
            "vector mode needs an embedder; the embedded index is FTS-only. Use --mode bm25|hybrid, \
             or lattice.mode = \"http\" against a lattice that has vectors"
                .into(),
        ));
    }
    let limit = p.limit.clamp(1, 50);
    let offset = p.offset;
    let fts = fts_expr(q);
    if fts.trim().is_empty() {
        return Ok(SearchResult { hits: vec![], modalities: vec!["bm25".into()] });
    }

    // Fetch a generous window so per-doc collapse and offset still have rows to
    // work with, then slice in Rust. Keeps snippet() out of a GROUP BY.
    let window = ((offset as usize + limit as usize) * 8).clamp(50, 1000) as i64;
    let mut sql = String::from(
        "SELECT c.path, d.title, c.heading, snippet(chunks_fts, 0, '', '', '…', 12), \
         bm25(chunks_fts), d.domain, d.doc_type \
         FROM chunks_fts JOIN chunks c ON c.chunk_id = chunks_fts.rowid \
         JOIN documents d ON d.path = c.path WHERE chunks_fts MATCH ?1",
    );
    let mut args: Vec<SqlValue> = vec![SqlValue::Text(fts)];
    if let Some(dom) = p.domain.as_deref().filter(|d| !d.trim().is_empty()) {
        args.push(SqlValue::Text(dom.to_string()));
        sql.push_str(&format!(" AND d.domain = ?{}", args.len()));
    }
    args.push(SqlValue::Integer(window));
    sql.push_str(&format!(" ORDER BY bm25(chunks_fts) LIMIT ?{}", args.len()));

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut scored: Vec<Hit> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        if p.per_doc && !seen.insert(path.clone()) {
            continue;
        }
        let score: f64 = row.get::<_, f64>(4).unwrap_or(0.0);
        scored.push(Hit {
            path,
            title: row.get(1)?,
            heading: row.get(2)?,
            snippet: row.get(3)?,
            rank: 0,
            // bm25() is lower-is-better; invert for a friendlier score.
            score: if score == 0.0 { 0.0 } else { 1.0 / (1.0 + score.abs()) },
            domain: row.get(5)?,
            doc_type: row.get(6)?,
        });
    }

    let hits: Vec<Hit> = scored
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .enumerate()
        .map(|(i, mut h)| {
            // Ranks stay absolute across pages.
            h.rank = offset + i as u32 + 1;
            h
        })
        .collect();
    Ok(SearchResult { hits, modalities: vec!["bm25".into()] })
}

/// `lapis list`: documents table, filtered and paged.
pub fn documents(conn: &Connection, p: &ListParams) -> Result<Vec<Document>> {
    let limit = if p.limit == 0 { 50 } else { p.limit.clamp(1, 1000) };
    let mut sql = String::from(
        "SELECT path, kind, title, domain, doc_type, status, priority, tags_json, mtime, hash \
         FROM documents WHERE 1 = 1",
    );
    let mut args: Vec<SqlValue> = Vec::new();
    let eq = |sql: &mut String, args: &mut Vec<SqlValue>, col: &str, v: Option<&str>| {
        if let Some(v) = v.filter(|s| !s.trim().is_empty()) {
            args.push(SqlValue::Text(v.to_string()));
            sql.push_str(&format!(" AND {col} = ?{}", args.len()));
        }
    };
    eq(&mut sql, &mut args, "domain", p.domain.as_deref());
    eq(&mut sql, &mut args, "doc_type", p.doc_type.as_deref());
    eq(&mut sql, &mut args, "status", p.status.as_deref());
    if let Some(prefix) = p.prefix.as_deref().filter(|s| !s.trim().is_empty()) {
        args.push(SqlValue::Text(format!("{}%", prefix.trim_end_matches('/'))));
        sql.push_str(&format!(" AND path LIKE ?{}", args.len()));
    }
    if let Some(tag) = p.tag.as_deref().filter(|s| !s.trim().is_empty()) {
        args.push(SqlValue::Text(tag.to_string()));
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM json_each(documents.tags_json) WHERE value = ?{})",
            args.len()
        ));
    }
    args.push(SqlValue::Integer(limit as i64));
    sql.push_str(&format!(" ORDER BY path LIMIT ?{}", args.len()));
    args.push(SqlValue::Integer(p.offset as i64));
    sql.push_str(&format!(" OFFSET ?{}", args.len()));

    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let tags_json: String = row.get(7)?;
        let mtime: i64 = row.get(8).unwrap_or(0);
        out.push(Document {
            path: row.get(0)?,
            kind: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "markdown".into()),
            title: row.get(2)?,
            domain: row.get(3)?,
            doc_type: row.get(4)?,
            status: row.get(5)?,
            priority: row.get(6)?,
            tags: serde_json::from_str(&tags_json).unwrap_or_default(),
            updated_at: if mtime > 0 { Some(mtime as u64) } else { None },
            hash: row.get(9)?,
        });
    }
    Ok(out)
}

pub fn health(conn: &Connection, db_path: &Path) -> Result<Health> {
    let count =
        |sql: &str| -> Result<u64> { Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0)).map(|n| n as u64)?) };
    let documents_indexed = count("SELECT COUNT(*) FROM documents")?;
    let edges = count("SELECT COUNT(*) FROM edges")?;
    let dangling_links = count("SELECT COUNT(*) FROM edges WHERE resolved = 0")?;
    // `built` distinguishes "never indexed" from "indexed, no links".
    let built = meta_get(conn, "graph_built").as_deref() == Some("1");
    let embed_model = meta_get(conn, "embed_model").filter(|m| m != "none");
    Ok(Health {
        status: if built { "ok".into() } else { "degraded".into() },
        documents_indexed,
        db_path: db_path.display().to_string(),
        embedder: "none",
        embed_model,
        embed_dim: None,
        graph: Graph { built, edges, dangling_links },
    })
}
