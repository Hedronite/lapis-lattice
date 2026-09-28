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

/// GET `<lattice_url>/search` on the lattice service (the Python `tome_indexer.py` index).
/// `top_k = retrieve_limit` (≤ 50), `retrieve_k` per modality, optional `domain`.
pub async fn http_search(
    cfg: &BaselineCfg,
    query: &str,
    domain: Option<&str>,
) -> Result<(Vec<Hit>, f64), BaselineError> {
    let mode = match cfg.mode.as_str() {
        "bm25" => "bm25_only",
        "vector" => "vector_only",
        _ => "hybrid",
    };
    let mut params: Vec<(&str, String)> = vec![
        ("q", query.to_string()),
        ("top_k", cfg.retrieve_limit.to_string()),
        ("retrieve_k", cfg.retrieve_k.to_string()),
        ("mode", mode.to_string()),
    ];
    if let Some(d) = domain {
        params.push(("domain", d.to_string()));
    }
    let url = format!("{}/search", cfg.lattice_url.trim_end_matches('/'));
    let t0 = Instant::now();
    let resp = reqwest::Client::new()
        .get(&url)
        .query(&params)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| BaselineError::LatticeDown(format!("{url}: {e}")))?;
    let status = resp.status();
    let body: Value = resp.json().await.map_err(|e| BaselineError::Internal(format!("search json: {e}")))?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    if status.as_u16() == 503 {
        return Err(BaselineError::LatticeDown(body.to_string().chars().take(200).collect()));
    }
    if !status.is_success() {
        return Err(BaselineError::Internal(format!(
            "search {status}: {}",
            body.to_string().chars().take(200).collect::<String>()
        )));
    }
    Ok((parse_http_results(&body)?, ms))
}

/// Lattice `/search` `results[]` → `lapis_lattice::Hit` (text → snippet, rrf_score → score).
pub fn parse_http_results(body: &Value) -> Result<Vec<Hit>, BaselineError> {
    let rows = body
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| BaselineError::Internal("search: no results array".into()))?;
    let num = |v: &Value, k: &str| {
        v.get(k).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
    };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let path =
                text(r, "path").ok_or_else(|| BaselineError::Internal(format!("result {i} has no path")))?;
            Ok(Hit {
                kind: if path.ends_with(".pdf") { "pdf".into() } else { "markdown".into() },
                title: text(r, "title").unwrap_or_default(),
                heading: text(r, "heading"),
                snippet: text(r, "text"),
                rank: num(r, "rank").map_or(i as u32 + 1, |x| x as u32),
                score: num(r, "rrf_score").or_else(|| num(r, "score")).unwrap_or(0.0),
                domain: text(r, "domain"),
                doc_type: text(r, "doc_type"),
                tags: vec![],
                chunk_id: num(r, "chunk_id").map(|x| x as i64),
                chunk_index: num(r, "chunk_index").map(|x| x as i64),
                jev: None,
                path,
            })
        })
        .collect()
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
    fn http_results_map_to_hits() {
        let body = serde_json::json!({"results": [
            {"chunk_id": 90981, "chunk_index": 117, "heading": "The Four Golden Signals", "text": "The four golden signals",
             "path": "d.pdf", "title": "SRE", "domain": "01-Earth-DevOps", "doc_type": "tome", "rrf_score": 0.032, "rank": 1},
            {"chunk_id": "7", "path": "x.md", "title": "x", "rank": "2", "rrf_score": "0.01"}
        ]});
        let hs = parse_http_results(&body).unwrap();
        assert_eq!(
            (hs[0].chunk_id, hs[0].kind.as_str(), hs[0].snippet.as_deref()),
            (Some(90981), "pdf", Some("The four golden signals"))
        );
        assert_eq!((hs[1].chunk_id, hs[1].rank), (Some(7), 2));
        assert!(parse_http_results(&serde_json::json!({"detail": "x"})).is_err());
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
