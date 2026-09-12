//! One retrieval surface, two implementations.
//!
//! `Embedded` is the default (SPEC-V02 D-V02-BACKEND): an in-process
//! `lapis-lattice` engine over `<vault>/.lapis/lattice.sqlite`, so a stranger
//! gets search with no daemon and no listening socket. `Http` is opt-in via
//! `lattice.mode = "http"` and keeps the operator's existing lattice working.
//!
//! Both arms return the same CLI wire types, so `main` and `mcp` never fork.

use std::path::Path;
use std::sync::{Arc, Mutex};

use lapis_lattice::{Engine, Hit, SearchParams};
use serde::Serialize;
use serde_json::Value;

use crate::error::{LapisError, Result};
use crate::http::{self, Client, Document, ListParams, Neighbors, SearchResult};
use crate::notes::{self, Kind};

/// Window fetched before identifier boost, matching `ops::SEARCH_CAP`.
const IDENTIFIER_WINDOW: u32 = 50;
const IDENTIFIER_MODALITY: &str = "identifier";

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

fn query_tokens(q: &str) -> Vec<String> {
    let mut t: Vec<String> = q
        .split_whitespace()
        .flat_map(|w| w.split('.'))
        .map(|w| w.chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').collect::<String>())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect();
    if t.iter().any(|s| s.len() >= 3) {
        t.retain(|s| s.len() >= 3);
    }
    t
}

fn identifier_haystack(path: &str, title: &str) -> String {
    let name = Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or(path);
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path);
    format!("{path} {name} {stem} {title}").to_lowercase()
}

fn is_identifier(path: &str, title: &str, tokens: &[String], q_fold: &str) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let hay = identifier_haystack(path, title);
    (!q_fold.is_empty() && hay.contains(q_fold)) || tokens.iter().all(|t| hay.contains(t))
}

fn identifier_strength(path: &str, title: &str, q_fold: &str) -> u8 {
    let path_l = path.to_lowercase();
    let title_l = title.to_lowercase();
    let name = Path::new(path).file_name().and_then(|s| s.to_str()).unwrap_or(path).to_lowercase();
    let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path).to_lowercase();
    let q_stem = q_fold.trim_end_matches(".md");
    if stem == q_fold || stem == q_stem || name == q_fold {
        3
    } else if title_l == q_fold || title_l == q_stem || stem.contains(q_stem) || name.contains(q_stem) {
        2
    } else if path_l.contains(q_stem) || title_l.contains(q_stem) {
        1
    } else {
        0
    }
}

