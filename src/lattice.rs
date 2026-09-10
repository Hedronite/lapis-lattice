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
    /// Per-arm breakdown (serve.py ≥ 2026-09-09); absent on older lattices.
    #[serde(default)]
    latency: Option<Latency>,
    #[serde(default)]
    results: Vec<RawHit>,
}

/// Per-arm search latency in milliseconds. An arm the lattice skipped is `0`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Latency {
    #[serde(default)]
    pub embed: f64,
    #[serde(default)]
    pub bm25: f64,
    #[serde(default)]
    pub vector: f64,
    #[serde(default)]
    pub title: f64,
    #[serde(default)]
    pub fuse: f64,
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
    /// 1-based position in this result list; always present.
    pub rank: Option<u32>,
    /// Always present (`null` when the note has no HAL domain).
    pub domain: Option<String>,
    /// Always present (`null` when the note has no HAL type).
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
    /// Per-arm breakdown; zeros when the lattice did not report one.
    pub latency: Latency,
    pub count: usize,
    pub hits: Vec<Hit>,
}

/// One row from `/neighbors`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbor {
    /// Resolved vault path. `None` on a dangling link; see `dst_raw`.
    #[serde(default)]
    pub path: Option<String>,
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

impl Neighbor {
    /// What to show for this edge: the resolved path, else the raw link target.
    pub fn label(&self) -> &str {
        self.path.as_deref().or(self.dst_raw.as_deref()).unwrap_or("?")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbors {
    pub path: String,
    pub direction: String,
    pub hop: u32,
    pub neighbors: Vec<Neighbor>,
}

/// One row of `GET /graph/ego` (N17). `depth` 1 = direct neighbor, 2 = neighbor of a
/// depth-1 node; `via` is the node it was reached through. Dangling rows have `path: null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EgoRow {
    pub depth: u32,
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default, rename = "dir")]
    pub direction: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub dst_raw: Option<String>,
    #[serde(default, deserialize_with = "int_or_bool")]
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ego {
    pub path: String,
    pub direction: String,
    pub hops: u32,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub rows: Vec<EgoRow>,
}

/// Names `GET /analytics?query=` accepts (N18). Anything else is refused
/// client-side as a usage error before any HTTP.
pub const ANALYTICS_QUERIES: [&str; 9] =
    ["inventory", "priority", "tags", "health", "recent", "hubs", "density", "degree", "dangling"];

pub fn check_analytics_query(q: &str) -> Result<()> {
    if ANALYTICS_QUERIES.contains(&q) {
        Ok(())
    } else {
        Err(LapisError::Usage(format!(
            "unknown analytics query {q:?}; one of: {}",
            ANALYTICS_QUERIES.join(", ")
        )))
    }
}

/// `GET /analytics` result: a small table plus any query-specific extras
/// (`health` adds index_state / counts).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analytics {
    pub query: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub rows: Vec<Vec<Value>>,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// `GET /tree` (N19): hub-routed out-link walk from a seed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeNode {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub doc_type: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    pub depth: u32,
    #[serde(default)]
    pub is_hub: bool,
    #[serde(default)]
    pub out_degree: u64,
    /// nomic similarity to `query` when one was given; `null` otherwise.
    #[serde(default)]
    pub score: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TreeEdge {
    pub src: String,
    pub dst: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub seed: String,
    #[serde(default)]
    pub query: Option<String>,
    pub depth: u32,
    #[serde(default)]
    pub max_nodes: u32,
    #[serde(default)]
    pub hubs: Vec<String>,
    #[serde(default)]
    pub ranked: bool,
    #[serde(default)]
    pub nodes: Vec<TreeNode>,
    #[serde(default)]
    pub edges: Vec<TreeEdge>,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub truncated: bool,
}

pub const TREE_MAX_DEPTH: u32 = 3;
pub const TREE_MAX_NODES: u32 = 200;

