//! Page-spanned table-of-contents trees for long PDFs.
//!
//! Clean-room: concepts from VectifyAI/PageIndex@619cbd8 (MIT). No code copied.
//! See `PROVENANCE.md`.
//!
//! # Frozen interface
//!
//! ```ignore
//! let index = TomeIndex::open(index_dir)?;
//! let _docs = index.docs()?;
//! let _nodes = index.tree(&doc, None, Some(2))?;
//! let _pages = OpenPassages::open(&index, &doc, &node_ids)?;
//! let _walk = index.walk(&doc, query, &judge, Budget::default())?;
//! ```
//!
//! `TomeIndex::open` is the constructor. Passage `open` is the [`OpenPassages`]
//! method, because Rust has one `open` per type. `use tome_tree::OpenPassages`
//! makes `index.open(&doc, &ids)?` resolve. [`TomeIndex::passages`] is the same
//! read without the trait import.
//!
//! [`TomeIndex::walk`] with the Jev judge needs a multi-thread tokio runtime.

#![forbid(unsafe_code)]

mod error;
mod headings;
mod llm;
mod outline;
mod pdf;
mod split;
mod store;
mod types;
mod walk;

pub use error::{Result, TomeError};
pub use store::{BuildOptions, TomeIndex};
pub use types::{
    BEAM, BUILDER_VERSION, Budget, Candidate, DEFAULT_JUDGE_CALLS, DocId, DocMeta, LEAD_CHARS, Node, NodeId,
    NodeSource, OPEN_BYTE_CAP, OPEN_PAGE_CAP, Passage, SPLIT_PAGES, SPLIT_TOKENS, STOP_PAGES, SummaryModel,
    Walk,
};
pub use walk::{FakeJudge, Judge};

/// SHA-256 of a PDF, lowercase hex. This is the doc id.
pub fn content_id(path: &std::path::Path) -> Result<DocId> {
    DocId::parse(&pdf::sha256_file(path)?)
}

/// Passage read. Import this trait to call `index.open(doc, nodes)`.
pub trait OpenPassages {
    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>>;
}

impl OpenPassages for TomeIndex {
    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        self.read_passages(doc, nodes)
    }
}

pub mod prelude {
    pub use crate::{Budget, DocId, FakeJudge, Judge, NodeId, OpenPassages, TomeIndex};
}
