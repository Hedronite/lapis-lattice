//! Provisional R2 contract shim for `tome_tree` (Marci plan §3, refinements R1–R5).
//!
//! `crates/tome-tree` is owned by Marci and is not frozen yet. Until it is, the MCP
//! wiring and `tome-eval` code against this trait. When the R2 interface freezes, this
//! module becomes a thin adapter (`impl TomeApi for tome_tree::TomeIndex`) and the
//! local types are replaced by re-exports. Nothing here parses a PDF.
//!
//! Clean-room: concepts from VectifyAI/PageIndex@619cbd8 (MIT); no code copied.

use serde::{Deserialize, Serialize};

/// Per-call `open` cap (R5): 12 pages / 48 KB. Over the cap is an error, never a clip.
pub const OPEN_MAX_PAGES: usize = 12;
pub const OPEN_MAX_BYTES: usize = 48 * 1024;

/// Vault-relative PDF path or its sha256; resolved to the sha256 (R4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocId(pub String);

/// Dotted node path such as `0003.0002`, stable per `(sha256, builder_version)` (R4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    /// `true` when every segment is a 4-digit number (`0003`, `0003.0002`).
    pub fn is_well_formed(&self) -> bool {
        !self.0.is_empty() && self.0.split('.').all(|s| s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeSource {
    Outline,
    Heading,
    Llm,
    Window,
}

/// R3 node shape. Pages are 1-based physical PDF indices, inclusive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub title: String,
    pub level: u8,
    pub page_start: u32,
    pub page_end: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub lead: String,
    pub source: NodeSource,
    /// Children below this node, cut at `depth`; `child_count` shows what was elided.
    pub child_count: u32,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocMeta {
    pub doc_id: String,
    pub path: String,
    pub sha256: String,
    pub pages: u32,
    pub outline: bool,
    pub source: NodeSource,
    pub built_at: String,
    pub builder_version: String,
}

/// One page of opened text (R5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Passage {
    pub node_id: NodeId,
    pub page: u32,
    pub text: String,
    pub truncated: bool,
}

/// Walk budget: max judge calls + max opened pages (Marci plan §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub beam: u8,
    pub max_judge_calls: u32,
    pub max_open_pages: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { beam: 2, max_judge_calls: 24, max_open_pages: OPEN_MAX_PAGES as u32 }
    }
}

/// Result of `walk`: the chosen nodes, their opened passages, and the cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Walk {
    pub chosen: Vec<NodeId>,
    pub passages: Vec<Passage>,
    pub judge_calls: u32,
    #[serde(default)]
    pub judge_prompt_tokens: Option<u64>,
    #[serde(default)]
    pub visited: Vec<NodeId>,
}

/// Error codes serialised as `error.code` (Marci plan §3). `Unavailable` exists only in
/// this shim: it is what the stub backend returns until the crate lands.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TomeError {
    #[error("unknown doc: {0}")]
    UnknownDoc(String),
    #[error("unknown node {node} in {doc}")]
    UnknownNode { doc: String, node: String },
    #[error("stale tree for {0}: pdf or builder changed, rebuild required")]
    Stale(String),
    #[error("no structure in {doc}: no outline and heading fallback failed")]
    NoStructure { doc: String },
    #[error("parse error: {0}")]
    Parse(String),
    #[error(
        "over budget: {requested_pages} pages / {requested_bytes} bytes > {max_pages} pages / {max_bytes} bytes"
    )]
    OverBudget { requested_pages: usize, requested_bytes: usize, max_pages: usize, max_bytes: usize },
    #[error("judge unavailable: {0}")]
    JudgeUnavailable(String),
    #[error("tome backend unavailable: {0}")]
    Unavailable(String),
}

