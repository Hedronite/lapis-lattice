//! Adapter over the frozen `tome_tree` read surface (PR #32, `crates/tome-tree/README.md`).
//!
//! All types are re-exported from `tome_tree`; nothing is redefined here. `TomeApi` is a
//! thin object-safe trait so the MCP tools and the harness can take either the real
//! `TomeIndex` or the in-memory `FakeTome` used by offline tests.
//!
//! Clean-room: concepts from VectifyAI/PageIndex@619cbd8 (MIT); no code copied.

pub use tome_tree::{
    BEAM, BUILDER_VERSION, Budget, Candidate, DocId, DocMeta, Judge, Node, NodeId, NodeSource, OPEN_BYTE_CAP,
    OPEN_PAGE_CAP, OpenPassages, Passage, Result, TomeError, TomeIndex, Walk,
};

/// The frozen §3 surface, object-safe. Every call returns `Result`; empty is never an error
/// and an error is never empty.
pub trait TomeApi: Send + Sync {
    fn docs(&self) -> Result<Vec<DocMeta>>;
    /// Stored meta for one doc; refuses a stale tree like `tree`.
    fn meta(&self, doc: &DocId) -> Result<DocMeta>;
    fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>>;
    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>>;
    fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk>;
    /// `tome_tree` | `fake`, recorded in eval results.
    fn backend_name(&self) -> &'static str;
    fn builder_version(&self) -> String {
        BUILDER_VERSION.to_string()
    }
}

impl TomeApi for TomeIndex {
    fn docs(&self) -> Result<Vec<DocMeta>> {
        TomeIndex::docs(self)
    }
    fn meta(&self, doc: &DocId) -> Result<DocMeta> {
        TomeIndex::meta(self, doc)
    }
    fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>> {
        TomeIndex::tree(self, doc, node, depth)
    }
    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        OpenPassages::open(self, doc, nodes)
    }
    fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk> {
        TomeIndex::walk(self, doc, query, judge, budget)
    }
    fn backend_name(&self) -> &'static str {
        "tome_tree"
    }
}

/// Placeholder when the tome arm is not requested (`--arms baseline`): never walked.
/// Any call is a harness bug and fails closed with `parse`.
pub struct NoTomeArm;

impl NoTomeArm {
    fn err<T>() -> Result<T> {
        Err(TomeError::Parse("tome arm not requested for this run".into()))
    }
}

impl TomeApi for NoTomeArm {
    fn docs(&self) -> Result<Vec<DocMeta>> {
        Self::err()
    }
    fn meta(&self, _: &DocId) -> Result<DocMeta> {
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
        "none"
    }
}

/// `true` for exactly 64 lowercase hex characters (a PDF sha256 doc id).
///
/// Callers check this before handing a doc id to the library: until the library's own
/// `DocId` validation lands, an unchecked id can reach a filesystem path.
pub fn is_doc_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `true` when every segment is a 4-digit number (`0003`, `0003.0002`).
pub fn is_node_id(s: &str) -> bool {
    !s.is_empty() && s.split('.').all(|p| p.len() == 4 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Caller mistakes (bad doc / node / budget) versus backend failures.
pub fn is_caller_error(e: &TomeError) -> bool {
    matches!(e.code(), "unknown_doc" | "unknown_node" | "over_budget")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_ids_are_exactly_64_lowercase_hex() {
        assert!(is_doc_sha256(&"a".repeat(64)));
        assert!(is_doc_sha256("6cba1924c6adc331dc3c61419935d8101e4f698069228517cd3999859bc7cd03"));
        for bad in [
            "".to_string(),
            "A".repeat(64),
            "a".repeat(63),
            "a".repeat(65),
            format!("{}g", "a".repeat(63)),
            "../../etc/passwd".into(),
            format!("../{}", "a".repeat(61)),
            format!("{} ", "a".repeat(63)),
        ] {
            assert!(!is_doc_sha256(&bad), "{bad:?}");
        }
    }

    #[test]
    fn node_ids_are_dotted_4_digit_paths() {
        assert!(is_node_id("0003") && is_node_id("0003.0002"));
        for bad in ["3.2", "", "0003.", "../0001", "0003/0001"] {
            assert!(!is_node_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn real_index_on_an_empty_dir_lists_nothing_and_rejects_unknown_docs() {
        let dir = std::env::temp_dir().join(format!("tome-eval-contract-{}", std::process::id()));
        let idx = TomeIndex::open(&dir).unwrap();
        let api: &dyn TomeApi = &idx;
        assert!(api.docs().unwrap().is_empty());
        let e = api.tree(&DocId("b".repeat(64)), None, None).unwrap_err();
        assert_eq!(e.code(), "unknown_doc");
        assert_eq!(api.backend_name(), "tome_tree");
        let _ = std::fs::remove_dir_all(dir);
    }
}
