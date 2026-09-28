//! tome-tree spike MCP tools: `tome_tree` and `tome_open` only (no `tome_search`;
//! an agent walks the tree itself). Compiled only with the `tome` cargo feature and
//! registered only when `LAPIS_TOME=1` at runtime; both are off by default.
//!
//! Output is page-cited JSON in the standard `{ok, data, meta}` envelope. Every
//! failure is an MCP error carrying `data.code` (unknown_doc, unknown_node, stale,
//! no_structure, parse, over_budget, judge_unavailable, stub), never an empty result.
//!
//! The backend is `tome_eval::contract::TomeApi`, a shim of Marci's R2 interface,
//! until `crates/tome-tree` freezes; today it is the fail-closed `StubTome`.
//!
//! Clean-room: concepts from VectifyAI/PageIndex@619cbd8 (MIT); no code copied.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ErrorData};
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::envelope::Meta;
use crate::mcp::LapisServer;
use tome_eval::contract::{DocId, NodeId, StubTome, TomeApi, TomeError};

/// Runtime flag. Anything but `1` / `true` / `on` keeps the tools unregistered.
pub const ENV_FLAG: &str = "LAPIS_TOME";

pub fn is_on(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on")
}

pub fn enabled_from_env() -> bool {
    std::env::var(ENV_FLAG).is_ok_and(|v| is_on(&v))
}

/// The tome backend for a vault. `<vault>/.lapis/tomes` once `tome_tree::TomeIndex`
/// lands; until then the stub, which fails every call with `code: stub`.
pub fn backend(_vault_root: &Path) -> Arc<dyn TomeApi> {
    Arc::new(StubTome)
}

/// Base router + tome tools when the runtime flag is on.
pub fn extend(
    router: ToolRouter<LapisServer>,
    vault_root: &Path,
) -> (ToolRouter<LapisServer>, Option<Arc<dyn TomeApi>>) {
    with_flag(router, enabled_from_env(), || backend(vault_root))
}

