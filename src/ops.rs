//! Shared operations behind both the CLI and the MCP server, so `--json` and
//! tool results are the same objects. Everything here returns data; printing
//! belongs to `main.rs`, envelopes to `mcp.rs`.

use serde::Serialize;

use crate::error::Result;
use crate::lattice::{Client, Document, ListParams, Reindex};
use crate::{config, lattice, notes, overlay, tasks, vault, write};

pub struct Ctx {
    pub json: bool,
    pub vault: vault::Vault,
    pub cfg: config::Config,
    pub lattice_url: String,
}

impl Ctx {
    pub fn client(&self) -> Result<Client> {
        Client::new(&self.lattice_url, self.cfg.lattice.timeout())
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
    pub url: String,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<lattice::Health>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Returns the report plus the health error (if any) so the CLI can exit 2
/// after printing, and MCP can report `reachable: false` without failing.
pub async fn vault_info(ctx: &Ctx) -> Result<(VaultInfo, Option<crate::error::LapisError>)> {
    let (ov, ov_src) = overlay::load(&ctx.vault.root)?;
    let client = ctx.client()?;
    let (lattice, err) = match client.health().await {
        Ok(h) => {
            (LatticeInfo { url: client.base().into(), reachable: true, health: Some(h), error: None }, None)
        }
        Err(e) => (
            LatticeInfo {
                url: client.base().into(),
                reachable: false,
                health: None,
                error: Some(e.to_string()),
            },
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
            path: d.path,
        }
    }
}

/// `prefix` accepts a folder (`foundry/lapis/`) with the same escape rules as `read`.
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
    Ok(ctx.client()?.documents(&params).await?.into_iter().map(ListRow::from).collect())
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

/// Kick the index after a successful write. The file is already the source
/// of truth on disk, so a failed kick is reported, not fatal; nightly
/// reconcile catches it.
pub async fn kick(ctx: &Ctx, written: write::Written, no_reindex: bool) -> Result<WriteReport> {
    let (reindex, reindex_error) = if no_reindex || written.dry_run {
        (None, None)
    } else {
        match ctx.client()?.reindex(&written.path).await {
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
        match ctx.client()?.reindex(&task.source_path).await {
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
        match ctx.client()?.reindex(&d.path).await {
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
        match ctx.client()?.reindex(&t.path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok((t, reindex, err))
}
