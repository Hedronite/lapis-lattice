//! Arm A: `lapis search` over the lattice HTTP backend (Python `tome_indexer.py`
//! chunks), same-PDF filter, Jev shadow rerank, then pages via a `chunk_id` join
//! (lapis' `Hit` has no page fields).

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::answer::PassageIn;
use crate::config::BaselineCfg;
use lapis_lattice::Hit;

/// One row of `evals/tome/data/chunk-pages.jsonl` (exported read-only from lattice.db).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRow {
    pub chunk_id: i64,
    pub doc: String,
    pub doc_sha256: String,
    pub chunk_index: i64,
    pub page_start: Option<u32>,
    pub page_end: Option<u32>,
    #[serde(default)]
    pub chapter: Option<String>,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub char_count: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkMap {
    rows: HashMap<i64, ChunkRow>,
}

impl ChunkMap {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut rows = HashMap::new();
        for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let r: ChunkRow =
                serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
            rows.insert(r.chunk_id, r);
        }
        Ok(Self { rows })
    }

    pub fn from_rows(rows: Vec<ChunkRow>) -> Self {
        Self { rows: rows.into_iter().map(|r| (r.chunk_id, r)).collect() }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Pages for a hit, only if the chunk belongs to `doc_sha256` and has pages.
    pub fn pages(&self, hit: &Hit, doc_sha256: &str) -> Option<(u32, u32)> {
        let r = self.rows.get(&hit.chunk_id?)?;
        if r.doc_sha256 != doc_sha256 {
            return None;
        }
        Some((r.page_start?, r.page_end.or(r.page_start)?))
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BaselineError {
    #[error("lattice down: {0}")]
    LatticeDown(String),
    #[error("no hits for this PDF in the top {0}")]
    NoHitsForDoc(u32),
    #[error("chunk_id join miss for {0} hit(s): chunk map is stale or the index changed")]
    ChunkJoinMiss(usize),
    #[error("lapis search failed: {0}")]
    Internal(String),
}

impl BaselineError {
    pub fn kind(&self) -> &'static str {
        match self {
            BaselineError::LatticeDown(_) => "lattice_down",
            BaselineError::NoHitsForDoc(_) => "no_hits_for_doc",
            BaselineError::ChunkJoinMiss(_) => "chunk_join_miss",
            BaselineError::Internal(_) => "internal",
        }
    }
}

/// `lapis --json --lattice <url> search <q> -n <retrieve_limit> --mode <mode>`.
pub async fn lapis_search(cfg: &BaselineCfg, query: &str) -> Result<(Vec<Hit>, f64), BaselineError> {
    let t0 = Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(&cfg.lapis_bin)
            .args(["--json", "--lattice", &cfg.lattice_url, "search"])
            .arg(query)
            .args(["-n", &cfg.retrieve_limit.to_string(), "--mode", &cfg.mode])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| BaselineError::LatticeDown("lapis search timed out".into()))?
    .map_err(|e| BaselineError::Internal(format!("spawn {}: {e}", cfg.lapis_bin)))?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let hits = parse_search_envelope(&out.stdout)?;
    Ok((hits, ms))
}

pub fn parse_search_envelope(stdout: &[u8]) -> Result<Vec<Hit>, BaselineError> {
    let v: Value = serde_json::from_slice(stdout).map_err(|e| {
        BaselineError::Internal(format!(
            "envelope json: {e}: {}",
            String::from_utf8_lossy(stdout).chars().take(200).collect::<String>()
        ))
    })?;
    if v.get("ok").and_then(Value::as_bool) != Some(true) {
        let kind = v.pointer("/error/kind").and_then(Value::as_str).unwrap_or("");
        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("").to_string();
        return Err(if kind == "lattice_down" {
            BaselineError::LatticeDown(msg)
        } else {
            BaselineError::Internal(format!("{kind}: {msg}"))
        });
    }
    let hits = v.pointer("/data/hits").cloned().unwrap_or(Value::Array(vec![]));
    serde_json::from_value(hits).map_err(|e| BaselineError::Internal(format!("hits: {e}")))
}