pub fn with_flag(
    router: ToolRouter<LapisServer>,
    on: bool,
    make: impl FnOnce() -> Arc<dyn TomeApi>,
) -> (ToolRouter<LapisServer>, Option<Arc<dyn TomeApi>>) {
    if on { (router + LapisServer::tome_router(), Some(make())) } else { (router, None) }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TomeTreeArg {
    /// Vault-relative PDF path (e.g. `Archmagus-Stack/09-Tomes/.../Book.pdf`) or its sha256.
    pub doc: String,
    /// Dotted node id (e.g. `0003.0002`); omit for the top level.
    pub node: Option<String>,
    /// Levels of children to include (default 2). `child_count` shows what was cut.
    pub depth: Option<u8>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TomeOpenArg {
    /// Vault-relative PDF path or its sha256.
    pub doc: String,
    /// Node ids to open. Max 12 pages / 48 KB per call; over that is an `over_budget` error.
    #[serde(alias = "nodes")]
    pub node_ids: Vec<String>,
}

/// TomeError → MCP error. Caller mistakes are invalid_params; the rest internal.
pub fn tome_err(e: TomeError) -> ErrorData {
    let data = Some(json!({ "code": e.code() }));
    if e.is_caller_error() {
        ErrorData::invalid_params(e.to_string(), data)
    } else {
        ErrorData::internal_error(e.to_string(), data)
    }
}

fn node_id(raw: &str) -> Result<NodeId, ErrorData> {
    let id = NodeId(raw.trim().to_string());
    if id.is_well_formed() {
        Ok(id)
    } else {
        Err(ErrorData::invalid_params(
            format!("node id must be a dotted path like 0003.0002, got {raw:?}"),
            Some(json!({ "code": "unknown_node" })),
        ))
    }
}

/// Path or sha256, without panicking. A 64-hex id is normalised via
/// `tome_tree::DocId::try_from`. Anything else is matched against `docs()`
/// (`DocMeta.path` / `doc_id` / `sha256`) or left as a path for `doc_meta`.
fn doc_ref(api: &dyn TomeApi, raw: &str) -> Result<DocId, ErrorData> {
    let trimmed = raw.trim();
    if let Ok(id) = tome_tree::DocId::try_from(trimmed) {
        return Ok(DocId(id.as_str().to_string()));
    }
    match api.docs() {
        Ok(docs) => {
            if let Some(meta) =
                docs.into_iter().find(|d| d.path == trimmed || d.doc_id == trimmed || d.sha256 == trimmed)
            {
                return Ok(DocId(meta.sha256));
            }
            Ok(DocId(trimmed.to_string()))
        }
        Err(err) => Err(tome_err(err)),
    }
}

/// `tome_tree` body: `{doc: DocMeta, nodes: [Node]}`.
pub fn tree_value(api: &dyn TomeApi, a: &TomeTreeArg) -> Result<Value, ErrorData> {
    let doc = doc_ref(api, &a.doc)?;
    let node = a.node.as_deref().map(node_id).transpose()?;
    let depth = Some(a.depth.unwrap_or(2).clamp(1, 8));
    let meta = api.doc_meta(&doc).map_err(tome_err)?;
    let nodes = api.tree(&doc, node.as_ref(), depth).map_err(tome_err)?;
    Ok(json!({ "doc": meta, "nodes": nodes }))
}

/// `tome_open` body: `{doc_id, path, pages, passages: [{node_id, page, text, truncated}]}`.
pub fn open_value(api: &dyn TomeApi, a: &TomeOpenArg) -> Result<Value, ErrorData> {
    if a.node_ids.is_empty() {
        return Err(ErrorData::invalid_params(
            "node_ids must not be empty",
            Some(json!({ "code": "unknown_node" })),
        ));
    }
    let doc = doc_ref(api, &a.doc)?;
    let ids = a.node_ids.iter().map(|n| node_id(n)).collect::<Result<Vec<_>, _>>()?;
    let meta = api.doc_meta(&doc).map_err(tome_err)?;
    let passages = api.open(&doc, &ids).map_err(tome_err)?;
    if passages.is_empty() {
        // Fail closed: a node always spans ≥ 1 page, so nothing back is a backend bug.
        return Err(ErrorData::internal_error("open returned no passages", Some(json!({ "code": "parse" }))));
    }
    let mut pages: Vec<u32> = passages.iter().map(|p| p.page).collect();
    pages.sort_unstable();
    pages.dedup();
    Ok(json!({ "doc_id": meta.sha256, "path": meta.path, "pages": pages, "passages": passages }))
}

#[tool_router(router = tome_router, vis = "pub(crate)")]
impl LapisServer {
    #[tool(
        description = "tome-tree (spike): section tree of a long PDF tome. Nodes carry id (dotted, e.g. 0003.0002), title, level, 1-based physical page_start/page_end, lead, source, child_count, children (cut at depth, default 2). Errors carry data.code."
    )]
    async fn tome_tree(&self, Parameters(a): Parameters<TomeTreeArg>) -> Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let api = self.tome_api()?;
        let v = tree_value(api.as_ref(), &a)?;
        self.tome_ok(t0, &v)
    }

    #[tool(
        description = "tome-tree (spike): page-cited text for node ids of a PDF tome, one passage per physical page. Max 12 pages / 48 KB per call; over that is an over_budget error (never clipped)."
    )]
    async fn tome_open(&self, Parameters(a): Parameters<TomeOpenArg>) -> Result<CallToolResult, ErrorData> {
        let t0 = Instant::now();
        let api = self.tome_api()?;
        let v = open_value(api.as_ref(), &a)?;
        self.tome_ok(t0, &v)
    }
}

impl LapisServer {
    fn tome_api(&self) -> Result<Arc<dyn TomeApi>, ErrorData> {
        self.tome.clone().ok_or_else(|| {
            ErrorData::internal_error(
                format!("tome tools are disabled (set {ENV_FLAG}=1)"),
                Some(json!({ "code": "stub" })),
            )
        })
    }

