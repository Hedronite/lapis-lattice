//! tome-tree spike MCP tools: `tome_tree` and `tome_open` only (no `tome_search`;
//! an agent walks the tree itself). Compiled only with the `tome` cargo feature and
//! registered only when `LAPIS_TOME=1` at runtime; both are off by default.
//!
//! Output is page-cited JSON in the standard `{ok, data, meta}` envelope. Every
//! failure is an MCP error carrying `data.code`, never an empty result. Caller
//! mistakes (`bad_input`, `unknown_doc`, `unknown_node`, `over_budget`) are
//! invalid_params; backend failures (`parse`, `io`, `stale`, `no_structure`,
//! `judge_unavailable`) are internal errors (see `crate::mcp::tome_error`).
//!
//! The backend is `tome_tree::TomeIndex` at `<vault>/.lapis/tomes`, behind the
//! object-safe `tome_eval::contract::TomeApi` (the frozen R2 read surface).
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
use crate::mcp::{LapisServer, tome_error};
use tome_eval::contract::{
    Budget, DocId, DocMeta, Judge, Node, NodeId, Passage, TomeApi, TomeError, TomeIndex, Walk, is_doc_sha256,
    is_node_id,
};

/// Runtime flag. Anything but `1` / `true` / `on` keeps the tools unregistered.
pub const ENV_FLAG: &str = "LAPIS_TOME";

/// Index location inside the vault (same as `lapis tome build`).
pub const INDEX_REL: &str = ".lapis/tomes";

pub fn is_on(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on")
}

pub fn enabled_from_env() -> bool {
    std::env::var(ENV_FLAG).is_ok_and(|v| is_on(&v))
}

/// An index that could not be opened: every call fails closed with the open error.
struct Unopened(String);

impl Unopened {
    fn err<T>(&self) -> tome_eval::contract::Result<T> {
        Err(TomeError::Parse(self.0.clone()))
    }
}

impl TomeApi for Unopened {
    fn docs(&self) -> tome_eval::contract::Result<Vec<DocMeta>> {
        self.err()
    }
    fn meta(&self, _: &DocId) -> tome_eval::contract::Result<DocMeta> {
        self.err()
    }
    fn tree(&self, _: &DocId, _: Option<&NodeId>, _: Option<u8>) -> tome_eval::contract::Result<Vec<Node>> {
        self.err()
    }
    fn open(&self, _: &DocId, _: &[NodeId]) -> tome_eval::contract::Result<Vec<Passage>> {
        self.err()
    }
    fn walk(&self, _: &DocId, _: &str, _: &dyn Judge, _: Budget) -> tome_eval::contract::Result<Walk> {
        self.err()
    }
    fn backend_name(&self) -> &'static str {
        "unopened"
    }
}

