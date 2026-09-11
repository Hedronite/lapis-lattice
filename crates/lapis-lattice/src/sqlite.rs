use std::path::Path;

use rusqlite::{Connection, params};

use rusqlite::types::Value as SqlValue;

use crate::embed::{self, Embedder};
use crate::{
    Document, Error, Graph, Health, Hit, ListParams, Mode, PRODUCER, Result, SearchParams, SearchResult,
};

/// Name of the `vec0` virtual table holding chunk embeddings. It lives in the
/// same file as `chunks_fts`, so one database is still the whole index.
pub const VEC_TABLE: &str = "chunk_vec";

/// Link sqlite-vec into every connection this process opens.
///
/// `sqlite3_auto_extension` registers the entry point with SQLite itself, so
/// each new connection gets `vec0` and `vec_version()` without loading anything
/// from disk. That matters twice over: there is no `.so` to ship or find on
/// PATH, and `load_extension` stays disabled, so a vault database cannot talk
/// this process into loading code.
pub fn register_vec() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // The entry point is a C function pointer; sqlite-vec exports it with a
        // bare signature, so the cast to what SQLite expects is explicit here
        // rather than inferred.
        type EntryPoint = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        unsafe {
            let entry: EntryPoint = std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ());
            rusqlite::ffi::sqlite3_auto_extension(Some(entry));
        }
    });
}

/// True when the `vec0` table exists. It is created only once a dimension is
/// known, which means only once an embedder is present.
pub fn vec_table_exists(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![VEC_TABLE],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Create the vector table for `dim`-wide embeddings.
///
/// `vec0` fixes the width at creation, so this cannot live in [`migrate`]: the
/// width is a property of the embedder, and a database opened with
/// `--embedder none` has no embeddings and needs no table.
pub fn create_vec_table(conn: &Connection, dim: usize) -> Result<()> {
    // Cosine, because that is what the Rust scan this replaces computed.
    // Switching to the default L2 here would quietly re-rank every vault.
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {VEC_TABLE} USING vec0(
            chunk_id integer primary key,
            embedding float[{dim}] distance_metric=cosine
        );"
    ))?;
    Ok(())
}

/// Drop the vector table. Used when the embedding space changes: rows from a
/// different model or width are not comparable and must not be mixed in.
pub fn drop_vec_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {VEC_TABLE};"))?;
    Ok(())
}

/// Forget the vectors for a set of chunks.
///
/// A `vec0` table carries no foreign key, so nothing cascades when chunk rows
/// go. Without this a reindex would leave vectors pointing at chunk ids that no
/// longer exist, and KNN would keep returning them for rows the hydrator then
/// silently drops.
pub fn forget_vectors_for_path(conn: &Connection, rel: &str) -> Result<()> {
    if !vec_table_exists(conn)? {
        return Ok(());
    }
    conn.execute(
        &format!("DELETE FROM {VEC_TABLE} WHERE chunk_id IN (SELECT chunk_id FROM chunks WHERE path = ?1)"),
        params![rel],
    )?;
    Ok(())
}