    fn tome_ok(&self, t0: Instant, v: &Value) -> Result<CallToolResult, ErrorData> {
        let env = crate::envelope::ok(v, Meta::default().with_latency(t0.elapsed().as_secs_f64() * 1000.0));
        let env = serde_json::to_value(env).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(crate::mcp::structured(env))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_eval::fake::FakeTome;

    const DOC: &str = "Archmagus-Stack/09-Tomes/fake/Fake Book.pdf";

    fn code(e: &ErrorData) -> String {
        e.data.as_ref().and_then(|d| d["code"].as_str()).unwrap_or("").to_string()
    }

    #[test]
    fn tools_register_only_with_the_runtime_flag() {
        let (off, none) = with_flag(LapisServer::tool_router(), false, || Arc::new(StubTome));
        assert!(!off.has_route("tome_tree") && !off.has_route("tome_open") && none.is_none());
        let (on, some) = with_flag(LapisServer::tool_router(), true, || Arc::new(StubTome));
        assert!(on.has_route("tome_tree") && on.has_route("tome_open") && some.is_some());
        assert!(!on.has_route("tome_search"), "no tome_search in the spike");
        assert!(on.has_route("search"), "base tools stay");
    }

    #[test]
    fn flag_parsing_is_strict() {
        for v in ["1", "true", "ON", " on "] {
            assert!(is_on(v), "{v}");
        }
        for v in ["", "0", "false", "yes", "enabled"] {
            assert!(!is_on(v), "{v}");
        }
    }

    #[test]
    fn tree_is_page_cited_and_cut_at_depth() {
        let api = FakeTome::sample();
        let v = tree_value(&api, &TomeTreeArg { doc: DOC.into(), node: None, depth: Some(1) }).unwrap();
        assert_eq!(v["doc"]["sha256"], "a".repeat(64));
        let n = &v["nodes"][1];
        assert_eq!(
            (n["id"].as_str(), n["page_start"].as_u64(), n["page_end"].as_u64()),
            (Some("0002"), Some(9), Some(20))
        );
        assert_eq!(n["child_count"], 2);
        assert_eq!(n["children"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn open_returns_pages_and_errors_map_to_mcp_errors() {
        let api = FakeTome::sample();
        let v =
            open_value(&api, &TomeOpenArg { doc: DOC.into(), node_ids: vec!["0002.0002".into()] }).unwrap();
        assert_eq!(v["pages"], json!([15, 16, 17, 18, 19, 20]));
        assert_eq!(v["passages"][0]["page"], 15);
        let over =
            open_value(&api, &TomeOpenArg { doc: DOC.into(), node_ids: vec!["0001".into(), "0002".into()] })
                .unwrap_err();
        assert_eq!(code(&over), "over_budget");
        let unk =
            tree_value(&api, &TomeTreeArg { doc: "nope.pdf".into(), node: None, depth: None }).unwrap_err();
        assert_eq!(code(&unk), "unknown_doc");
        let bad =
            open_value(&api, &TomeOpenArg { doc: DOC.into(), node_ids: vec!["3.2".into()] }).unwrap_err();
        assert_eq!(code(&bad), "unknown_node");
        let empty = open_value(&api, &TomeOpenArg { doc: DOC.into(), node_ids: vec![] }).unwrap_err();
        assert_eq!(code(&empty), "unknown_node");
        let ns = tree_value(
            &FakeTome::sample(),
            &TomeTreeArg {
                doc: "Archmagus-Stack/09-Tomes/fake/No Outline.pdf".into(),
                node: None,
                depth: None,
            },
        )
        .unwrap_err();
        assert_eq!(code(&ns), "no_structure");
    }

    #[test]
    fn stub_backend_fails_closed() {
        let e = tree_value(&StubTome, &TomeTreeArg { doc: DOC.into(), node: None, depth: None }).unwrap_err();
        assert_eq!(code(&e), "stub");
    }

    #[test]
    fn open_accepts_marci_nodes_alias() {
        let a: TomeOpenArg = serde_json::from_str(r#"{"doc":"d","nodes":["0001"]}"#).unwrap();
        assert_eq!(a.node_ids, vec!["0001"]);
    }
}
