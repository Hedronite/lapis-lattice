//! `lapis mcp`: MCP server over stdio (rmcp). Every tool result is the same
//! `{ok, data, meta}` envelope `--json` prints, delivered as
//! `structuredContent` plus a short text companion (never a 300 kB text dump).
//! Notes are also exposed as `lapis://note/{path}` resources.

use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ListResourceTemplatesResult,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::{self, Meta};
use crate::error::LapisError;
use crate::lattice::{Hit, ListParams, Mode, SearchParams};
use crate::ops::{self, Ctx};
use crate::{notes, resolve, tasks, write};

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
9. `list_tasks` without `path` returns counts (n, byStatus, byFolder); pass `path` (or full=true) for rows.
10. Every result is `structuredContent` = {ok, data, meta}. meta.truncated / meta.next tell you to page (offset).
11. `search_and_read` returns top hits with HAL meta and a body snippet in one call; `read_note` takes heading / chunk / max_chars.
12. Notes are also resources: `lapis://note/{path}` (body as text/markdown). `resolve_link` maps a [[wikilink]] or dst_raw to a path.";

pub const NOTE_URI_PREFIX: &str = "lapis://note/";

/// `#[tool_handler]` in rmcp 3.x routes through `Self::tool_router()`, so the
/// server holds only its context.
#[derive(Clone)]
pub struct LapisServer {
    ctx: Arc<Ctx>,
}

fn fail(e: LapisError) -> ErrorData {
    match e {
        LapisError::Usage(_) | LapisError::Path(_) => ErrorData::invalid_params(e.to_string(), None),
        LapisError::LatticeDown(_) | LapisError::Internal(_) => {
            ErrorData::internal_error(e.to_string(), None)
        }
    }
}

/// Short text companion for a result: the whole envelope when it is small,
/// otherwise a one-line digest pointing at `structuredContent`.
pub fn summary_text(env: &Value) -> String {
    const MAX: usize = 4000;
    let full = serde_json::to_string_pretty(env).unwrap_or_default();
    if full.len() <= MAX {
        return full;
    }
    let shape = match &env["data"] {
        Value::Array(a) => format!("array of {} items", a.len()),
        Value::Object(o) => {
            format!("object with keys [{}]", o.keys().take(12).cloned().collect::<Vec<_>>().join(", "))
        }
        other => other.to_string(),
    };
    format!(
        "ok: {shape}; {} chars in structuredContent. meta: {}",
        full.len(),
        env.get("meta").cloned().unwrap_or(Value::Null)
    )
}

/// Envelope → CallToolResult with `structuredContent` and a short text block.
pub fn structured(env: Value) -> CallToolResult {
    let text = summary_text(&env);
    let mut r = CallToolResult::structured(env);
    r.content = vec![ContentBlock::text(text)];
    r
}

fn ok_at<T: serde::Serialize>(t0: Instant, v: &T) -> std::result::Result<CallToolResult, ErrorData> {
    ok_meta(t0, v, Meta::default())
}

