//! One retrieval surface, two implementations.
//!
//! `Embedded` is the default (SPEC-V02 D-V02-BACKEND): an in-process
//! `lapis-lattice` engine over `<vault>/.lapis/lattice.sqlite`, so a stranger
//! gets search with no daemon and no listening socket. `Http` is opt-in via
//! `lattice.mode = "http"` and keeps the operator's existing lattice working.
//!
//! Both arms return the same CLI wire types, so `main` and `mcp` never fork.

use std::sync::{Arc, Mutex};

use lapis_lattice::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::error::{LapisError, Result};
use crate::lattice::{self, Client, Hit, ListParams, Mode, Neighbors, SearchParams, SearchResult};
use crate::notes;

/// What `lapis doctor` and `vault info` report, per `schema/v0.2/health.schema.json`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Health {
    pub status: String,
    pub documents_indexed: u64,
    pub db_path: String,
    /// Resolved, not requested: `none` until vectors land (slice 2).
    pub embedder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed_dim: Option<u32>,
    pub graph: Graph,
}

/// `built` separates "never indexed" from "indexed, and this vault has no links".
#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub built: bool,
    pub edges: u64,
    pub dangling_links: u64,
}

/// The engine holds a rusqlite `Connection`, which is `Send` but not `Sync`.
/// MCP hands its futures to a runtime that requires `Send`, so the engine lives
/// behind a mutex: every embedded arm below locks, works, and releases without
/// ever holding the guard across an `await`.
pub enum Backend {
    Embedded(Arc<Mutex<Engine>>),
    Http(Client),
}

fn lock(e: &Arc<Mutex<Engine>>) -> std::sync::MutexGuard<'_, Engine> {
    // A poisoned index mutex means a previous call panicked mid-query; the
    // connection itself is still usable, so take it rather than cascade.
    e.lock().unwrap_or_else(|p| p.into_inner())
}

/// Message for surfaces the embedded engine does not implement in v0.2.
/// B11: never a vague empty result, always this sentence.
fn http_only(what: &str) -> LapisError {
    LapisError::Usage(format!(
        "{what} is not implemented by the embedded index in v0.2; it requires `lattice.mode = \"http\"` \
         (set it in ~/.config/lapis/config.toml or pass --lattice <url>)"
    ))
}

