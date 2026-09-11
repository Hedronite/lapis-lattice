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
use crate::http::{self, Client, ListParams, Neighbors, SearchResult};
use lapis_lattice::SearchParams;

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

/// B11: never a vague empty result. `kind` is `http_only`, not a Usage paragraph.
fn http_only(op: &'static str) -> LapisError {
    LapisError::HttpOnly { op }
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
                let t0 = std::time::Instant::now();
                let r = lock(e).search_with(p).map_err(LapisError::from)?;
                Ok(SearchResult::from_engine(p, r, t0.elapsed().as_secs_f64() * 1000.0))
            }
        }
    }

    pub async fn documents(&self, p: &ListParams) -> Result<Vec<http::Document>> {
        match self {
            Backend::Http(c) => c.documents(p).await,
            Backend::Embedded(e) => Ok(lock(e)
                .documents(&p.into())
                .map_err(LapisError::from)?
                .into_iter()
                .map(Into::into)
                .collect()),
        }
    }

    pub async fn neighbors(&self, path: &str, direction: &str, resolved_only: bool) -> Result<Neighbors> {
        match self {
            Backend::Http(c) => c.neighbors(path, direction, resolved_only).await,
            Backend::Embedded(e) => {
                let neighbors = lock(e)
                    .neighbors(path, direction)
                    .map_err(LapisError::from)?
                    .into_iter()
                    .filter(|n| !resolved_only || n.resolved)
                    .map(Into::into)
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
    ) -> Result<http::Ego> {
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
    ) -> Result<http::Tree> {
        match self {
            Backend::Http(c) => c.tree(path, query, depth, max_nodes).await,
            Backend::Embedded(_) => Err(http_only("`tree-retrieve`")),
        }
    }

    pub async fn analytics(&self, query: &str) -> Result<http::Analytics> {
        match self {
            Backend::Http(c) => c.analytics(query).await,
            Backend::Embedded(e) => {
                http::check_analytics_query(query)?;
                let t0 = std::time::Instant::now();
                let a = lock(e).analytics(query).map_err(LapisError::from)?;
                Ok(http::Analytics {
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
                let h = lock(e).health().map_err(LapisError::from)?;
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
    pub async fn reindex(&self, rel: &str) -> Result<http::Reindex> {
        match self {
            Backend::Http(c) => c.reindex(rel).await,
            Backend::Embedded(e) => {
                let t0 = std::time::Instant::now();
                let r = lock(e).reindex_path(rel).map_err(LapisError::from)?;
                Ok(http::Reindex {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// B11, and G2f in v0.3: the desktop may walk the whole index, but the CLI
    /// must not quietly grow a hop-2 answer on the embedded backend. It names
    /// the setting that would give one, and it fails with the usage exit code
    /// rather than returning an empty graph that looks like "no neighbours".
    #[tokio::test]
    async fn embedded_hop2_and_tree_still_require_http_mode() {
        let d = std::env::temp_dir().join(format!(
            "lapis-b11-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Welcome.md"), "# Welcome\n\nSee [[Alpha]].\n").unwrap();
        std::fs::write(d.join("Alpha.md"), "# Alpha\n\nBack to [[Welcome]].\n").unwrap();
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        let b = Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(e)));
        assert_eq!(b.mode(), "embedded");

        // hop-1 is answered locally, so this is not "the backend does nothing".
        let hop1 = b.neighbors("Welcome.md", "both", false).await.unwrap();
        assert_eq!(hop1.hop, 1);
        assert!(!hop1.neighbors.is_empty());

        for err in [
            b.ego("Welcome.md", 2, "both", false).await.err(),
            b.tree(Some("Welcome.md"), None, 2, 50).await.err(),
        ] {
            match err {
                Some(e @ LapisError::HttpOnly { .. }) => {
                    assert_eq!(e.kind(), "http_only");
                    assert_eq!(e.exit_code(), 1);
                    let m = e.message();
                    assert!(m.contains("lattice.mode"), "names the setting: {m}");
                    assert!(m.contains("http"), "names the mode: {m}");
                }
                other => panic!("expected HttpOnly naming lattice.mode, got {other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// G0b: the TUI palette calls this same `Backend::search`. Embedded mode
    /// with `--embedder none` must return hits from the vault index with no
    /// HTTP listener. Empty-because-:8080-is-down is a failure, not a miss.
    #[tokio::test]
    async fn embedded_search_answers_without_http() {
        let d = std::env::temp_dir().join(format!(
            "lapis-g0b-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Welcome.md"), "# Welcome\n\nThe quokka is a small macropod.\n").unwrap();
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        drop(e);

        let ctx = crate::ops::Ctx {
            json: false,
            vault: crate::vault::Vault { root: d.clone(), source: "test" },
            cfg: crate::config::Config::default(),
            lattice_url: crate::config::DEFAULT_LATTICE_URL.into(),
            force_http: false,
        };
        let b = ctx.backend().expect("embedded backend opens without a lattice listener");
        assert_eq!(b.mode(), "embedded");
        assert!(b.health().await.is_ok(), "health must not depend on HTTP :8080 in embedded mode");

        let r = b
            .search(&SearchParams {
                query: "quokka".into(),
                limit: 10,
                mode: lapis_lattice::Mode::Bm25,
                per_doc: true,
                embedder: Some("none".into()),
                ..Default::default()
            })
            .await
            .expect("embedded search must be an error or hits, never a silent HTTP miss");
        assert!(
            r.hits.iter().any(|h| h.path == "Welcome.md"),
            "expected Welcome.md in hits, got {:?}",
            r.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// G2c: a tagged note's hits carry tags and chunk_id from the index.
    #[tokio::test]
    async fn embedded_search_carries_tags_and_chunk_id() {
        let d = std::env::temp_dir().join(format!(
            "lapis-g2c-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("Tagged.md"),
            "---\nname: Tagged\ntags: [intro, lattice]\n---\n# Tagged\n\nQuokka tags live here.\n",
        )
        .unwrap();
        let mut e = lapis_lattice::Engine::open(&d).unwrap();
        e.reindex().unwrap();
        let b = Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(e)));
        let r = b
            .search(&SearchParams {
                query: "quokka".into(),
                limit: 10,
                mode: lapis_lattice::Mode::Bm25,
                embedder: Some("none".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        let h = r.hits.iter().find(|h| h.path == "Tagged.md").expect("Tagged.md hit");
        assert!(h.tags.iter().any(|t| t == "intro"), "tags from the index, got {:?}", h.tags);
        assert!(h.chunk_id.is_some(), "chunk_id from the index");
        let _ = std::fs::remove_dir_all(&d);
    }
}