impl TomeError {
    pub fn code(&self) -> &'static str {
        match self {
            TomeError::UnknownDoc(_) => "unknown_doc",
            TomeError::UnknownNode { .. } => "unknown_node",
            TomeError::Stale(_) => "stale",
            TomeError::NoStructure { .. } => "no_structure",
            TomeError::Parse(_) => "parse",
            TomeError::OverBudget { .. } => "over_budget",
            TomeError::JudgeUnavailable(_) => "judge_unavailable",
            TomeError::Unavailable(_) => "stub",
        }
    }

    /// Caller mistakes (bad doc / node / budget) versus backend failures.
    pub fn is_caller_error(&self) -> bool {
        matches!(
            self,
            TomeError::UnknownDoc(_) | TomeError::UnknownNode { .. } | TomeError::OverBudget { .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, TomeError>;

/// A child the judge scores: title + lead only (never the full text).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate<'a> {
    pub id: &'a NodeId,
    pub title: &'a str,
    pub lead: &'a str,
    pub page_start: u32,
    pub page_end: u32,
}

/// Scores children 0–3 for a query. `Err(JudgeUnavailable)` on missing transport or
/// low confidence: the walk must fail closed, never guess a path.
pub trait Judge: Send + Sync {
    fn name(&self) -> &str;
    fn score(&self, query: &str, candidates: &[Candidate<'_>]) -> Result<Vec<u8>>;
}

/// The §3 interface. Every call returns `Result` (R1); `walk` lives with the index (R2).
pub trait TomeApi: Send + Sync {
    fn docs(&self) -> Result<Vec<DocMeta>>;
    fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>>;
    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>>;
    fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk>;
    /// `tome_tree` | `stub` | `fake`, recorded in eval results.
    fn backend_name(&self) -> &'static str;
    fn builder_version(&self) -> Option<String> {
        None
    }
    /// Resolve a doc (vault-relative path or sha256) to its meta; unknown → `UnknownDoc`.
    fn doc_meta(&self, doc: &DocId) -> Result<DocMeta> {
        self.docs()?
            .into_iter()
            .find(|d| d.path == doc.0 || d.sha256 == doc.0 || d.doc_id == doc.0)
            .ok_or_else(|| TomeError::UnknownDoc(doc.0.clone()))
    }
}

/// Placeholder until `crates/tome-tree` is on the branch: every call fails explicitly.
#[derive(Debug, Default, Clone)]
pub struct StubTome;

impl StubTome {
    fn err<T>() -> Result<T> {
        Err(TomeError::Unavailable(
            "crates/tome-tree not on spike/tome-tree yet (R2 interface not frozen)".into(),
        ))
    }
}

impl TomeApi for StubTome {
    fn docs(&self) -> Result<Vec<DocMeta>> {
        Self::err()
    }
    fn tree(&self, _: &DocId, _: Option<&NodeId>, _: Option<u8>) -> Result<Vec<Node>> {
        Self::err()
    }
    fn open(&self, _: &DocId, _: &[NodeId]) -> Result<Vec<Passage>> {
        Self::err()
    }
    fn walk(&self, _: &DocId, _: &str, _: &dyn Judge, _: Budget) -> Result<Walk> {
        Self::err()
    }
    fn backend_name(&self) -> &'static str {
        "stub"
    }
}

/// Check an `open` request against the R5 cap. Shared by the fake backend and MCP.
pub fn check_open_budget(pages: usize, bytes: usize) -> Result<()> {
    if pages > OPEN_MAX_PAGES || bytes > OPEN_MAX_BYTES {
        return Err(TomeError::OverBudget {
            requested_pages: pages,
            requested_bytes: bytes,
            max_pages: OPEN_MAX_PAGES,
            max_bytes: OPEN_MAX_BYTES,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_are_dotted_4_digit_paths() {
        assert!(NodeId("0003".into()).is_well_formed());
        assert!(NodeId("0003.0002".into()).is_well_formed());
        assert!(!NodeId("3.2".into()).is_well_formed());
        assert!(!NodeId("".into()).is_well_formed());
        assert!(!NodeId("0003.".into()).is_well_formed());
    }

    #[test]
    fn stub_fails_closed_with_a_code() {
        let s = StubTome;
        let e = s.docs().unwrap_err();
        assert_eq!(e.code(), "stub");
        assert!(s.tree(&DocId("x".into()), None, None).is_err());
    }

    #[test]
    fn open_budget_is_an_error_not_a_clip() {
        assert!(check_open_budget(12, 48 * 1024).is_ok());
        assert_eq!(check_open_budget(13, 10).unwrap_err().code(), "over_budget");
        assert_eq!(check_open_budget(1, 48 * 1024 + 1).unwrap_err().code(), "over_budget");
    }

    #[test]
    fn node_serialises_r3_shape() {
        let n = Node {
            id: NodeId("0001".into()),
            title: "Ch 1".into(),
            level: 1,
            page_start: 3,
            page_end: 9,
            page_label: None,
            summary: None,
            lead: "lead".into(),
            source: NodeSource::Outline,
            child_count: 2,
            children: vec![],
        };
        let v = serde_json::to_value(&n).unwrap();
        for k in
            ["id", "title", "level", "page_start", "page_end", "lead", "source", "child_count", "children"]
        {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert_eq!(v["source"], "outline");
        assert_eq!(v["id"], "0001");
    }
}