impl Backend {
    pub fn mode(&self) -> &'static str {
        match self {
            Backend::Embedded(_) => "embedded",
            Backend::Http(_) => "http",
        }
    }

    /// Where reads come from: the sqlite file, or the lattice URL.
    pub fn source(&self) -> String {
        match self {
            Backend::Embedded(e) => lock(e).db_path().display().to_string(),
            Backend::Http(c) => c.base().to_string(),
        }
    }

    pub async fn search(&self, p: &SearchParams) -> Result<SearchResult> {
        match self {
            Backend::Http(c) => c.search(p).await,
            Backend::Embedded(e) => {
                let mode = if p.embedder.as_deref() == Some("none") {
                    // BM25 only: do not invoke the stored embedder (no Ollama TCP).
                    lapis_lattice::Mode::Bm25
                } else {
                    match p.mode {
                        Mode::Bm25 => lapis_lattice::Mode::Bm25,
                        Mode::Vector => lapis_lattice::Mode::Vector,
                        Mode::Hybrid => lapis_lattice::Mode::Hybrid,
                    }
                };
                let q = lapis_lattice::SearchParams {
                    query: p.query.clone(),
                    limit: p.top_k,
                    offset: 0,
                    domain: p.domain.clone(),
                    per_doc: p.per_doc,
                    mode,
                };
                let t0 = std::time::Instant::now();
                let r = lock(e).search_with(&q).map_err(engine_err)?;
                let hits: Vec<Hit> = r
                    .hits
                    .into_iter()
                    .map(|h| Hit {
                        // recorded at index time, not re-derived per consumer
                        kind: match h.kind.as_str() {
                            lapis_lattice::HTML => notes::Kind::Html,
                            lapis_lattice::YAML => notes::Kind::Yaml,
                            lapis_lattice::MARKDOWN => notes::Kind::Markdown,
                            _ => notes::kind_of(&h.path),
                        },
                        title: h
                            .title
                            .filter(|t| !t.trim().is_empty())
                            .unwrap_or_else(|| crate::notes::stem_of(&h.path)),
                        heading: h.heading.filter(|s| !s.is_empty()),
                        snippet: h.snippet.filter(|s| !s.is_empty()),
                        score: Some(h.score),
                        rank: Some(h.rank),
                        domain: h.domain,
                        doc_type: h.doc_type,
                        tags: vec![],
                        chunk_id: None,
                        chunk_index: None,
                        path: h.path,
                    })
                    .collect();
                Ok(SearchResult {
                    query: p.query.trim().to_string(),
                    mode: p.mode,
                    modalities: r.modalities,
                    latency_ms: Some(t0.elapsed().as_secs_f64() * 1000.0),
                    latency: lattice::Latency::default(),
                    count: hits.len(),
                    hits,
                })
            }
        }
    }

    pub async fn documents(&self, p: &ListParams) -> Result<Vec<lattice::Document>> {
        match self {
            Backend::Http(c) => c.documents(p).await,
            Backend::Embedded(e) => {
                let q = lapis_lattice::ListParams {
                    domain: p.domain.clone(),
                    doc_type: p.doc_type.clone(),
                    status: p.status.clone(),
                    tag: p.tag.clone(),
                    prefix: p.prefix.clone(),
                    limit: p.limit,
                    offset: p.offset,
                };
                Ok(lock(e)
                    .documents(&q)
                    .map_err(engine_err)?
                    .into_iter()
                    .map(|d| lattice::Document {
                        path: d.path,
                        title: d.title,
                        domain: d.domain,
                        doc_type: d.doc_type,
                        status: d.status,
                        priority: d.priority,
                        tags: d.tags,
                        // the engine stores millisecond mtime; Document carries seconds
                        mtime: d.updated_at.map(|ms| ms as f64 / 1000.0),
                        hash: d.hash,
                    })
                    .collect())
            }
        }
    }

    pub async fn neighbors(&self, path: &str, direction: &str, resolved_only: bool) -> Result<Neighbors> {
        match self {
            Backend::Http(c) => c.neighbors(path, direction, resolved_only).await,
            Backend::Embedded(e) => {
                let rows = lock(e).neighbors(path, direction).map_err(engine_err)?;
                let neighbors = rows
                    .into_iter()
                    .filter(|n| !resolved_only || n.resolved)
                    .map(|n| lattice::Neighbor {
                        path: n.path,
                        dst_raw: Some(n.dst_raw),
                        alias: n.alias,
                        anchor: n.anchor,
                        resolved: n.resolved,
                        direction: n.dir,
                    })
                    .collect();
                Ok(Neighbors { path: path.to_string(), direction: direction.to_string(), hop: 1, neighbors })
            }
        }
    }

    /// Hop-2 ego graph. B11: embedded says so plainly instead of returning nothing.
    pub async fn ego(
        &self,
        path: &str,
        hops: u32,
        direction: &str,
        resolved_only: bool,
    ) -> Result<lattice::Ego> {
        match self {
            Backend::Http(c) => c.ego(path, hops, direction, resolved_only).await,
            Backend::Embedded(_) => Err(http_only("`neighbors --hop 2` (the hop-2 ego graph)")),
        }
    }

    /// B11: same rule as `ego`.
    pub async fn tree(
        &self,
        path: Option<&str>,
        query: Option<&str>,
        depth: u32,
        max_nodes: u32,
    ) -> Result<lattice::Tree> {
        match self {
            Backend::Http(c) => c.tree(path, query, depth, max_nodes).await,
            Backend::Embedded(_) => Err(http_only("`tree-retrieve`")),
        }
    }

    pub async fn analytics(&self, query: &str) -> Result<lattice::Analytics> {
        match self {
            Backend::Http(c) => c.analytics(query).await,
            Backend::Embedded(e) => {
                lattice::check_analytics_query(query)?;
                let t0 = std::time::Instant::now();
                let a = lock(e).analytics(query).map_err(engine_err)?;
                Ok(lattice::Analytics {
                    query: a.query,
                    columns: a.columns,
                    rows: a.rows.into_iter().map(|r| r.into_iter().map(Value::String).collect()).collect(),
                    count: a.count,
                    truncated: false,
                    latency_ms: Some(t0.elapsed().as_secs_f64() * 1000.0),
                    extra: serde_json::Map::new(),
                })
            }
        }
    }

    pub async fn health(&self) -> Result<Health> {
        match self {
            Backend::Embedded(e) => {
                let h = lock(e).health().map_err(engine_err)?;
                Ok(Health {
                    status: h.status,
                    documents_indexed: h.documents_indexed,
                    db_path: h.db_path,
                    embedder: h.embedder.to_string(),
                    embed_model: h.embed_model,
                    embed_dim: h.embed_dim,
                    graph: Graph {
                        built: h.graph.built,
                        edges: h.graph.edges,
                        dangling_links: h.graph.dangling_links,
                    },
                })
            }
            Backend::Http(c) => {
                let h = c.health().await?;
                Ok(Health {
                    status: h.status,
                    documents_indexed: h.documents_indexed,
                    db_path: h.db_path.unwrap_or_else(|| c.base().to_string()),
                    // the HTTP lattice owns its own embedder; report what it says
                    embedder: h
                        .extra
                        .get("embedder")
                        .and_then(Value::as_str)
                        .unwrap_or(if h.ollama_url.is_some() { "ollama" } else { "none" })
                        .to_string(),
                    embed_model: h.extra.get("embed_model").and_then(Value::as_str).map(str::to_string),
                    embed_dim: h.extra.get("embed_dim").and_then(Value::as_u64).map(|n| n as u32),
                    graph: Graph { built: true, edges: h.edges, dangling_links: h.dangling_links },
                })
            }
        }
    }

    /// Index one path after a write. Embedded does it in-process; HTTP kicks serve.py.
    pub async fn reindex(&self, rel: &str) -> Result<lattice::Reindex> {
        match self {
            Backend::Http(c) => c.reindex(rel).await,
            Backend::Embedded(e) => {
                let t0 = std::time::Instant::now();
                let r = lock(e).reindex_path(rel).map_err(engine_err)?;
                Ok(lattice::Reindex {
                    ok: true,
                    path: rel.to_string(),
                    changed: Some(r.documents > 0),
                    chunks: Some(r.chunks),
                    indexer_exit: Some(0),
                    elapsed_ms: Some(t0.elapsed().as_secs_f64() * 1000.0),
                    stdout_tail: vec![],
                    stderr_tail: vec![],
                })
            }
        }
    }
}

/// Engine errors carry the CLI exit contract: usage is 1, sqlite trouble is 2
/// (the index is down, the same class as the lattice being unreachable).
fn engine_err(e: lapis_lattice::Error) -> LapisError {
    match e {
        lapis_lattice::Error::Usage(m) => LapisError::Usage(m),
        lapis_lattice::Error::Io(io) => LapisError::from(io),
        lapis_lattice::Error::Sqlite(s) => LapisError::LatticeDown(format!("index: {s}")),
    }
}