/// `tome_tree::TomeIndex` at `<vault>/.lapis/tomes`. Only called when the flag is on.
pub fn backend(vault_root: &Path) -> Arc<dyn TomeApi> {
    match TomeIndex::open(&vault_root.join(INDEX_REL)) {
        Ok(idx) => Arc::new(idx),
        Err(e) => Arc::new(Unopened(format!("tome index {INDEX_REL}: {e}"))),
    }
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
    /// Doc id: the PDF's sha256, exactly 64 lowercase hex characters.
    pub doc: String,
    /// Dotted node id (e.g. `0003.0002`); omit for the top level.
    pub node: Option<String>,
    /// Levels of children to include (default 2). `child_count` shows what was cut.
    pub depth: Option<u8>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TomeOpenArg {
    /// Doc id: the PDF's sha256, exactly 64 lowercase hex characters.
    pub doc: String,
    /// Node ids to open. Max 12 pages / 48 KB per call; over that is an `over_budget` error.
    #[serde(alias = "nodes")]
    pub node_ids: Vec<String>,
}

/// TomeError → MCP error with `data.code`.
pub fn tome_err(e: TomeError) -> ErrorData {
    tome_error(e.code(), e.to_string())
}

fn bad_input(msg: String) -> ErrorData {
    tome_error("bad_input", msg)
}

/// Checked before the library sees the id: exactly 64 lowercase hex, nothing else
/// (no trimming, no case folding), so no caller string can reach a filesystem path.
fn doc_id(raw: &str) -> Result<DocId, ErrorData> {
    if is_doc_sha256(raw) {
        Ok(DocId(raw.to_string()))
    } else {
        Err(bad_input(format!("doc must be the PDF sha256 (64 lowercase hex characters), got {raw:?}")))
    }
}

fn node_id(raw: &str) -> Result<NodeId, ErrorData> {
    if is_node_id(raw) {
        Ok(NodeId(raw.to_string()))
    } else {
        Err(bad_input(format!("node id must be a dotted path like 0003.0002, got {raw:?}")))
    }
}

/// `tome_tree` body: `{doc: DocMeta, nodes: [Node]}`.
pub fn tree_value(api: &dyn TomeApi, a: &TomeTreeArg) -> Result<Value, ErrorData> {
    let doc = doc_id(&a.doc)?;
    let node = a.node.as_deref().map(node_id).transpose()?;
    let depth = Some(a.depth.unwrap_or(2).clamp(1, 8));
    let meta = api.meta(&doc).map_err(tome_err)?;
    let nodes = api.tree(&doc, node.as_ref(), depth).map_err(tome_err)?;
    Ok(json!({ "doc": meta, "nodes": nodes }))
}

/// `tome_open` body: `{doc_id, path, pages, passages: [{node_id, page, text, truncated}]}`.
pub fn open_value(api: &dyn TomeApi, a: &TomeOpenArg) -> Result<Value, ErrorData> {
    let doc = doc_id(&a.doc)?;
    if a.node_ids.is_empty() {
        return Err(bad_input("node_ids must not be empty".into()));
    }
    let ids = a.node_ids.iter().map(|n| node_id(n)).collect::<Result<Vec<_>, _>>()?;
    let meta = api.meta(&doc).map_err(tome_err)?;
    let passages = api.open(&doc, &ids).map_err(tome_err)?;
    if passages.is_empty() {
        // Fail closed: a node always spans ≥ 1 page, so nothing back is a backend bug.
        return Err(tome_error("parse", "open returned no passages".into()));
    }
    let mut pages: Vec<u32> = passages.iter().map(|p| p.page).collect();
    pages.sort_unstable();
    pages.dedup();
    Ok(json!({ "doc_id": meta.doc_id, "path": meta.path, "pages": pages, "passages": passages }))
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
                Some(json!({ "code": "disabled" })),
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
    use rmcp::model::ErrorCode;
    use tome_eval::fake::{FakeTome, no_structure_doc, sample_doc};

    fn doc() -> String {
        sample_doc().0
    }

    fn code(e: &ErrorData) -> String {
        e.data.as_ref().and_then(|d| d["code"].as_str()).unwrap_or("").to_string()
    }

    fn tree(api: &dyn TomeApi, doc: &str) -> Result<Value, ErrorData> {
        tree_value(api, &TomeTreeArg { doc: doc.into(), node: None, depth: None })
    }

    #[test]
    fn tools_register_only_with_the_runtime_flag() {
        let fake = || -> Arc<dyn TomeApi> { Arc::new(FakeTome::sample()) };
        let (off, none) = with_flag(LapisServer::tool_router(), false, fake);
        assert!(!off.has_route("tome_tree") && !off.has_route("tome_open") && none.is_none());
        let (on, some) = with_flag(LapisServer::tool_router(), true, fake);
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
    fn doc_must_be_64_lowercase_hex_before_the_library_is_called() {
        // Unopened fails every library call with `parse`; a bad_input answer proves the
        // check ran first and the library was never reached.
        let api = Unopened("must not be reached".into());
        for bad in [
            "../../../etc/passwd".to_string(),
            "Archmagus-Stack/09-Tomes/x.pdf".into(),
            "A".repeat(64),
            format!(" {}", "a".repeat(64)),
            format!("{}\n", "a".repeat(64)),
            "a".repeat(63),
            format!("../{}", "a".repeat(61)),
            String::new(),
        ] {
            let e = tree(&api, &bad).unwrap_err();
            assert_eq!((e.code, code(&e).as_str()), (ErrorCode::INVALID_PARAMS, "bad_input"), "{bad:?}");
            let e = open_value(&api, &TomeOpenArg { doc: bad.clone(), node_ids: vec!["0001".into()] })
                .unwrap_err();
            assert_eq!(code(&e), "bad_input", "{bad:?}");
        }
        let e = tree(&api, &doc()).unwrap_err();
        assert_eq!((e.code, code(&e).as_str()), (ErrorCode::INTERNAL_ERROR, "parse"));
    }

    #[test]
    fn tree_is_page_cited_and_cut_at_depth() {
        let api = FakeTome::sample();
        let v = tree_value(&api, &TomeTreeArg { doc: doc(), node: None, depth: Some(1) }).unwrap();
        assert_eq!(v["doc"]["sha256"], doc());
        assert_eq!(v["doc"]["summary_model"], "fake/lead");
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
        let open = |ids: &[&str]| {
            open_value(
                &api,
                &TomeOpenArg { doc: doc(), node_ids: ids.iter().map(|s| s.to_string()).collect() },
            )
        };
        let v = open(&["0002.0002"]).unwrap();
        assert_eq!(v["pages"], json!([15, 16, 17, 18, 19, 20]));
        assert_eq!(v["passages"][0]["page"], 15);
        assert_eq!(v["doc_id"], doc());
        for (e, want, kind) in [
            (open(&["0001", "0002"]).unwrap_err(), "over_budget", ErrorCode::INVALID_PARAMS),
            (open(&["0009"]).unwrap_err(), "unknown_node", ErrorCode::INVALID_PARAMS),
            (open(&["3.2"]).unwrap_err(), "bad_input", ErrorCode::INVALID_PARAMS),
            (open(&[]).unwrap_err(), "bad_input", ErrorCode::INVALID_PARAMS),
            (tree(&api, &"c".repeat(64)).unwrap_err(), "unknown_doc", ErrorCode::INVALID_PARAMS),
            (tree(&api, &no_structure_doc().0).unwrap_err(), "no_structure", ErrorCode::INTERNAL_ERROR),
        ] {
            assert_eq!((code(&e).as_str(), e.code), (want, kind));
        }
    }

    #[test]
    fn real_backend_on_an_empty_vault_fails_closed() {
        let vault = std::env::temp_dir().join(format!("lapis-mcp-tome-{}", std::process::id()));
        let api = backend(&vault);
        assert_eq!(api.backend_name(), "tome_tree");
        let e = tree(api.as_ref(), &doc()).unwrap_err();
        assert_eq!((code(&e).as_str(), e.code), ("unknown_doc", ErrorCode::INVALID_PARAMS));
        let _ = std::fs::remove_dir_all(vault);
    }

    #[test]
    fn open_accepts_marci_nodes_alias() {
        let a: TomeOpenArg = serde_json::from_str(r#"{"doc":"d","nodes":["0001"]}"#).unwrap();
        assert_eq!(a.node_ids, vec!["0001"]);
    }
}