/// One row from `GET /documents` (metadata only, no chunks or embeddings).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub path: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<f64>,
    /// Content fingerprint from the embedded index, so an agent can plan
    /// `--if-hash` writes from one `list` call. `None` over HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ListParams {
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    pub prefix: Option<String>,
    pub limit: u32,
    pub offset: u32,
    pub include_archives: bool,
}

/// `POST /reindex` result. `ok=false` with a 500 is surfaced as an error by
/// the client; this struct is what a 200 carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reindex {
    pub ok: bool,
    pub path: String,
    #[serde(default)]
    pub changed: Option<bool>,
    #[serde(default)]
    pub chunks: Option<u64>,
    #[serde(default)]
    pub indexer_exit: Option<i32>,
    #[serde(default)]
    pub elapsed_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stdout_tail: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stderr_tail: Vec<String>,
}

/// serve.py error bodies are `{"detail": "..."}`; pull the text out.
fn lattice_detail(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("detail").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.to_string())
}

/// Non-2xx from the lattice → the exit-code contract. 404 is "that note is not
/// in the index" (exit 3, same as `read` on a missing file); other 4xx are our
/// own bad params (exit 1); 5xx is the lattice being down (exit 2).
fn http_error(route: &str, status: reqwest::StatusCode, body: &str) -> LapisError {
    let body = body.trim();
    let detail = if body.is_empty() { String::new() } else { format!(": {}", lattice_detail(body)) };
    let msg = format!("lattice {route} {status}{detail}");
    if status == reqwest::StatusCode::NOT_FOUND {
        LapisError::Path(msg)
    } else if status.is_client_error() {
        LapisError::Usage(msg)
    } else {
        LapisError::LatticeDown(msg)
    }
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
            return Err(http_error(route, status, &body));
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
            .enumerate()
            .map(|(i, r)| {
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
                    rank: Some(r.rank.unwrap_or(i as u32 + 1)),
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
            latency: raw.latency.unwrap_or_default(),
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

    /// `GET /graph/ego`: hop-1 or hop-2 ego graph (serve.py rejects hops > 2).
    pub async fn ego(&self, path: &str, hops: u32, direction: &str, resolved_only: bool) -> Result<Ego> {
        if !(1..=2).contains(&hops) {
            return Err(LapisError::Usage(format!("hops must be 1 or 2, got {hops}")));
        }
        let query = vec![
            ("path", path.to_string()),
            ("hops", hops.to_string()),
            ("direction", direction.to_string()),
            ("resolved", if resolved_only { "1" } else { "0" }.to_string()),
        ];
        self.get_json("/graph/ego", &query).await
    }

    /// `GET /analytics?query=`: one of [`ANALYTICS_QUERIES`], run read-only in DuckDB.
    pub async fn analytics(&self, query: &str) -> Result<Analytics> {
        check_analytics_query(query)?;
        self.get_json("/analytics", &[("query", query.to_string())]).await
    }

    /// `GET /tree`: hub-routed walk. Depth and node caps are clamped client-side
    /// to what serve.py accepts.
    pub async fn tree(
        &self,
        path: Option<&str>,
        query: Option<&str>,
        depth: u32,
        max_nodes: u32,
    ) -> Result<Tree> {
        if path.is_none_or(|p| p.trim().is_empty()) && query.is_none_or(|q| q.trim().is_empty()) {
            return Err(LapisError::Usage("tree-retrieve needs --path or --query".into()));
        }
        let mut q: Vec<(&str, String)> = vec![
            ("depth", depth.clamp(1, TREE_MAX_DEPTH).to_string()),
            ("max_nodes", max_nodes.clamp(1, TREE_MAX_NODES).to_string()),
        ];
        if let Some(p) = path.filter(|p| !p.trim().is_empty()) {
            q.push(("path", p.to_string()));
        }
        if let Some(s) = query.filter(|s| !s.trim().is_empty()) {
            q.push(("query", s.to_string()));
        }
        self.get_json("/tree", &q).await
    }
}

impl Client {
    /// `GET /documents`: list notes from the lattice table. Never walks the vault.
    pub async fn documents(&self, p: &ListParams) -> Result<Vec<Document>> {
        let mut query: Vec<(&str, String)> =
            vec![("limit", p.limit.clamp(1, 1000).to_string()), ("offset", p.offset.to_string())];
        for (k, v) in [
            ("domain", &p.domain),
            ("doc_type", &p.doc_type),
            ("status", &p.status),
            ("tag", &p.tag),
            ("prefix", &p.prefix),
        ] {
            if let Some(v) = v {
                query.push((k, v.clone()));
            }
        }
        if p.include_archives {
            query.push(("include_archives", "true".into()));
        }
        self.get_json("/documents", &query).await
    }

    /// `POST /reindex?path=`: ask the lattice to run its own indexer on one
    /// note. The indexer is the only writer of `lattice.db`; this binary
    /// never inserts. Used by L2 after create/append.
    pub async fn reindex(&self, rel_path: &str) -> Result<Reindex> {
        let url = format!("{}/reindex", self.base);
        let resp = self
            .http
            .post(&url)
            .query(&[("path", rel_path)])
            .send()
            .await
            .map_err(|e| LapisError::LatticeDown(describe_reqwest(&url, &e)))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            return serde_json::from_str::<Reindex>(&text)
                .map_err(|e| LapisError::LatticeDown(format!("lattice /reindex: bad JSON: {e}")));
        }
        // A 500 from /reindex carries the same body with indexer output; keep it readable.
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("detail").and_then(|d| d.as_str().map(str::to_string)).or_else(|| {
                    v.get("stderr_tail")
                        .and_then(|t| t.as_array())
                        .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" | "))
                })
            })
            .unwrap_or_else(|| text.trim().to_string());
        if status.is_client_error() {
            Err(LapisError::Usage(format!("lattice /reindex {status}: {detail}")))
        } else {
            Err(LapisError::LatticeDown(format!("lattice /reindex {status}: {detail}")))
        }
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
        assert!(raw.latency.is_none(), "older lattice: no breakdown");
    }

    /// N9 / N10: latency breakdown decodes with zeros for skipped arms; hit
    /// JSON always carries rank, domain, doc_type.
    #[test]
    fn latency_breakdown_and_hit_keys() {
        let raw: RawSearch = serde_json::from_str(
            r#"{"latency_ms":40.5,"latency":{"embed":12.0,"bm25":3.5,"fuse":1.0},"results":[]}"#,
        )
        .unwrap();
        let l = raw.latency.unwrap();
        assert_eq!(l, Latency { embed: 12.0, bm25: 3.5, vector: 0.0, title: 0.0, fuse: 1.0 });
        let hit = Hit {
            path: "a.md".into(),
            kind: Kind::Markdown,
            title: "A".into(),
            heading: None,
            snippet: None,
            score: None,
            rank: Some(1),
            domain: None,
            doc_type: None,
            tags: vec![],
            chunk_id: None,
            chunk_index: None,
        };
        let j = serde_json::to_value(&hit).unwrap();
        assert_eq!(j["rank"], 1);
        assert!(j.get("domain").is_some() && j["domain"].is_null());
        assert!(j.get("doc_type").is_some() && j["doc_type"].is_null());
        assert!(j.get("score").is_none());
        let empty = serde_json::to_value(Latency::default()).unwrap();
        assert_eq!(empty["vector"], 0.0);
    }

    #[test]
    fn neighbors_resolved_accepts_int() {
        let n: Neighbors = serde_json::from_str(
            r#"{"path":"a.md","direction":"out","hop":1,"neighbors":[{"path":"b.md","dst_raw":"b","alias":null,"anchor":null,"resolved":1,"dir":"out"}]}"#,
        )
        .unwrap();
        assert!(n.neighbors[0].resolved);
        assert_eq!(n.neighbors[0].direction, "out");
        assert_eq!(n.neighbors[0].path.as_deref(), Some("b.md"));
    }

    /// N1: serve.py emits `"path": null` on unresolved edges (`resolved=0`).
    #[test]
    fn neighbors_null_path_is_a_dangling_row() {
        let n: Neighbors = serde_json::from_str(
            r#"{"path":"Cross-References/Manual.md","direction":"both","hop":1,"neighbors":[
                {"path":"notes/Capital.md","dst_raw":"Capital","alias":null,"anchor":null,"resolved":1,"dir":"out"},
                {"path":null,"dst_raw":"aes_schema_genesis_canon","alias":null,"anchor":null,"resolved":0,"dir":"out"}]}"#,
        )
        .expect("null path must decode");
        assert_eq!(n.neighbors.len(), 2);
        let d = &n.neighbors[1];
        assert!(d.path.is_none());
        assert!(!d.resolved);
        assert_eq!(d.label(), "aes_schema_genesis_canon");
        assert_eq!(n.neighbors[0].label(), "notes/Capital.md");
        // round-trips with an explicit null so agents can tell dangling from resolved
        let out = serde_json::to_value(d).unwrap();
        assert!(out["path"].is_null());
        assert_eq!(out["dst_raw"], "aes_schema_genesis_canon");
    }

    /// N18: the allowlist is enforced before any HTTP.
    #[test]
    fn analytics_allowlist_rejects_unknown() {
        for q in ANALYTICS_QUERIES {
            check_analytics_query(q).unwrap();
        }
        for bad in ["select 1", "DROP TABLE documents", "inventory;", "", "Inventory"] {
            let e = check_analytics_query(bad).unwrap_err();
            assert_eq!((e.exit_code(), e.kind()), (1, "usage"), "{bad:?}");
        }
        let a: Analytics = serde_json::from_str(
            r#"{"query":"degree","columns":["path","degree"],"rows":[["a.md",3]],"count":1,"truncated":false,"latency_ms":2.5}"#,
        )
        .unwrap();
        assert_eq!(a.rows[0][1], 3);
        let h: Analytics = serde_json::from_str(
            r#"{"query":"health","columns":[],"rows":[],"count":0,"truncated":false,"documents":4619,"index_state":{"embedding_model":"nomic-embed-text"}}"#,
        )
        .unwrap();
        assert_eq!(h.extra["documents"], 4619);
        assert_eq!(h.extra["index_state"]["embedding_model"], "nomic-embed-text");
    }

    /// N17: a hops=2 ego fixture, as serve.py emits it, including a dangling row.
    #[test]
    fn ego_hops2_fixture() {
        let e: Ego = serde_json::from_str(
            r#"{"path":"Cross-References/Manual.md","direction":"both","hops":2,"count":3,"truncated":false,"rows":[
              {"depth":1,"via":"Cross-References/Manual.md","dir":"out","path":"Cross-References/Capital.md","dst_raw":"Capital","resolved":1},
              {"depth":1,"via":"Cross-References/Manual.md","dir":"out","path":null,"dst_raw":"aes_schema_genesis_canon","resolved":0},
              {"depth":2,"via":"Cross-References/Capital.md","dir":"out","path":"ideas/capital/x.md","dst_raw":"ideas/capital/x","resolved":1}]}"#,
        )
        .unwrap();
        assert_eq!((e.hops, e.count, e.rows.len()), (2, 3, 3));
        assert_eq!(e.rows[0].depth, 1);
        assert!(e.rows[1].path.is_none() && !e.rows[1].resolved);
        let two = &e.rows[2];
        assert_eq!(
            (two.depth, two.via.as_deref(), two.direction.as_str()),
            (2, Some("Cross-References/Capital.md"), "out")
        );
        assert_eq!(two.path.as_deref(), Some("ideas/capital/x.md"));
        // wire shape keeps `dir`
        let j = serde_json::to_value(two).unwrap();
        assert_eq!(j["dir"], "out");
    }

    /// N19: tree fixture with the node cap hit → `truncated: true`.
    #[test]
    fn tree_fixture_truncated_flag() {
        let t: Tree = serde_json::from_str(
            r#"{"seed":"Cross-References/Manual.md","query":"lattice","depth":2,"max_nodes":2,"hubs":["Cross-References/Manual.md"],"ranked":true,
              "nodes":[{"path":"Cross-References/Manual.md","title":"Manual","doc_type":"hub","domain":null,"depth":0,"is_hub":true,"out_degree":8,"score":0.05},
                       {"path":"ideas/x.md","depth":1,"is_hub":false,"out_degree":0,"score":null}],
              "edges":[{"src":"Cross-References/Manual.md","dst":"ideas/x.md"}],"count":2,"truncated":true}"#,
        )
        .unwrap();
        assert!(t.truncated);
        assert_eq!((t.count, t.nodes.len(), t.edges.len(), t.hubs.len()), (2, 2, 1, 1));
        assert!(t.ranked && t.nodes[0].is_hub && t.nodes[0].score == Some(0.05));
        assert_eq!(t.nodes[1].depth, 1);
        assert!(t.nodes[1].score.is_none() && t.nodes[1].title.is_none());
        let full: Tree = serde_json::from_str(r#"{"seed":"a.md","depth":1,"nodes":[],"edges":[]}"#).unwrap();
        assert!(!full.truncated && full.hubs.is_empty());
    }

    /// N5: lattice 404 is exit 3 like `read`; other 4xx exit 1; 5xx exit 2.
    #[test]
    fn http_status_maps_to_exit_codes() {
        use reqwest::StatusCode;
        let e =
            http_error("/neighbors", StatusCode::NOT_FOUND, r#"{"detail":"document not found: nope.md"}"#);
        assert_eq!(e.exit_code(), 3);
        assert_eq!(e.kind(), "path");
        assert_eq!(e.message(), "lattice /neighbors 404 Not Found: document not found: nope.md");
        let e = http_error("/neighbors", StatusCode::BAD_REQUEST, r#"{"detail":"hop=1 only"}"#);
        assert_eq!((e.exit_code(), e.kind()), (1, "usage"));
        let e = http_error("/search", StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert_eq!((e.exit_code(), e.kind()), (2, "lattice_down"));
        assert_eq!(e.message(), "lattice /search 500 Internal Server Error: boom");
        let e = http_error("/healthz", StatusCode::BAD_GATEWAY, "");
        assert_eq!(e.message(), "lattice /healthz 502 Bad Gateway");
    }

    #[test]
    fn document_row_tolerates_nulls() {
        let d: Document = serde_json::from_str(
            r#"{"path":"a/b.md","title":null,"domain":null,"doc_type":"note","status":null,"priority":null,"tags":[],"mtime":null}"#,
        )
        .unwrap();
        assert_eq!(d.path, "a/b.md");
        assert!(d.title.is_none());
        assert!(d.tags.is_empty());
    }

    #[test]
    fn reindex_body_parses() {
        let r: Reindex = serde_json::from_str(
            r#"{"ok":true,"path":"notes/STATUS.md","changed":true,"chunks":7,"indexer_exit":0,"elapsed_ms":900.5,"indexer":"x","stdout_tail":["a"],"stderr_tail":[]}"#,
        )
        .unwrap();
        assert!(r.ok);
        assert_eq!(r.chunks, Some(7));
        assert_eq!(r.changed, Some(true));
    }

    #[tokio::test]
    async fn reindex_unreachable_is_exit_2() {
        let c = Client::new("http://127.0.0.1:9", Duration::from_millis(500)).unwrap();
        let e = c.reindex("a.md").await.unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
    }

    #[tokio::test]
    async fn unreachable_lattice_is_exit_2() {
        // Port 9 (discard) is closed on macOS by default; connection refused.
        let c = Client::new("http://127.0.0.1:9", Duration::from_millis(500)).unwrap();
        let e = c.health().await.unwrap_err();
        assert_eq!(e.exit_code(), 2, "{e}");
    }
}
