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
    let (reindex, reindex_error) = if no_reindex {
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
    pub reindex: Option<Reindex>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_error: Option<String>,
}

pub async fn toggle_task(ctx: &Ctx, id: &str, no_reindex: bool) -> Result<ToggleReport> {
    let task = tasks::toggle(&ctx.vault.root, id)?;
    let (reindex, reindex_error) = if no_reindex {
        (None, None)
    } else {
        match ctx.client()?.reindex(&task.source_path).await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };
    Ok(ToggleReport { task, reindex, reindex_error })
}
