//! `lapis mcp`: MCP server over stdio (rmcp). Tool results are JSON objects
//! as text content, the same shapes `--json` prints (SPEC.md § MCP).

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::error::LapisError;
use crate::lattice::{ListParams, Mode, SearchParams};
use crate::ops::{self, Ctx};
use crate::{notes, tasks, write};

pub const INSTRUCTIONS: &str = "\
Lapis: an Atrium notes vault with Lapis Lattice retrieval.
1. Call vault_info once per session; it reports the vault root and lattice health.
2. Trust `path` values returned by other tools. They are vault-relative; never prefix inbox/ yourself.
3. Prefer `search` (hybrid lattice retrieval) over constructing greps or reading many files.
4. Prefer toggle_task / append_to_note over rewriting HAL frontmatter by hand.
5. On create_note, pass a template or accept the taxonomy defaults; never emit empty frontmatter.
6. Present notes as [title](path). There is no zennotes:// scheme.
7. If the lattice is down (vault_info.lattice.reachable=false), say so; do not walk the filesystem for search.
8. `search` collapses to one hit per document unless per_doc=false. `neighbors` returns both directions; dangling rows have path=null and dst_raw.
9. `list_tasks` without `path` returns counts (n, byStatus, byFolder); pass `path` (or full=true) for rows.";

/// `#[tool_handler]` in rmcp 3.x routes through `Self::tool_router()`, so the
/// server holds only its context.
#[derive(Clone)]
pub struct LapisServer {
    ctx: Arc<Ctx>,
}

fn ok<T: serde::Serialize>(v: &T) -> std::result::Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string_pretty(v).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

fn fail(e: LapisError) -> ErrorData {
    match e {
        LapisError::Usage(_) | LapisError::Path(_) => ErrorData::invalid_params(e.to_string(), None),
        LapisError::LatticeDown(_) | LapisError::Internal(_) => {
            ErrorData::internal_error(e.to_string(), None)
        }
    }
}

#[derive(Deserialize, JsonSchema)]
pub struct PathArg {
    /// Vault-relative note path, e.g. `foundry/lapis/SPEC.md`.
    pub path: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArg {
    /// Query text.
    pub query: String,
    /// Result count (1..50), default 10.
    pub limit: Option<u32>,
    /// Filter by HAL domain.
    pub domain: Option<String>,
    /// `hybrid` (default), `bm25`, or `vector`.
    pub mode: Option<String>,
    /// Collapse to the best chunk per document. Default true.
    pub per_doc: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListArg {
    /// Vault-relative folder prefix, e.g. `foundry/lapis/`.
    pub prefix: Option<String>,
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub tag: Option<String>,
    /// Max rows (default 50, max 1000).
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct NeighborsArg {
    pub path: String,
    /// `both` (default), `out`, or `in`.
    pub direction: Option<String>,
    /// Include dangling links too (rows with `path: null` and `dst_raw`).
    pub dangling: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct CreateArg {
    /// Title; becomes HAL `name` and the slugged filename.
    pub title: String,
    /// Folder (`foundry/lapis/`) or file path. Default: the inbox bucket.
    pub path: Option<String>,
    /// Template name from `.lapis/templates/`.
    pub template: Option<String>,
    /// HAL type/doc_type (default: taxonomy).
    pub doc_type: Option<String>,
    pub domain: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Body markdown (default `# title`).
    pub body: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppendArg {
    pub path: String,
    /// Markdown to append.
    pub text: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListTasksArg {
    /// `open`, `done`, `in-progress`, `cancelled`, `forwarded`, `waiting`.
    pub status: Option<String>,
    /// `today`, `overdue`, or `YYYY-MM-DD`.
    pub due: Option<String>,
    pub tag: Option<String>,
    /// Restrict the scan to this folder or file. Without it the result is a summary.
    pub path: Option<String>,
    /// Return every row even when unscoped (default false: counts only).
    pub full: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ToggleArg {
    /// Task id `path#index` or `path#task`, from list_tasks.
    pub id: String,
}

#[tool_router]
impl LapisServer {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    #[tool(description = "Vault root, overlay buckets, and lattice health. Call once per session.")]
    async fn vault_info(&self) -> std::result::Result<CallToolResult, ErrorData> {
        let (info, _) = ops::vault_info(&self.ctx).await.map_err(fail)?;
        ok(&info)
    }

    #[tool(description = "Lattice /healthz.")]
    async fn health(&self) -> std::result::Result<CallToolResult, ErrorData> {
        ok(&self.ctx.client().map_err(fail)?.health().await.map_err(fail)?)
    }

    #[tool(description = "Read one note: body plus parsed HAL frontmatter (hal, halValid).")]
    async fn read_note(
        &self,
        Parameters(a): Parameters<PathArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        ok(&notes::read(&self.ctx.vault.root, &a.path).map_err(fail)?)
    }

    #[tool(
        description = "Hybrid lattice search (BM25 + vectors + title). Returns hits with vault-relative paths."
    )]
    async fn search(
        &self,
        Parameters(a): Parameters<SearchArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let p = search_params(a)?;
        ok(&self.ctx.client().map_err(fail)?.search(&p).await.map_err(fail)?)
    }

    #[tool(
        description = "List notes from the lattice documents table (metadata only). Never walks the vault."
    )]
    async fn list_notes(
        &self,
        Parameters(a): Parameters<ListArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let p = ListParams {
            domain: a.domain,
            doc_type: a.doc_type,
            status: a.status,
            tag: a.tag,
            prefix: a.prefix,
            limit: a.limit.unwrap_or(50),
            offset: a.offset.unwrap_or(0),
            include_archives: false,
        };
        ok(&ops::list(&self.ctx, p).await.map_err(fail)?)
    }

    #[tool(description = "Hop-1 wikilink neighbors of a note from the lattice edges table.")]
    async fn neighbors(
        &self,
        Parameters(a): Parameters<NeighborsArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let rel = notes::clean_rel(&a.path).map_err(fail)?;
        let rel = if std::path::Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel };
        let dir = a.direction.unwrap_or_else(|| "both".into());
        ok(&self
            .ctx
            .client()
            .map_err(fail)?
            .neighbors(&rel, &dir, !a.dangling.unwrap_or(false))
            .await
            .map_err(fail)?)
    }

    #[tool(
        description = "Create a note with a HAL create-set, then kick the lattice index. Returns the path."
    )]
    async fn create_note(
        &self,
        Parameters(a): Parameters<CreateArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let opts = write::CreateOpts {
            title: a.title,
            path: a.path,
            template: a.template,
            doc_type: a.doc_type,
            domain: a.domain,
            tags: a.tags.unwrap_or_default(),
            body: a.body,
            operator: self.ctx.cfg.operator.name.clone(),
            inbox: self.ctx.inbox().map_err(fail)?,
            director: None,
            template_date: None,
        };
        let w = write::create(&self.ctx.vault.root, &opts).map_err(fail)?;
        ok(&ops::kick(&self.ctx, w, false).await.map_err(fail)?)
    }

