//! Optional semantic arm.
//!
//! The crate does no HTTP. Transport belongs to the caller (Ollama today, ONNX
//! later), which hands in an [`Embedder`]. That keeps the engine dependency-light
//! and keeps model-specific details — like nomic's `search_query:` /
//! `search_document:` prefixes — with the implementation that knows about them.
//!
//! The rule that matters: **a failing embedder omits the vector arm.** It never
//! writes a placeholder vector. Identical placeholders give cosine 1.0 against
//! everything, so recall degrades into confident nonsense rather than falling
//! back to keyword search.
//!
//! Vectors live in a `vec0` virtual table in the same database as `chunks_fts`,
//! and the nearest-neighbour search is a SQL `MATCH`. Before v0.4 this module
//! read every stored blob into Rust and scored it there, which meant the whole
//! corpus crossed the FFI boundary on every query.

use rusqlite::{Connection, params};

use crate::Result;

/// Turns text into vectors. One embedding space per database.
pub trait Embedder: Send + Sync {
    /// Model id stamped into `meta.embed_model`; a change wipes stored vectors.
    fn model(&self) -> &str;
    fn dim(&self) -> usize;
    /// `ollama` | `onnx`, for `health.embedder`.
    fn provider(&self) -> &'static str;
    /// Embed stored text. Implementations add any document-side prefix.
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Embed a search query. Implementations add any query-side prefix.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}

pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn from_blob(b: &[u8]) -> Vec<f32> {
    let (quads, _tail) = b.as_chunks::<4>();
    quads.iter().copied().map(f32::from_le_bytes).collect()
}

/// Name of the pre-v0.4 blob table. A database written by an older build still
/// has one; see [`adopt_legacy`].
const LEGACY_TABLE: &str = "embeddings";

fn legacy_exists(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![LEGACY_TABLE],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Move pre-v0.4 blobs into the `vec0` table and retire the old one.
///
/// Re-embedding a whole vault costs a model run per chunk, so rows that are
/// still the right width are carried over rather than thrown away. A row of the
/// wrong width belongs to another embedding space and is dropped, which is the
/// same rule [`reconcile_space`] applies everywhere else. Returns how many moved.
pub fn adopt_legacy(conn: &Connection, dim: usize) -> Result<u64> {
    if !legacy_exists(conn)? {
        return Ok(0);
    }
    let table = crate::sqlite::VEC_TABLE;
    let mut moved = 0u64;
    {
        let mut stmt = conn.prepare(&format!("SELECT chunk_id, vec FROM {LEGACY_TABLE}"))?;
        let rows: Vec<(i64, Vec<u8>)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        for (id, blob) in rows {
            // Decode rather than trust the byte count: a row that does not read
            // back as exactly `dim` floats is from another space.
            if from_blob(&blob).len() != dim {
                continue;
            }
            conn.execute(
                &format!("INSERT OR IGNORE INTO {table}(chunk_id, embedding) VALUES(?1, ?2)"),
                params![id, blob],
            )?;
            moved += 1;
        }
    }
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {LEGACY_TABLE};"))?;
    Ok(moved)
}

/// Drop every stored vector when the embedding space changes. Keeping rows from
/// a different model or dimension would silently compare incomparable numbers.
pub fn reconcile_space(conn: &Connection, e: Option<&dyn Embedder>) -> Result<()> {
    let cur_model = crate::sqlite::meta_get(conn, "embed_model").unwrap_or_else(|| "none".into());
    let cur_dim = crate::sqlite::meta_get(conn, "embed_dim").unwrap_or_default();
    let (want_model, want_dim, provider) = match e {
        Some(e) => (e.model().to_string(), e.dim().to_string(), e.provider()),
        None => ("none".to_string(), String::new(), "none"),
    };
    if cur_model != want_model || cur_dim != want_dim {
        crate::sqlite::drop_vec_table(conn)?;
        conn.execute_batch(&format!("DROP TABLE IF EXISTS {LEGACY_TABLE};"))?;
    }
    match e {
        // The width is fixed at creation, so the table can only exist once an
        // embedder has said how wide.
        Some(e) => {
            crate::sqlite::create_vec_table(conn, e.dim())?;
            adopt_legacy(conn, e.dim())?;
        }
        // No embedder: nothing will write vectors and nothing can read the old
        // blobs, so the retired table goes rather than sitting there looking
        // like a store.
        None => conn.execute_batch(&format!("DROP TABLE IF EXISTS {LEGACY_TABLE};"))?,
    }
    crate::sqlite::meta_set(conn, "embed_model", &want_model)?;
    crate::sqlite::meta_set(conn, "embed_dim", &want_dim)?;
    crate::sqlite::meta_set(conn, "embedder", provider)?;
    Ok(())
}

/// Embed every chunk that has no vector yet. Returns how many were written.
/// An embedder error propagates: indexing reports it rather than storing
/// placeholders.
pub fn embed_missing(conn: &Connection, e: &dyn Embedder, batch: usize) -> Result<u64> {
    crate::sqlite::create_vec_table(conn, e.dim())?;
    let table = crate::sqlite::VEC_TABLE;
    let pending: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(&format!(
            "SELECT c.chunk_id, c.text FROM chunks c
             WHERE c.chunk_id NOT IN (SELECT chunk_id FROM {table})"
        ))?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect()
    };
    let mut written = 0u64;
    for group in pending.chunks(batch.max(1)) {
        let texts: Vec<String> = group.iter().map(|(_, t)| t.clone()).collect();
        let vecs = e.embed_documents(&texts)?;
        for ((id, _), v) in group.iter().zip(vecs.iter()) {
            conn.execute(
                &format!("INSERT INTO {table}(chunk_id, embedding) VALUES(?1, ?2)"),
                params![id, to_blob(v)],
            )?;
            written += 1;
        }
    }
    Ok(written)
}