/// Empty the vector table, for a full reindex.
pub fn clear_vectors(conn: &Connection) -> Result<()> {
    if !vec_table_exists(conn)? {
        return Ok(());
    }
    conn.execute_batch(&format!("DELETE FROM {VEC_TABLE};"))?;
    Ok(())
}

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
        CREATE TABLE IF NOT EXISTS embeddings (
            chunk_id INTEGER PRIMARY KEY REFERENCES chunks(chunk_id) ON DELETE CASCADE,
            vec BLOB NOT NULL
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

pub fn search(conn: &Connection, p: &SearchParams, embedder: Option<&dyn Embedder>) -> Result<SearchResult> {
    let q = p.query.trim();
    if q.is_empty() {
        return Err(Error::Usage("search query is required".into()));
    }
    if p.mode == Mode::Vector && embedder.is_none() {
        // Refuse rather than quietly returning BM25 rows and calling them vector hits.
        return Err(Error::Usage(
            "vector mode needs an embedder; this index has none. Use --mode bm25|hybrid, or configure \
             [embedder] and reindex"
                .into(),
        ));
    }
    let limit = p.limit.clamp(1, 50);
    let offset = p.offset;
    let window = ((offset as usize + limit as usize) * 8).clamp(50, 1000);

    // ---- BM25 arm (skipped only in vector-only mode)
    let mut modalities: Vec<String> = Vec::new();
    let mut fts_hits: std::collections::HashMap<i64, Hit> = std::collections::HashMap::new();
    let mut fts_rank: Vec<i64> = Vec::new();
    if p.mode != Mode::Vector {
        let fts = fts_expr(q);
        if !fts.trim().is_empty() {
            let mut sql = String::from(
                "SELECT c.chunk_id, c.path, d.title, c.heading, \
                 snippet(chunks_fts, 0, '', '', '…', 12), bm25(chunks_fts), d.domain, d.doc_type, \
                 d.kind, d.tags_json, c.chunk_index \
                 FROM chunks_fts JOIN chunks c ON c.chunk_id = chunks_fts.rowid \
                 JOIN documents d ON d.path = c.path WHERE chunks_fts MATCH ?1",
            );
            let mut args: Vec<SqlValue> = vec![SqlValue::Text(fts)];
            if let Some(dom) = p.domain.as_deref().filter(|d| !d.trim().is_empty()) {
                args.push(SqlValue::Text(dom.to_string()));
                sql.push_str(&format!(" AND d.domain = ?{}", args.len()));
            }
            args.push(SqlValue::Integer(window as i64));
            sql.push_str(&format!(" ORDER BY bm25(chunks_fts) LIMIT ?{}", args.len()));
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let score: f64 = row.get::<_, f64>(5).unwrap_or(0.0);
                fts_rank.push(id);
                fts_hits.insert(
                    id,
                    hit_from_row(
                        id,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        // bm25() is lower-is-better; invert for a friendlier score.
                        if score == 0.0 { 0.0 } else { 1.0 / (1.0 + score.abs()) },
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ),
                );
            }
        }
        modalities.push("bm25".into());
    }

    // ---- Vector arm. A failure here drops the arm; it never invents a vector.
    let mut vec_rank: Vec<i64> = Vec::new();
    if p.mode != Mode::Bm25
        && let Some(e) = embedder
        && let Ok(ids) = embed::vector_rank(conn, e, q, window)
        && !ids.is_empty()
    {
        vec_rank = ids;
        modalities.push("vector".into());
    }
    if p.mode == Mode::Vector && vec_rank.is_empty() {
        // asked for vectors only, and the arm produced nothing usable
        return Ok(SearchResult { hits: vec![], modalities });
    }

    // ---- Fuse. One arm alone keeps its own order.
    let mut rankings: Vec<Vec<i64>> = Vec::new();
    if !fts_rank.is_empty() {
        rankings.push(fts_rank.clone());
    }
    if !vec_rank.is_empty() {
        rankings.push(vec_rank.clone());
    }
    let fused = embed::rrf(&rankings, 60.0);

    // ---- Collapse, page, rank.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut hits: Vec<Hit> = Vec::new();
    for id in fused {
        let hit = match fts_hits.remove(&id) {
            Some(h) => h,
            None => match hydrate_chunk(conn, id, p.domain.as_deref())? {
                Some(h) => h,
                None => continue,
            },
        };
        if p.per_doc && !seen.insert(hit.path.clone()) {
            continue;
        }
        hits.push(hit);
        if hits.len() >= offset as usize + limit as usize {
            break;
        }
    }
    let hits: Vec<Hit> = hits
        .into_iter()
        .skip(offset as usize)
        .enumerate()
        .map(|(i, mut h)| {
            // Ranks stay absolute across pages.
            h.rank = offset + i as u32 + 1;
            h
        })
        .collect();
    Ok(SearchResult { hits, modalities })
}

/// Build a hit for a chunk the BM25 arm never saw (vector-only match). The
/// snippet is a plain text head: `snippet()` only exists inside an FTS query.
fn hydrate_chunk(conn: &Connection, chunk_id: i64, domain: Option<&str>) -> Result<Option<Hit>> {
    let mut stmt = conn.prepare(
        "SELECT c.path, d.title, c.heading, c.text, d.domain, d.doc_type, d.kind, \
         d.tags_json, c.chunk_index \
         FROM chunks c JOIN documents d ON d.path = c.path WHERE c.chunk_id = ?1",
    )?;
    let mut rows = stmt.query(params![chunk_id])?;
    let Some(row) = rows.next()? else { return Ok(None) };
    let dom: Option<String> = row.get(4)?;
    if let Some(want) = domain.filter(|d| !d.trim().is_empty())
        && dom.as_deref() != Some(want)
    {
        return Ok(None);
    }
    let text: String = row.get(3)?;
    let snippet: String = text.chars().take(160).collect();
    Ok(Some(hit_from_row(
        chunk_id,
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        Some(snippet),
        0.0,
        dom,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    )))
}

fn hit_from_row(
    chunk_id: i64,
    path: String,
    title: Option<String>,
    heading: Option<String>,
    snippet: Option<String>,
    score: f64,
    domain: Option<String>,
    doc_type: Option<String>,
    kind: Option<String>,
    tags_json: String,
    chunk_index: Option<i64>,
) -> Hit {
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
        Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or(&path).to_string()
    });
    Hit {
        kind: kind.unwrap_or_else(|| "markdown".into()),
        title,
        heading: heading.filter(|s| !s.is_empty()),
        snippet: snippet.filter(|s| !s.is_empty()),
        rank: 0,
        score,
        domain,
        doc_type,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        chunk_id: Some(chunk_id),
        chunk_index,
        path,
    }
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
    let embedder = match meta_get(conn, "embedder").as_deref() {
        Some("ollama") => "ollama",
        Some("onnx") => "onnx",
        _ => "none",
    };
    let embed_dim = meta_get(conn, "embed_dim").and_then(|d| d.parse::<u32>().ok());
    Ok(Health {
        status: if built { "ok".into() } else { "degraded".into() },
        documents_indexed,
        db_path: db_path.display().to_string(),
        embedder,
        embed_model,
        embed_dim,
        graph: Graph { built, edges, dangling_links },
    })
}