fn ok_meta<T: serde::Serialize>(
    t0: Instant,
    v: &T,
    meta: Meta,
) -> std::result::Result<CallToolResult, ErrorData> {
    let env = envelope::ok(v, meta.with_latency(t0.elapsed().as_secs_f64() * 1000.0));
    let env = serde_json::to_value(env).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(structured(env))
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadArg {
    /// Vault-relative note path, e.g. `foundry/lapis/SPEC.md`.
    pub path: String,
    /// Only the section under this heading (case-insensitive; nested sub-sections included).
    pub heading: Option<String>,
    /// Only the N-th heading section (0 = preamble / first section), local outline order.
    pub chunk: Option<usize>,
    /// Clip the body to this many chars; `meta.truncated` says when it happened. Default `[agent].read_max_chars`.
    pub max_chars: Option<usize>,
    /// HAL and metadata only, no body.
    pub meta_only: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchArg {
    /// Query text.
    pub query: String,
    /// Result count (1..50), default 10.
    pub limit: Option<u32>,
    /// Skip this many hits; offset + limit ≤ 50. `meta.next` gives the next offset.
    pub offset: Option<u32>,
    /// Filter by HAL domain.
    pub domain: Option<String>,
    /// `hybrid` (default), `bm25`, or `vector`.
    pub mode: Option<String>,
    /// Collapse to the best chunk per document. Default `[agent].per_doc` (true).
    pub per_doc: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchReadArg {
    /// Query text.
    pub query: String,
    /// Top-k notes to read (1..20), default 5.
    pub limit: Option<u32>,
    /// Filter by HAL domain.
    pub domain: Option<String>,
    /// `hybrid` (default), `bm25`, or `vector`.
    pub mode: Option<String>,
    /// Max chars of body snippet per hit, default 600.
    pub snippet_chars: Option<usize>,
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
    /// `out`, `in`, or `both`. Default `[agent].neighbors_direction` (both).
    pub direction: Option<String>,
    /// Include dangling links too (rows with `path: null` and `dst_raw`).
    pub dangling: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResolveArg {
    /// `[[Name]]`, `[[Name|alias#anchor]]`, or a raw target such as a neighbors `dst_raw`.
    pub link: String,
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
    /// Compute path, HAL and text; write nothing.
    pub dry_run: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AppendArg {
    pub path: String,
    /// Markdown to append.
    pub text: String,
    /// Report the result without writing.
    pub dry_run: Option<bool>,
    /// Refuse if the note's `updatedAt` (ms) no longer matches.
    pub if_mtime: Option<u64>,
    /// Refuse if the note's `hash` no longer matches.
    pub if_hash: Option<String>,
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
    /// Return every row even when unscoped (default `[agent].task_unscoped`).
    pub full: Option<bool>,
    /// Rows per page when listing rows (default 500, 0 = all).
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ToggleArg {
    /// Task id `path#index` or `path#task`, from list_tasks.
    pub id: String,
    /// Report the flipped task without writing.
    pub dry_run: Option<bool>,
    /// Refuse if the note's `updatedAt` (ms) no longer matches.
    pub if_mtime: Option<u64>,
    /// Refuse if the note's `hash` no longer matches.
    pub if_hash: Option<String>,
}

fn guard(dry_run: Option<bool>, if_mtime: Option<u64>, if_hash: Option<String>) -> write::Guard {
    write::Guard { dry_run: dry_run.unwrap_or(false), if_mtime, if_hash }
}

fn parse_mode(m: Option<&str>) -> std::result::Result<Mode, ErrorData> {
    match m {
        None | Some("hybrid") => Ok(Mode::Hybrid),
        Some("bm25") => Ok(Mode::Bm25),
        Some("vector") => Ok(Mode::Vector),
        Some(other) => {
            Err(ErrorData::invalid_params(format!("mode must be hybrid|bm25|vector, got {other}"), None))
        }
    }
}

/// MCP `search` args → lattice params. `per_doc` falls back to the agent
/// profile default. Returns `(params, limit, offset)`; `top_k` is offset+limit.
pub fn search_params(
    a: SearchArg,
    default_per_doc: bool,
) -> std::result::Result<(SearchParams, u32, u32), ErrorData> {
    let mode = parse_mode(a.mode.as_deref())?;
    let limit = a.limit.unwrap_or(10).max(1);
    let offset = a.offset.unwrap_or(0);
    if offset + limit > 50 {
        return Err(ErrorData::invalid_params(
            format!("offset + limit must be ≤ 50 (got {})", offset + limit),
            None,
        ));
    }
    Ok((
        SearchParams {
            query: a.query,
            top_k: offset + limit,
            domain: a.domain,
            mode,
            per_doc: a.per_doc.unwrap_or(default_per_doc),
            mmr: false,
            include_archives: false,
        },
        limit,
        offset,
    ))
}

/// `lapis://note/{path}` → vault-relative path (percent-decoded).
pub fn note_uri_to_rel(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix(NOTE_URI_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(percent_decode(rest))
}

pub fn rel_to_note_uri(rel: &str) -> String {
    let mut out = String::with_capacity(rel.len() + NOTE_URI_PREFIX.len());
    out.push_str(NOTE_URI_PREFIX);
    for b in rel.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Snippet for `search_and_read`: the live section under the hit's heading
/// when the body has one, else the lattice snippet, else the body head.
pub fn snippet_for(
    body: Option<&str>,
    heading: Option<&str>,
    hit_snippet: Option<&str>,
    max: usize,
) -> (String, bool) {
    if let (Some(b), Some(h)) = (body, heading)
        && let Some(sec) = notes::section(b, h)
    {
        return notes::clip(&sec, max);
    }
    if let Some(s) = hit_snippet.filter(|s| !s.trim().is_empty()) {
        return notes::clip(s.trim(), max);
    }
    notes::clip(body.unwrap_or("").trim(), max)
}

fn hit_meta(h: &Hit) -> Value {
    json!({
        "rank": h.rank, "path": h.path, "uri": rel_to_note_uri(&h.path), "title": h.title, "kind": h.kind,
        "heading": h.heading, "score": h.score, "domain": h.domain, "docType": h.doc_type, "tags": h.tags,
    })
}

#[tool_router]
impl LapisServer {
    pub fn new(ctx: Ctx) -> Self {
        Self { ctx: Arc::new(ctx) }
    }

    #[tool(description = "Vault root, overlay buckets, and lattice health. Call once per session.")]
    async fn vault_info(&self) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let (info, _) = ops::vault_info(&self.ctx).await.map_err(fail)?;
        ok_at(t0, &info)
    }

    #[tool(description = "Lattice /healthz.")]
    async fn health(&self) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        ok_at(t0, &self.ctx.client().map_err(fail)?.health().await.map_err(fail)?)
    }

    #[tool(
        description = "Read one note: body plus parsed HAL frontmatter (hal, halValid, hash). Slice with heading / chunk, clip with max_chars, or meta_only."
    )]
    async fn read_note(
        &self,
        Parameters(a): Parameters<ReadArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let mut note = notes::read(&self.ctx.vault.root, &a.path).map_err(fail)?;
        if a.meta_only.unwrap_or(false) {
            let v = json!({
                "path": note.path, "kind": note.kind, "title": note.title, "hal": note.hal,
                "halValid": note.hal_valid, "halError": note.hal_error, "tags": note.tags,
                "size": note.size, "updatedAt": note.updated_at, "hash": note.hash,
            });
            return ok_at(t0, &v);
        }
        let max = a.max_chars.or(self.ctx.cfg.agent.read_max_chars);
        let (body, truncated) = notes::excerpt(&note, a.heading.as_deref(), a.chunk, max).map_err(fail)?;
        note.body = body;
        ok_meta(t0, &note, Meta { truncated, ..Meta::default() })
    }

    #[tool(
        description = "Hybrid lattice search (BM25 + vectors + title). Hits carry rank, domain, doc_type; meta has per-arm latency and paging."
    )]
    async fn search(
        &self,
        Parameters(a): Parameters<SearchArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let (p, limit, offset) = search_params(a, self.ctx.cfg.agent.per_doc)?;
        let mut r = self.ctx.client().map_err(fail)?.search(&p).await.map_err(fail)?;
        let total = r.hits.len();
        r.hits = r.hits.into_iter().skip(offset as usize).collect();
        r.count = r.hits.len();
        let requested = offset + limit;
        let truncated = total as u32 >= requested && requested < 50;
        let meta = Meta {
            truncated,
            next: if truncated { Some(requested) } else { None },
            latency: Some(r.latency),
            count: Some(r.count),
            limit: Some(limit),
            offset: Some(offset),
            ..Meta::default()
        };
        ok_meta(t0, &r, meta)
    }

    #[tool(
        description = "Search, then read the top hits: per hit the HAL meta (name, type, domain, status, updatedAt, hash) and a body snippet (the hit's section when it exists). One call instead of search + N read_note."
    )]
    async fn search_and_read(
        &self,
        Parameters(a): Parameters<SearchReadArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let limit = a.limit.unwrap_or(5).clamp(1, 20);
        let p = SearchParams {
            query: a.query.clone(),
            top_k: limit,
            domain: a.domain,
            mode: parse_mode(a.mode.as_deref())?,
            per_doc: true,
            mmr: false,
            include_archives: false,
        };
        let r = self.ctx.client().map_err(fail)?.search(&p).await.map_err(fail)?;
        let max = a.snippet_chars.unwrap_or(600);
        let root = self.ctx.vault.root.clone();
        let mut any_truncated = false;
        let items: Vec<Value> = r
            .hits
            .iter()
            .map(|h| {
                let mut v = hit_meta(h);
                let note = notes::read(&root, &h.path).ok();
                let (snippet, truncated) = snippet_for(
                    note.as_ref().map(|n| n.body.as_str()),
                    h.heading.as_deref(),
                    h.snippet.as_deref(),
                    max,
                );
                any_truncated |= truncated;
                if let Some(n) = note {
                    let pick = |k: &str| n.hal.get(k).cloned().unwrap_or(Value::Null);
                    v["hal"] = json!({
                        "name": pick("name"), "type": pick("type"), "domain": pick("domain"),
                        "status": pick("status"), "created": pick("created"), "updated": pick("updated"),
                    });
                    v["updatedAt"] = json!(n.updated_at);
                    v["hash"] = json!(n.hash);
                    v["bodyChars"] = json!(n.body.chars().count());
                } else {
                    v["readError"] = json!("unreadable on disk");
                }
                v["snippet"] = json!(snippet);
                v["snippetTruncated"] = json!(truncated);
                v
            })
            .collect();
        let data = json!({ "query": r.query, "mode": r.mode, "modalities": r.modalities, "count": items.len(), "hits": items });
        let meta = Meta {
            truncated: any_truncated,
            latency: Some(r.latency),
            count: Some(items.len()),
            limit: Some(limit),
            offset: Some(0),
            ..Meta::default()
        };
        ok_meta(t0, &data, meta)
    }

    #[tool(
        description = "List notes from the lattice documents table (metadata only). Never walks the vault. Page with limit / offset; meta.next is the next offset."
    )]
    async fn list_notes(
        &self,
        Parameters(a): Parameters<ListArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let limit = a.limit.unwrap_or(50);
        let offset = a.offset.unwrap_or(0);
        let p = ListParams {
            domain: a.domain,
            doc_type: a.doc_type,
            status: a.status,
            tag: a.tag,
            prefix: a.prefix,
            limit,
            offset,
            include_archives: false,
        };
        let rows = ops::list(&self.ctx, p).await.map_err(fail)?;
        ok_meta(t0, &rows, Meta::page(rows.len(), limit, offset))
    }

    #[tool(
        description = "Hop-1 wikilink neighbors of a note from the lattice edges table. Dangling rows have path=null and dst_raw."
    )]
    async fn neighbors(
        &self,
        Parameters(a): Parameters<NeighborsArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let rel = notes::clean_rel(&a.path).map_err(fail)?;
        let rel = if std::path::Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel };
        let dir = a.direction.unwrap_or_else(|| self.ctx.cfg.agent.direction().to_string());
        let n = self
            .ctx
            .client()
            .map_err(fail)?
            .neighbors(&rel, &dir, !a.dangling.unwrap_or(false))
            .await
            .map_err(fail)?;
        ok_meta(t0, &n, Meta { count: Some(n.neighbors.len()), ..Meta::default() })
    }

    #[tool(
        description = "Resolve a [[wikilink]] or a neighbors dst_raw to a vault path: exact path, unique basename, collision, or dangling."
    )]
    async fn resolve_link(
        &self,
        Parameters(a): Parameters<ResolveArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        ok_at(t0, &resolve::resolve(&self.ctx.vault.root, &a.link).map_err(fail)?)
    }

    #[tool(
        description = "Create a note with a HAL create-set, then kick the lattice index. Returns the path. dry_run=true writes nothing and returns the text."
    )]
    async fn create_note(
        &self,
        Parameters(a): Parameters<CreateArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
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
            dry_run: a.dry_run.unwrap_or(false),
        };
        let w = write::create(&self.ctx.vault.root, &opts).map_err(fail)?;
        ok_at(t0, &ops::kick(&self.ctx, w, false).await.map_err(fail)?)
    }

    #[tool(
        description = "Append markdown to a note (bumps `updated:`, keeps all other frontmatter), then kick the index. dry_run / if_mtime / if_hash guard the write."
    )]
    async fn append_to_note(
        &self,
        Parameters(a): Parameters<AppendArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let g = guard(a.dry_run, a.if_mtime, a.if_hash);
        let w = write::append_with(&self.ctx.vault.root, &a.path, &a.text, &g).map_err(fail)?;
        ok_at(t0, &ops::kick(&self.ctx, w, false).await.map_err(fail)?)
    }

    #[tool(
        description = "List checkbox tasks (ids `path#index`) and file tasks (`path#task`). Unscoped → summary counts; with path or full=true → rows, paged by limit / offset."
    )]
    async fn list_tasks(
        &self,
        Parameters(a): Parameters<ListTasksArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let full = a.full.unwrap_or(self.ctx.cfg.agent.task_unscoped_full());
        let summary = tasks::wants_summary(a.path.as_deref(), full);
        let f = tasks::Filter { status: a.status, due: a.due, tag: a.tag, prefix: a.path };
        let list = tasks::list(&self.ctx.vault.root, &f).map_err(fail)?;
        if summary {
            return ok_at(t0, &tasks::summarize(&list));
        }
        let (page, meta) = envelope::slice_page(&list, a.limit.unwrap_or(500), a.offset.unwrap_or(0));
        ok_meta(t0, &page, meta)
    }

    #[tool(
        description = "Toggle one task by id (open <-> done). Rewrites only that checkbox line, then kicks the index. dry_run / if_mtime / if_hash guard the write."
    )]
    async fn toggle_task(
        &self,
        Parameters(a): Parameters<ToggleArg>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let g = guard(a.dry_run, a.if_mtime, a.if_hash);
        ok_at(t0, &ops::toggle_task_with(&self.ctx, &a.id, false, &g).await.map_err(fail)?)
    }
}