fn hit_from_doc(d: Document) -> Hit {
    let path = d.path;
    let title = d.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
        Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or(&path).to_string()
    });
    Hit {
        kind: match notes::kind_of(&path) {
            Kind::Html => lapis_lattice::HTML.into(),
            Kind::Yaml => lapis_lattice::YAML.into(),
            Kind::Markdown => lapis_lattice::MARKDOWN.into(),
            Kind::Pdf => "pdf".into(),
            Kind::Source => "source".into(),
        },
        title,
        heading: None,
        snippet: None,
        rank: 0,
        score: 1.0,
        domain: d.domain,
        doc_type: d.doc_type,
        tags: d.tags,
        chunk_id: None,
        chunk_index: None,
        path,
    }
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
        let want = p.limit.clamp(1, IDENTIFIER_WINDOW);
        let mut fetch = p.clone();
        fetch.limit = IDENTIFIER_WINDOW;
        fetch.offset = 0;
        let mut r = match self {
            Backend::Http(c) => c.search(&fetch).await?,
            Backend::Embedded(e) => {
                let t0 = std::time::Instant::now();
                let eng = lock(e).search_with(&fetch).map_err(LapisError::from)?;
                SearchResult::from_engine(p, eng, t0.elapsed().as_secs_f64() * 1000.0)
            }
        };
        self.apply_identifier_boost(p, &mut r).await;
        if r.hits.len() > want as usize {
            r.hits.truncate(want as usize);
        }
        for (i, h) in r.hits.iter_mut().enumerate() {
            h.rank = i as u32 + 1;
        }
        r.count = r.hits.len();
        Ok(r)
    }

    /// Reorder (and if needed inject) hits whose path, basename, or title
    /// contains the query tokens. Shared by HTTP and embedded.
    async fn apply_identifier_boost(&self, p: &SearchParams, r: &mut SearchResult) {
        let tokens = query_tokens(&p.query);
        if tokens.is_empty() {
            return;
        }
        let q_fold = p.query.trim().to_lowercase();
        let mut idents = Vec::new();
        let mut rest = Vec::new();
        for h in r.hits.drain(..) {
            if is_identifier(&h.path, &h.title, &tokens, &q_fold) {
                idents.push(h);
            } else {
                rest.push(h);
            }
        }
        if idents.is_empty()
            && let Ok(extra) = self.inject_identifier_hits(p, &tokens, &q_fold, &rest).await
        {
            idents = extra;
        }
        if idents.is_empty() {
            r.hits = rest;
            return;
        }
        idents.sort_by(|a, b| {
            identifier_strength(&b.path, &b.title, &q_fold)
                .cmp(&identifier_strength(&a.path, &a.title, &q_fold))
        });
        if !r.modalities.iter().any(|m| m == IDENTIFIER_MODALITY) {
            r.modalities.push(IDENTIFIER_MODALITY.into());
        }
        let seen: std::collections::HashSet<String> = idents.iter().map(|h| h.path.clone()).collect();
        rest.retain(|h| !seen.contains(&h.path));
        r.hits = idents;
        r.hits.append(&mut rest);
    }

    async fn inject_identifier_hits(
        &self,
        p: &SearchParams,
        tokens: &[String],
        q_fold: &str,
        rest: &[Hit],
    ) -> Result<Vec<Hit>> {
        let existing: std::collections::HashSet<&str> = rest.iter().map(|h| h.path.as_str()).collect();
        let mut out = Vec::new();
        let mut offset = 0u32;
        for _ in 0..8 {
            let docs = self
                .documents(&ListParams {
                    domain: p.domain.clone(),
                    limit: 1000,
                    offset,
                    ..Default::default()
                })
                .await?;
            let n = docs.len() as u32;
            for d in docs {
                if existing.contains(d.path.as_str()) {
                    continue;
                }
                let title = d.title.clone().unwrap_or_default();
                if is_identifier(&d.path, &title, tokens, q_fold) {
                    out.push(hit_from_doc(d));
                }
            }
            if n < 1000 {
                break;
            }
            offset += 1000;
        }
        Ok(out)
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

    /// Explicit embedded full-vault rebuild. HTTP retains its own index lifecycle.
    pub async fn reindex_all(&self) -> Result<lapis_lattice::IndexReport> {
        match self {
            Backend::Embedded(engine) => lock(engine).reindex().map_err(LapisError::from),
            Backend::Http(_) => {
                Err(LapisError::Usage("Full indexing is managed by the configured HTTP service".into()))
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

    #[test]
    fn identifier_match_is_basename_not_body() {
        let tokens = super::query_tokens("GOAL-struct");
        assert!(super::is_identifier("pack/GOAL-struct.md", "GOAL-struct", &tokens, "goal-struct"));
        assert!(!super::is_identifier("notes/Skills-Paradigm.md", "Skills Paradigm", &tokens, "goal-struct"));
        assert_eq!(super::identifier_strength("pack/GOAL-struct.md", "GOAL-struct", "goal-struct"), 3);
    }

    /// Letter-bag embedder so hybrid actually runs a vector arm. Content-dependent
    /// so a long decoy can outrank a short identifier note before the boost.
    struct BagEmbedder {
        dim: usize,
    }

    impl lapis_lattice::Embedder for BagEmbedder {
        fn model(&self) -> &str {
            "bag-v1"
        }
        fn dim(&self) -> usize {
            self.dim
        }
        fn provider(&self) -> &'static str {
            "onnx"
        }
        fn embed_documents(&self, texts: &[String]) -> lapis_lattice::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|t| bag(t, self.dim)).collect())
        }
        fn embed_query(&self, text: &str) -> lapis_lattice::Result<Vec<f32>> {
            Ok(bag(text, self.dim))
        }
    }

    fn bag(text: &str, dim: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        for c in text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()) {
            v[(c as usize - b'a' as usize) % dim] += 1.0;
        }
        v
    }

    /// G2c: hybrid (BM25+vector) still ranks the identifier note first after
    /// `Backend::search`. Empty hits while indexed is a fail.
    #[tokio::test]
    async fn identifier_boost_beats_hybrid_decoy() {
        let d = std::env::temp_dir().join(format!(
            "lapis-g2c-id-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(d.join("pack")).unwrap();
        std::fs::create_dir_all(d.join("notes")).unwrap();
        std::fs::write(
            d.join("pack/GOAL-struct.md"),
            "---\nname: GOAL-struct\ntitle: GOAL-struct\n---\n# Paste\n\n\
             Authorized loop text. Ranking uses path and HAL name, not this body.\n",
        )
        .unwrap();
        let decoy = format!(
            "---\nname: Skills Paradigm\n---\n# Skills Paradigm\n\n{}\n",
            "structure skills paradigm hub vibe lattice ranking. ".repeat(80)
        );
        std::fs::write(d.join("notes/Skills-Paradigm.md"), decoy).unwrap();
        let mut e =
            lapis_lattice::Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { dim: 8 }))).unwrap();
        e.reindex().unwrap();
        let b = Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(e)));
        let r = b
            .search(&SearchParams {
                query: "GOAL-struct".into(),
                limit: 10,
                mode: lapis_lattice::Mode::Hybrid,
                per_doc: true,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!r.hits.is_empty(), "indexed GOAL-struct.md must not silent-zero");
        assert_eq!(
            r.hits[0].path,
            "pack/GOAL-struct.md",
            "identifier must rank #1 under hybrid, got {:?}",
            r.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
        );
        assert_eq!(r.hits[0].rank, 1);
        assert!(
            r.modalities.iter().any(|m| m == "identifier"),
            "boost must name itself, got {:?}",
            r.modalities
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
