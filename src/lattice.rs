//! HTTP client for Lapis Lattice (`projects/atrium-lattice/serve.py`).
//!
//! Read-only by construction: this module only issues `GET`s to `/healthz`,
//! `/search`, and `/neighbors`. It never opens `lattice.db`. Index writes
//! (`POST /reindex`) arrive in L1/L2 and still go through HTTP.
//!
//! Any transport failure, timeout, or non-2xx maps to
//! [`LapisError::LatticeDown`] (exit 2).

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{LapisError, Result};
use crate::notes::{Kind, kind_of};

#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    http: reqwest::Client,
}

/// `GET /healthz` as served today.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    #[serde(default)]
    pub documents_indexed: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub dangling_links: u64,
    #[serde(default)]
    pub unresolved_collisions: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_url: Option<String>,
    /// Anything serve.py adds later is carried through, not dropped.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// CLI-facing search mode. Maps onto serve.py's `hybrid|bm25_only|vector_only`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Hybrid,
    Bm25,
    Vector,
}

impl Mode {
    pub fn wire(self) -> &'static str {
        match self {
            Mode::Hybrid => "hybrid",
            Mode::Bm25 => "bm25_only",
            Mode::Vector => "vector_only",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchParams {
    pub query: String,
    pub top_k: u32,
    pub domain: Option<String>,
    pub mode: Mode,
    pub per_doc: bool,
    pub mmr: bool,
    pub include_archives: bool,
}

/// One raw chunk row from `/search`. Field names are serve.py's.
#[derive(Debug, Clone, Deserialize)]
struct RawHit {
    path: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    heading: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    rrf_score: Option<f64>,
    #[serde(default)]
    score: Option<f64>,
    #[serde(default)]
    rank: Option<u32>,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    doc_type: Option<String>,
    #[serde(default)]
    chunk_id: Option<i64>,
    #[serde(default)]
    chunk_index: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawSearch {
    #[serde(default)]
    modalities: Vec<String>,
    #[serde(default)]
    latency_ms: Option<f64>,
    #[serde(default)]
    results: Vec<RawHit>,
}

/// Search hit per `schema/search-hit.schema.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub path: String,
    pub kind: Kind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_index: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub mode: Mode,
    /// Modalities the lattice fused for this query (`bm25`, `vector`, `title`).
    pub modalities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    pub count: usize,
    pub hits: Vec<Hit>,
}

/// One row from `/neighbors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_raw: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(default, deserialize_with = "int_or_bool")]
    pub resolved: bool,
    #[serde(default, rename = "dir")]
    pub direction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbors {
    pub path: String,
    pub direction: String,
    pub hop: u32,
    pub neighbors: Vec<Neighbor>,
}

fn int_or_bool<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<bool, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(match v {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

fn normalize_modality(m: &str) -> String {
    match m {
        "bm25_folded" | "bm25_only" => "bm25".to_string(),
        "vector_only" => "vector".to_string(),
        other => other.to_string(),
    }
}

impl Client {
    pub fn new(base: &str, timeout: Duration) -> Result<Self> {
        let base = base.trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(LapisError::Usage(format!("lattice url must be http(s): {base}")));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(3)))
            .user_agent(concat!("lapis/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| LapisError::Internal(format!("http client: {e}")))?;
        Ok(Self { base, http })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        route: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{}{}", self.base, route);
        let resp = self
            .http
            .get(&url)
            .query(query)
            .send()
            .await
            .map_err(|e| LapisError::LatticeDown(describe_reqwest(&url, &e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let body = body.trim();
            let detail = if body.is_empty() { String::new() } else { format!(": {body}") };
            // 4xx on our own route call is a user error (bad path/param),
            // not the lattice being down.
            return if status.is_client_error() {
                Err(LapisError::Usage(format!("lattice {route} {status}{detail}")))
            } else {
                Err(LapisError::LatticeDown(format!("lattice {route} {status}{detail}")))
            };
        }
        resp.json::<T>().await.map_err(|e| LapisError::LatticeDown(format!("lattice {route}: bad JSON: {e}")))
    }

    pub async fn health(&self) -> Result<Health> {
        self.get_json("/healthz", &[]).await
    }

    pub async fn search(&self, p: &SearchParams) -> Result<SearchResult> {
        let q = p.query.trim();
        if q.is_empty() {
            return Err(LapisError::Usage("search requires a query".into()));
        }
        let top_k = p.top_k.clamp(1, 50);
        let mut query: Vec<(&str, String)> =
            vec![("q", q.to_string()), ("top_k", top_k.to_string()), ("mode", p.mode.wire().to_string())];
        if let Some(d) = &p.domain {
            query.push(("domain", d.clone()));
        }
        if p.per_doc {
            query.push(("per_doc", "true".into()));
        }
        if p.mmr {
            query.push(("mmr", "true".into()));
        }
        if p.include_archives {
            query.push(("include_archives", "true".into()));
        }
        let raw: RawSearch = self.get_json("/search", &query).await?;
        let hits: Vec<Hit> = raw
            .results
            .into_iter()
            .map(|r| {
                let kind = kind_of(&r.path);
                let title = r
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| crate::notes::stem_of(&r.path));
                Hit {
                    kind,
                    title,
                    heading: r.heading.filter(|h| !h.is_empty()),
                    snippet: r.text.filter(|t| !t.is_empty()),
                    score: r.rrf_score.or(r.score),
                    rank: r.rank,
                    domain: r.domain,
                    doc_type: r.doc_type,
                    tags: r.tags,
                    chunk_id: r.chunk_id,
                    chunk_index: r.chunk_index,
                    path: r.path,
                }
            })
            .collect();
        Ok(SearchResult {
            query: q.to_string(),
            mode: p.mode,
            modalities: raw.modalities.iter().map(|m| normalize_modality(m)).collect(),
            latency_ms: raw.latency_ms,
            count: hits.len(),
            hits,
        })
    }

    /// Hop-1 only; serve.py rejects `hop>1`.
    pub async fn neighbors(&self, path: &str, direction: &str, resolved_only: bool) -> Result<Neighbors> {
        let query = vec![
            ("path", path.to_string()),
            ("direction", direction.to_string()),
            ("resolved", if resolved_only { "1" } else { "0" }.to_string()),
            ("hop", "1".to_string()),
        ];
        self.get_json("/neighbors", &query).await
    }
}

fn describe_reqwest(url: &str, e: &reqwest::Error) -> String {
    if e.is_timeout() {
        format!("lattice timed out: {url}")
    } else if e.is_connect() {
        format!("lattice unreachable: {url} (is serve.py running? try `launchctl kickstart`)")
    } else {
        format!("lattice request failed: {url}: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_wire_names() {
        assert_eq!(Mode::Hybrid.wire(), "hybrid");
        assert_eq!(Mode::Bm25.wire(), "bm25_only");
        assert_eq!(Mode::Vector.wire(), "vector_only");
    }

    #[test]
    fn modality_names_match_schema_enum() {
        assert_eq!(normalize_modality("bm25_folded"), "bm25");
        assert_eq!(normalize_modality("vector"), "vector");
        assert_eq!(normalize_modality("title"), "title");
    }

    #[test]
    fn client_rejects_non_http_base() {
        assert!(matches!(Client::new("127.0.0.1:8080", Duration::from_secs(1)), Err(LapisError::Usage(_))));
        let c = Client::new("http://127.0.0.1:8080/", Duration::from_secs(1)).unwrap();
        assert_eq!(c.base(), "http://127.0.0.1:8080");
    }

    #[test]
    fn health_carries_unknown_fields() {
        let h: Health = serde_json::from_str(
            r#"{"status":"ok","documents_indexed":4615,"edges":11694,"dangling_links":637,"unresolved_collisions":637,"db_path":"/x/lattice.db","ollama_url":"http://localhost:11434","new_thing":1}"#,
        )
        .unwrap();
        assert_eq!(h.documents_indexed, 4615);
        assert_eq!(h.extra["new_thing"], 1);
    }

    #[test]
    fn raw_search_hit_maps_to_schema_hit() {
        let raw: RawSearch = serde_json::from_str(
            r#"{"query":"x","modalities":["vector","bm25_folded","title"],"latency_ms":12.5,"results":[{"chunk_id":1,"chunk_index":0,"heading":"H","text":"T","path":"a/b.md","title":"B","domain":"d","doc_type":"t","tags":[],"rrf_score":0.05,"rank":1}]}"#,
        )
        .unwrap();
        assert_eq!(raw.results.len(), 1);
        let r = &raw.results[0];
        assert_eq!(r.rrf_score, Some(0.05));
        assert_eq!(kind_of(&r.path), Kind::Markdown);
    }

    #[test]
    fn neighbors_resolved_accepts_int() {
        let n: Neighbors = serde_json::from_str(
            r#"{"path":"a.md","direction":"out","hop":1,"neighbors":[{"path":"b.md","dst_raw":"b","alias":null,"anchor":null,"resolved":1,"dir":"out"}]}"#,
        )
        .unwrap();
        assert!(n.neighbors[0].resolved);
        assert_eq!(n.neighbors[0].direction, "out");
    }

    #[tokio::test]
    async fn unreachable_lattice_is_exit_2() {
        // Port 9 (discard) is closed on macOS by default; connection refused.
        let c = Client::new("http://127.0.0.1:9", Duration::from_millis(500)).unwrap();
        let e = c.health().await.unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
    }
}