#[tool_handler]
impl ServerHandler for LapisServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_resources().build())
            .with_server_info(Implementation::new("lapis", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(format!("{NOTE_URI_PREFIX}{{path}}"), "note")
                .with_description("A vault note by vault-relative path; body as text/markdown.")
                .with_mime_type("text/markdown"),
        ]))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListResourcesResult, ErrorData> {
        // Thousands of notes: discover them through search / list_notes, then read by template.
        Ok(ListResourcesResult::default())
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ReadResourceResponse, ErrorData> {
        let Some(rel) = note_uri_to_rel(&request.uri) else {
            return Err(ErrorData::resource_not_found(
                format!("unknown resource {}; expected {NOTE_URI_PREFIX}<path>", request.uri),
                None,
            ));
        };
        let note = notes::read(&self.ctx.vault.root, &rel)
            .map_err(|e| ErrorData::resource_not_found(e.to_string(), None))?;
        let text = ResourceContents::text(note.body, request.uri.clone()).with_mime_type("text/markdown");
        Ok(ReadResourceResult::new(vec![text]).into())
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

    /// N3 / N14: MCP search defaults to the agent profile's per_doc; explicit false wins; offset pages.
    #[test]
    fn mcp_search_defaults_to_per_doc() {
        let (p, limit, offset) = search_params(arg(r#"{"query":"lattice"}"#), true).unwrap();
        assert!(p.per_doc);
        assert_eq!((p.top_k, p.mode, limit, offset), (10, Mode::Hybrid, 10, 0));
        let (p, ..) = search_params(arg(r#"{"query":"lattice"}"#), false).unwrap();
        assert!(!p.per_doc, "[agent] per_doc=false is honoured");
        let (p, limit, offset) = search_params(
            arg(r#"{"query":"lattice","per_doc":false,"mode":"bm25","limit":3,"offset":6}"#),
            true,
        )
        .unwrap();
        assert!(!p.per_doc);
        assert_eq!((p.top_k, p.mode, limit, offset), (9, Mode::Bm25, 3, 6));
        assert!(search_params(arg(r#"{"query":"x","mode":"sideways"}"#), true).is_err());
        assert!(search_params(arg(r#"{"query":"x","limit":30,"offset":30}"#), true).is_err());
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
        let g = guard(Some(true), Some(5), None);
        assert!(g.dry_run && g.if_mtime == Some(5) && g.if_hash.is_none());
    }

    /// N7: structuredContent carries the envelope; the text block stays short.
    #[test]
    fn structured_result_has_object_and_short_text() {
        let small = serde_json::to_value(envelope::ok(json!({"a": 1}), Meta::default())).unwrap();
        let r = structured(small.clone());
        assert_eq!(r.structured_content, Some(small));
        assert_eq!(r.is_error, Some(false));
        let text = r.content[0].as_text().expect("text block").text.clone();
        assert!(text.contains("\"ok\": true") && text.contains("\"a\": 1"));

        let big: Vec<Value> = (0..2000).map(|i| json!({"id": i, "content": "x".repeat(100)})).collect();
        let env = serde_json::to_value(envelope::ok(big, Meta::page(2000, 500, 0))).unwrap();
        let r = structured(env.clone());
        let text = r.content[0].as_text().expect("text block").text.clone();
        assert!(text.len() < 600, "digest, not a dump: {} chars", text.len());
        assert!(text.contains("array of 2000 items") && text.contains("\"truncated\":true"));
        assert_eq!(r.structured_content.as_ref().unwrap()["data"].as_array().unwrap().len(), 2000);
    }

    /// N15: note URIs round-trip, with percent-encoding for spaces and unicode.
    #[test]
    fn note_uri_roundtrip() {
        let rel = "Cross-References/Hedronite Capital · v2.md";
        let uri = rel_to_note_uri(rel);
        assert!(uri.starts_with("lapis://note/Cross-References/Hedronite%20Capital"));
        assert_eq!(note_uri_to_rel(&uri).as_deref(), Some(rel));
        assert_eq!(note_uri_to_rel("lapis://note/a/b.md").as_deref(), Some("a/b.md"));
        assert_eq!(note_uri_to_rel("lapis://note/"), None);
        assert_eq!(note_uri_to_rel("file:///etc/passwd"), None);
    }

    /// N16: snippet prefers the live section under the hit's heading.
    #[test]
    fn snippet_prefers_section_then_lattice_then_head() {
        let body = "# Top\n\nhead text\n\n## Goals\n\ngoal text here\n";
        let (s, t) = snippet_for(Some(body), Some("Goals"), Some("lattice chunk"), 100);
        assert!(s.starts_with("## Goals") && s.contains("goal text") && !t);
        let (s, _) = snippet_for(Some(body), Some("Missing"), Some("lattice chunk"), 100);
        assert_eq!(s, "lattice chunk");
        let (s, t) = snippet_for(Some(body), None, None, 5);
        assert_eq!((s.as_str(), t), ("# Top", true));
        let (s, _) = snippet_for(None, None, None, 5);
        assert_eq!(s, "");
    }
}
