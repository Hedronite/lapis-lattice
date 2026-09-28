//! Flag-gated LLM tree builder. The spike ships a stub that fails closed.

use crate::error::{Result, TomeError};
use crate::types::RawNode;

pub(crate) fn stub(doc: &str) -> Result<Vec<RawNode>> {
    let _ = doc;
    Err(TomeError::NoStructure { doc: doc.to_string(), detail: DETAIL.into() })
}

#[cfg(feature = "llm-struct")]
const DETAIL: &str = "llm-struct is a stub in this spike and refuses to invent a tree";

#[cfg(not(feature = "llm-struct"))]
const DETAIL: &str = "llm-struct feature is not enabled";