    #[tool(
        description = "Append markdown to a note (bumps `updated:`, keeps all other frontmatter), then kick the index."
    )]
    async fn append_to_note(
        &self,
        Parameters(a): Parameters<AppendArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let w = write::append(&self.ctx.vault.root, &a.path, &a.text).map_err(fail)?;
        ok(&ops::kick(&self.ctx, w, false).await.map_err(fail)?)
    }

    #[tool(
        description = "List checkbox tasks (ids `path#index`) and file tasks (`path#task`) across the vault or under a path."
    )]
    async fn list_tasks(
        &self,
        Parameters(a): Parameters<ListTasksArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let summary = tasks::wants_summary(a.path.as_deref(), a.full.unwrap_or(false));
        let f = tasks::Filter { status: a.status, due: a.due, tag: a.tag, prefix: a.path };
        let list = tasks::list(&self.ctx.vault.root, &f).map_err(fail)?;
        if summary { ok(&tasks::summarize(&list)) } else { ok(&list) }
    }

    #[tool(
        description = "Toggle one task by id (open <-> done). Rewrites only that checkbox line, then kicks the index."
    )]
    async fn toggle_task(
        &self,
        Parameters(a): Parameters<ToggleArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        ok(&ops::toggle_task(&self.ctx, &a.id, false).await.map_err(fail)?)
    }
}

/// MCP `search` args → lattice params. Agent defaults: `per_doc` on.
fn search_params(a: SearchArg) -> std::result::Result<SearchParams, ErrorData> {
    let mode = match a.mode.as_deref() {
        None | Some("hybrid") => Mode::Hybrid,
        Some("bm25") => Mode::Bm25,
        Some("vector") => Mode::Vector,
        Some(other) => {
            return Err(ErrorData::invalid_params(
                format!("mode must be hybrid|bm25|vector, got {other}"),
                None,
            ));
        }
    };
    Ok(SearchParams {
        query: a.query,
        top_k: a.limit.unwrap_or(10),
        domain: a.domain,
        mode,
        per_doc: a.per_doc.unwrap_or(true),
        mmr: false,
        include_archives: false,
    })
}

#[tool_handler]
impl ServerHandler for LapisServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("lapis", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

pub async fn serve(ctx: Ctx) -> crate::error::Result<()> {
    let server = LapisServer::new(ctx);
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| LapisError::Internal(format!("mcp: {e}")))?;
    running.waiting().await.map_err(|e| LapisError::Internal(format!("mcp: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(json: &str) -> SearchArg {
        serde_json::from_str(json).unwrap()
    }

    /// N3: MCP search defaults to per_doc=true; explicit false still wins.
    #[test]
    fn mcp_search_defaults_to_per_doc() {
        let p = search_params(arg(r#"{"query":"lattice"}"#)).unwrap();
        assert!(p.per_doc);
        assert_eq!((p.top_k, p.mode), (10, Mode::Hybrid));
        let p = search_params(arg(r#"{"query":"lattice","per_doc":false,"mode":"bm25","limit":3}"#)).unwrap();
        assert!(!p.per_doc);
        assert_eq!((p.top_k, p.mode), (3, Mode::Bm25));
        assert!(search_params(arg(r#"{"query":"x","mode":"sideways"}"#)).is_err());
    }

    /// N2 / N4: arg defaults documented in the schema match the handlers.
    #[test]
    fn neighbors_and_tasks_arg_defaults() {
        let n: NeighborsArg = serde_json::from_str(r#"{"path":"a.md"}"#).unwrap();
        assert_eq!(n.direction.unwrap_or_else(|| "both".into()), "both");
        let t: ListTasksArg = serde_json::from_str(r#"{}"#).unwrap();
        assert!(tasks::wants_summary(t.path.as_deref(), t.full.unwrap_or(false)));
        let t: ListTasksArg = serde_json::from_str(r#"{"path":"foundry/lapis"}"#).unwrap();
        assert!(!tasks::wants_summary(t.path.as_deref(), t.full.unwrap_or(false)));
        let t: ListTasksArg = serde_json::from_str(r#"{"full":true}"#).unwrap();
        assert!(!tasks::wants_summary(t.path.as_deref(), t.full.unwrap_or(false)));
    }
}
