//! Shared operations behind both the CLI and the MCP server, so `--json` and
//! tool results are the same objects. Everything here returns data; printing
//! belongs to `main.rs`, envelopes to `mcp.rs`.

use serde::Serialize;

use std::path::Path;

use crate::backend::Backend;
use crate::error::{LapisError, Result};
use crate::http::{self, Client, Document, ListParams, Reindex, SearchResult};
use crate::{config, notes, overlay, tasks, vault, write};
use lapis_lattice::SearchParams;

pub struct Ctx {
    pub json: bool,
    pub vault: vault::Vault,
    pub cfg: config::Config,
    pub lattice_url: String,
    /// `--lattice <url>` on the command line means the operator wants HTTP.
    pub force_http: bool,
}

impl Ctx {
    pub fn client(&self) -> Result<Client> {
        Client::new(&self.lattice_url, self.cfg.lattice.timeout())
    }

    /// The read path. Embedded by default (D-V02-BACKEND); HTTP only when the
    /// operator asked for it in config or passed `--lattice`.
    pub fn backend(&self) -> Result<Backend> {
        if self.cfg.lattice.is_embedded() && !self.force_http {
            let engine = lapis_lattice::Engine::open(&self.vault.root).map_err(LapisError::from)?;
            Ok(Backend::Embedded(std::sync::Arc::new(std::sync::Mutex::new(engine))))
        } else {
            Ok(Backend::Http(self.client()?))
        }
    }
    pub fn inbox(&self) -> Result<String> {
        Ok(overlay::load(&self.vault.root)?.0.buckets.inbox)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub vault_root: String,
    pub vault_source: &'static str,
    pub overlay: OverlayInfo,
    pub lattice: LatticeInfo,
}

#[derive(Serialize)]
pub struct OverlayInfo {
    pub source: overlay::OverlaySource,
    pub path: String,
    #[serde(flatten)]
    pub overlay: overlay::Overlay,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatticeInfo {
    /// `embedded` or `http` — which read path answered.
    pub mode: &'static str,
    /// sqlite file when embedded, base URL when http.
    pub url: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<crate::backend::Health>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Returns the report plus the health error (if any) so the CLI can exit 2
/// after printing, and MCP can report `reachable: false` without failing.
pub async fn vault_info(ctx: &Ctx) -> Result<(VaultInfo, Option<crate::error::LapisError>)> {
    let (ov, ov_src) = overlay::load(&ctx.vault.root)?;
    let backend = ctx.backend()?;
    let source = backend.source();
    let mode = backend.mode();
    let (lattice, err) = match backend.health().await {
        Ok(h) => (LatticeInfo { mode, url: source, reachable: true, health: Some(h), error: None }, None),
        Err(e) => (
            LatticeInfo { mode, url: source, reachable: false, health: None, error: Some(e.to_string()) },
            Some(e),
        ),
    };
    Ok((
        VaultInfo {
            vault_root: ctx.vault.root.display().to_string(),
            vault_source: ctx.vault.source,
            overlay: OverlayInfo { source: ov_src, path: overlay::OVERLAY_REL.into(), overlay: ov },
            lattice,
        },
        err,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRow {
    pub path: String,
    pub kind: notes::Kind,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    /// Content fingerprint (embedded backend only), for `--if-hash` planning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

impl From<Document> for ListRow {
    fn from(d: Document) -> Self {
        let kind =
            if d.doc_type.as_deref() == Some("tome") { notes::Kind::Pdf } else { notes::kind_of(&d.path) };
        let title = d.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| d.path.clone());
        ListRow {
            kind,
            title,
            domain: d.domain,
            doc_type: d.doc_type,
            status: d.status,
            priority: d.priority,
            tags: d.tags,
            updated_at: d.mtime.map(|m| (m * 1000.0) as u64),
            hash: d.hash,
            path: d.path,
        }
    }
}

/// `prefix` accepts a folder (`notes/`) with the same escape rules as `read`.
pub fn clean_prefix(prefix: Option<&str>) -> Result<Option<String>> {
    Ok(match prefix {
        Some(p) => {
            let trailing = p.ends_with('/');
            let clean = notes::clean_rel(p)?;
            Some(if trailing { format!("{clean}/") } else { clean })
        }
        None => None,
    })
}

pub async fn list(ctx: &Ctx, mut params: ListParams) -> Result<Vec<ListRow>> {
    params.prefix = clean_prefix(params.prefix.as_deref())?;
    Ok(ctx.backend()?.documents(&params).await?.into_iter().map(ListRow::from).collect())
}

/// Search result cap: offset + limit must stay ≤ this (CLI, MCP, JSON meta).
pub const SEARCH_CAP: u32 = 50;

/// What a search surface asks of [`search`]. Offset/limit live here, not in main/mcp.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub limit: u32,
    pub offset: u32,
    pub domain: Option<String>,
    pub mode: lapis_lattice::Mode,
    pub per_doc: bool,
    pub mmr: bool,
    pub include_archives: bool,
    pub embedder: Option<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            limit: 10,
            offset: 0,
            domain: None,
            mode: lapis_lattice::Mode::Hybrid,
            per_doc: true,
            mmr: false,
            include_archives: false,
            embedder: None,
        }
    }
}

pub struct SearchPage {
    pub result: SearchResult,
    pub truncated: bool,
    pub next: Option<u32>,
    pub limit: u32,
    pub offset: u32,
}

fn prepare_search(q: SearchQuery) -> Result<(SearchParams, u32, u32)> {
    let limit = q.limit.max(1);
    let offset = q.offset;
    let requested = offset.saturating_add(limit);
    if requested > SEARCH_CAP {
        return Err(LapisError::Usage(format!("offset + limit must be ≤ {SEARCH_CAP} (got {requested})")));
    }
    if q.embedder.as_deref() == Some("none") && q.mode == lapis_lattice::Mode::Vector {
        return Err(LapisError::Usage(
            "vector mode needs an embedder; got --embedder none. Use --mode bm25|hybrid.".into(),
        ));
    }
    Ok((
        SearchParams {
            query: q.query,
            limit: requested,
            offset: 0,
            domain: q.domain,
            mode: q.mode,
            per_doc: q.per_doc,
            mmr: q.mmr,
            include_archives: q.include_archives,
            embedder: q.embedder,
        },
        limit,
        offset,
    ))
}

pub async fn search(ctx: &Ctx, q: SearchQuery) -> Result<SearchPage> {
    let (params, limit, offset) = prepare_search(q)?;
    let mut result = ctx.backend()?.search(&params).await?;
    let total = result.hits.len();
    result.hits = result.hits.into_iter().skip(offset as usize).collect();
    result.count = result.hits.len();
    let requested = offset + limit;
    let truncated = total as u32 >= requested && requested < SEARCH_CAP;
    Ok(SearchPage { result, truncated, next: if truncated { Some(requested) } else { None }, limit, offset })
}

pub enum NeighborView {
    Direct(http::Neighbors),
    Ego(http::Ego),
}

fn note_rel(path: &str) -> Result<String> {
    let rel = notes::clean_rel(path)?;
    Ok(if Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel })
}

pub async fn neighbors(
    ctx: &Ctx,
    path: &str,
    direction: Option<&str>,
    dangling: bool,
    hop: u32,
) -> Result<NeighborView> {
    if hop != 1 && hop != 2 {
        return Err(LapisError::Usage(format!("hop must be 1 or 2, got {hop}")));
    }
    let rel = note_rel(path)?;
    let dir = direction.unwrap_or_else(|| ctx.cfg.agent.direction());
    let resolved_only = !dangling;
    if hop == 2 {
        return Ok(NeighborView::Ego(ctx.backend()?.ego(&rel, 2, dir, resolved_only).await?));
    }
    Ok(NeighborView::Direct(ctx.backend()?.neighbors(&rel, dir, resolved_only).await?))
}

pub async fn reindex(ctx: &Ctx, path: &str) -> Result<Reindex> {
    let (rel, _) = notes::resolve(&ctx.vault.root, path)?;
    ctx.backend()?.reindex(&rel).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteReport {
    #[serde(flatten)]
    pub written: write::Written,
    /// `null` when the kick was skipped or failed; see `reindexError`.
    pub reindex: Option<Reindex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_error: Option<String>,
}

/// Index the path after a successful write. Embedded does it in-process, so a
/// note is searchable the moment it is written; HTTP kicks serve.py. The file
/// is already the source of truth on disk either way, so a failed index is
/// reported, not fatal.
pub async fn kick(ctx: &Ctx, written: write::Written, no_reindex: bool) -> Result<WriteReport> {
    let (reindex, reindex_error) = if no_reindex || written.dry_run {
        (None, None)
    } else {
        match ctx.backend()?.reindex(&written.path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok(WriteReport { written, reindex, reindex_error })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleReport {
    pub task: tasks::Task,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    pub reindex: Option<Reindex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_error: Option<String>,
}

pub async fn toggle_task_with(
    ctx: &Ctx,
    id: &str,
    no_reindex: bool,
    guard: &write::Guard,
) -> Result<ToggleReport> {
    let task = tasks::toggle_with(&ctx.vault.root, id, guard)?;
    let (reindex, reindex_error) = if no_reindex || guard.dry_run {
        (None, None)
    } else {
        match ctx.backend()?.reindex(&task.source_path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok(ToggleReport { task, dry_run: guard.dry_run, reindex, reindex_error })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyReport {
    #[serde(flatten)]
    pub daily: write::Periodic,
    pub reindex: Option<Reindex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_error: Option<String>,
}

pub async fn periodic(
    ctx: &Ctx,
    period: write::Period,
    date: Option<&str>,
    no_reindex: bool,
) -> Result<DailyReport> {
    let d = write::periodic(&ctx.vault.root, period, date, ctx.cfg.operator.name.clone())?;
    let (reindex, reindex_error) = if no_reindex || !d.created {
        (None, None)
    } else {
        match ctx.backend()?.reindex(&d.path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok(DailyReport { daily: d, reindex, reindex_error })
}

/// Trash keeps the file; the lattice row goes stale until reconcile (the
/// kick route only indexes existing walked paths, and trash is unwalked).
pub fn trash(ctx: &Ctx, rel: &str) -> Result<write::Trashed> {
    let bucket = overlay::load(&ctx.vault.root)?.0.buckets.trash;
    write::trash(&ctx.vault.root, rel, &bucket)
}

pub fn trash_bucket(ctx: &Ctx) -> String {
    overlay::load(&ctx.vault.root).map(|(o, _)| o.buckets.trash).unwrap_or_else(|_| ".lapis/trash".into())
}

/// Restore, then kick the index for the restored path.
pub async fn restore(
    ctx: &Ctx,
    trashed_rel: &str,
    no_reindex: bool,
) -> Result<(write::Trashed, Option<Reindex>, Option<String>)> {
    let bucket = trash_bucket(ctx);
    let t = write::restore(&ctx.vault.root, trashed_rel, &bucket)?;
    let (reindex, err) = if no_reindex {
        (None, None)
    } else {
        match ctx.backend()?.reindex(&t.path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok((t, reindex, err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_offset_plus_limit_cap_lives_here() {
        let q = SearchQuery { query: "x".into(), limit: 30, offset: 30, ..Default::default() };
        match prepare_search(q) {
            Err(LapisError::Usage(m)) => assert!(m.contains("≤ 50"), "{m}"),
            other => panic!("expected usage, got {other:?}"),
        }
        let (p, limit, offset) = prepare_search(SearchQuery {
            query: "lattice".into(),
            limit: 3,
            offset: 6,
            mode: lapis_lattice::Mode::Bm25,
            per_doc: false,
            ..Default::default()
        })
        .unwrap();
        assert_eq!((p.limit, p.mode, limit, offset), (9, lapis_lattice::Mode::Bm25, 3, 6));
        assert!(!p.per_doc);
        let (p, limit, offset) =
            prepare_search(SearchQuery { query: "lattice".into(), ..Default::default() }).unwrap();
        assert!(p.per_doc);
        assert_eq!((p.limit, limit, offset), (10, 10, 0));
    }

    #[test]
    fn vector_plus_embedder_none_is_usage() {
        let q = SearchQuery {
            query: "x".into(),
            mode: lapis_lattice::Mode::Vector,
            embedder: Some("none".into()),
            ..Default::default()
        };
        match prepare_search(q) {
            Err(LapisError::Usage(m)) => assert!(m.contains("embedder"), "{m}"),
            other => panic!("expected usage, got {other:?}"),
        }
    }
}