/// Same-PDF filter, keep retrieve order, cap at `top_k`.
pub fn same_pdf(hits: Vec<Hit>, doc: &str, top_k: u32) -> Vec<Hit> {
    hits.into_iter().filter(|h| h.path == doc).take(top_k as usize).collect()
}

/// Join hits to pages. Any miss is an explicit error (never a silently page-less passage).
pub fn to_passages(hits: &[Hit], map: &ChunkMap, doc_sha256: &str) -> Result<Vec<PassageIn>, BaselineError> {
    let mut out = Vec::with_capacity(hits.len());
    let mut misses = 0;
    for h in hits {
        match map.pages(h, doc_sha256) {
            Some((a, b)) => out.push(PassageIn {
                label: format!("chunk {}", h.chunk_id.unwrap_or_default()),
                page_start: a,
                page_end: b,
                text: h.snippet.clone().unwrap_or_default(),
            }),
            None => misses += 1,
        }
    }
    if misses > 0 {
        return Err(BaselineError::ChunkJoinMiss(misses));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, chunk: i64) -> Hit {
        Hit {
            path: path.into(),
            kind: "pdf".into(),
            title: "t".into(),
            heading: None,
            snippet: Some(format!("text {chunk}")),
            rank: 1,
            score: 0.0,
            domain: None,
            doc_type: None,
            tags: vec![],
            chunk_id: Some(chunk),
            chunk_index: None,
            jev: None,
        }
    }

    fn row(id: i64, sha: &str, a: u32, b: u32) -> ChunkRow {
        ChunkRow {
            chunk_id: id,
            doc: "d.pdf".into(),
            doc_sha256: sha.into(),
            chunk_index: 0,
            page_start: Some(a),
            page_end: Some(b),
            chapter: None,
            section: None,
            char_count: None,
        }
    }

    #[test]
    fn join_on_chunk_id_and_fail_on_miss() {
        let m = ChunkMap::from_rows(vec![row(1, "s", 86, 87), row(2, "other", 1, 1)]);
        let p = to_passages(&[hit("d.pdf", 1)], &m, "s").unwrap();
        assert_eq!((p[0].page_start, p[0].page_end), (86, 87));
        assert_eq!(to_passages(&[hit("d.pdf", 2)], &m, "s").unwrap_err().kind(), "chunk_join_miss");
        assert_eq!(to_passages(&[hit("d.pdf", 9)], &m, "s").unwrap_err().kind(), "chunk_join_miss");
    }

    #[test]
    fn same_pdf_filter_keeps_order_and_caps() {
        let hs = vec![hit("x.md", 1), hit("d.pdf", 2), hit("d.pdf", 3), hit("d.pdf", 4)];
        let f = same_pdf(hs, "d.pdf", 2);
        assert_eq!(f.iter().map(|h| h.chunk_id.unwrap()).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn envelope_errors_are_typed() {
        let down =
            br#"{"ok":false,"data":null,"error":{"kind":"lattice_down","message":"refused","exit":2}}"#;
        assert_eq!(parse_search_envelope(down).unwrap_err().kind(), "lattice_down");
        let ok = br#"{"ok":true,"data":{"hits":[{"path":"d.pdf","kind":"pdf","title":"t","rank":1,"score":0.1,"domain":null,"doc_type":"tome","chunk_id":7}]},"meta":{}}"#;
        let hs = parse_search_envelope(ok).unwrap();
        assert_eq!(hs[0].chunk_id, Some(7));
    }

    #[test]
    fn committed_chunk_map_covers_the_four_pdfs() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tome/data/chunk-pages.jsonl");
        let m = ChunkMap::load(&p).unwrap();
        assert!(m.len() > 1000, "{}", m.len());
        assert!(m.rows.values().all(|r| r.page_start.is_some()));
    }
}