/// The KNN query. `k` is how sqlite-vec is told how many neighbours to return;
/// it is part of the constraint, not a `LIMIT` applied afterwards.
pub const KNN_SQL: &str = "SELECT chunk_id FROM chunk_vec \
                           WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance";

/// Chunk ids ordered by similarity to `query`, best first.
///
/// This is a brute-force nearest-neighbour scan inside SQLite. The work stays in
/// the database instead of pulling every stored vector across the FFI boundary
/// to score it in Rust, and the ordering is the same cosine distance as before.
/// An approximate index is a later question and only if a profile asks for one.
pub fn vector_rank(conn: &Connection, e: &dyn Embedder, query: &str, top: usize) -> Result<Vec<i64>> {
    if !crate::sqlite::vec_table_exists(conn)? {
        return Ok(Vec::new());
    }
    let qv = e.embed_query(query)?;
    // A query vector of the wrong width is a bug in the embedder, not a reason
    // to ask sqlite-vec and get an error back mid-search.
    if qv.len() != e.dim() {
        return Ok(Vec::new());
    }
    let k = top.clamp(1, 1000) as i64;
    let mut stmt = conn.prepare(KNN_SQL)?;
    let ids: Vec<i64> =
        stmt.query_map(params![to_blob(&qv), k], |r| r.get(0))?.filter_map(|r| r.ok()).collect();
    Ok(ids)
}

/// Reciprocal rank fusion. `k` damps the tail so a single arm cannot dominate.
pub fn rrf(rankings: &[Vec<i64>], k: f32) -> Vec<i64> {
    let mut score: std::collections::HashMap<i64, f32> = std::collections::HashMap::new();
    for ranking in rankings {
        for (i, id) in ranking.iter().enumerate() {
            *score.entry(*id).or_insert(0.0) += 1.0 / (k + i as f32 + 1.0);
        }
    }
    let mut ids: Vec<(i64, f32)> = score.into_iter().collect();
    // ties break on chunk id so results are stable run to run
    ids.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    ids.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip() {
        let v = vec![0.5f32, -0.25, 1.0];
        assert_eq!(from_blob(&to_blob(&v)), v);
        assert_eq!(to_blob(&v).len(), v.len() * 4, "four bytes a float, which is what vec0 reads");
        assert!(from_blob(&[]).is_empty());
        // a trailing partial float is ignored rather than panicking
        assert_eq!(from_blob(&[0, 0, 0, 0, 7]).len(), 1);
    }

    /// The forbidden neighbours. v0.4 is brute force inside SQLite; an
    /// approximate index is a later question and only if a profile asks.
    #[test]
    fn the_knn_is_brute_force_and_names_no_index() {
        let sql = KNN_SQL.to_lowercase();
        for banned in ["diskann", "hnsw", "ivf", "vss", "indexed by"] {
            assert!(!sql.contains(banned), "{banned} has no business in the KNN query: {KNN_SQL}");
        }
        assert!(sql.contains("match"), "it is a MATCH constraint: {KNN_SQL}");
        assert!(sql.contains("order by distance"), "and sqlite-vec orders it: {KNN_SQL}");
    }

    #[test]
    fn rrf_prefers_agreement() {
        // 9 is second in one arm and first in the other; 1 is first in one arm
        // and absent from the other. Appearing in both beats topping one.
        let fused = rrf(&[vec![1, 9], vec![9]], 60.0);
        assert_eq!(fused[0], 9, "the document both arms found wins");
        assert_eq!(fused[1], 1);

        // Perfectly symmetric input really does tie; ties break on id so the
        // order is stable run to run rather than hash-dependent.
        assert_eq!(rrf(&[vec![1, 7, 2], vec![2, 7, 1]], 60.0), vec![1, 2, 7]);

        let single = rrf(&[vec![9, 8]], 60.0);
        assert_eq!(single, vec![9, 8], "one arm keeps its own order");
        assert!(rrf(&[], 60.0).is_empty());
    }
}
