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

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
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
        conn.execute("DELETE FROM embeddings", [])?;
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
    let mut stmt = conn.prepare(
        "SELECT c.chunk_id, c.text FROM chunks c
         LEFT JOIN embeddings v ON v.chunk_id = c.chunk_id
         WHERE v.chunk_id IS NULL",
    )?;
    let pending: Vec<(i64, String)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
    let mut written = 0u64;
    for group in pending.chunks(batch.max(1)) {
        let texts: Vec<String> = group.iter().map(|(_, t)| t.clone()).collect();
        let vecs = e.embed_documents(&texts)?;
        for ((id, _), v) in group.iter().zip(vecs.iter()) {
            conn.execute(
                "INSERT OR REPLACE INTO embeddings(chunk_id, vec) VALUES(?1, ?2)",
                params![id, to_blob(v)],
            )?;
            written += 1;
        }
    }
    Ok(written)
}

/// Chunk ids ordered by similarity to `query`, best first.
pub fn vector_rank(conn: &Connection, e: &dyn Embedder, query: &str, top: usize) -> Result<Vec<i64>> {
    let qv = e.embed_query(query)?;
    let mut stmt = conn.prepare("SELECT chunk_id, vec FROM embeddings")?;
    let mut scored: Vec<(i64, f32)> = stmt
        .query_map([], |r| {
            let id: i64 = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            Ok((id, cosine(&qv, &from_blob(&blob))))
        })?
        .filter_map(|r| r.ok())
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(top);
    Ok(scored.into_iter().map(|(id, _)| id).collect())
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
    fn blob_roundtrip_and_cosine() {
        let v = vec![0.5f32, -0.25, 1.0];
        assert_eq!(from_blob(&to_blob(&v)), v);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // mismatched dimensions score zero rather than panicking
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
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
