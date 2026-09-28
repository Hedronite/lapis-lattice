//! Failures a caller can tell apart from an empty tree.

use std::fmt;

/// Stable `error.code` values. Do not rename them; MCP and the CLI map `code()`
/// straight into the JSON envelope.
#[derive(Debug, thiserror::Error)]
pub enum TomeError {
    #[error("unknown doc {doc}")]
    UnknownDoc { doc: String },
    #[error("unknown node {node} in doc {doc}")]
    UnknownNode { doc: String, node: String },
    #[error("stale tree for {doc}: {detail}")]
    Stale { doc: String, detail: String },
    #[error("no structure for {doc}: {detail}")]
    NoStructure { doc: String, detail: String },
    #[error("parse: {0}")]
    Parse(String),
    #[error("io: {0}")]
    Io(String),
    #[error("over budget: {detail}")]
    OverBudget { detail: String },
    #[error("judge unavailable: {reason}")]
    JudgeUnavailable { reason: String },
}

impl TomeError {
    /// Wire code. One of `unknown_doc`, `unknown_node`, `stale`, `no_structure`,
    /// `parse`, `io`, `over_budget`, `judge_unavailable`.
    pub fn code(&self) -> &'static str {
        match self {
            TomeError::UnknownDoc { .. } => "unknown_doc",
            TomeError::UnknownNode { .. } => "unknown_node",
            TomeError::Stale { .. } => "stale",
            TomeError::NoStructure { .. } => "no_structure",
            TomeError::Parse(_) => "parse",
            TomeError::Io(_) => "io",
            TomeError::OverBudget { .. } => "over_budget",
            TomeError::JudgeUnavailable { .. } => "judge_unavailable",
        }
    }
}

pub type Result<T> = std::result::Result<T, TomeError>;

pub(crate) fn parse(err: impl fmt::Display) -> TomeError {
    TomeError::Parse(err.to_string())
}

pub(crate) fn io(err: impl fmt::Display) -> TomeError {
    TomeError::Io(err.to_string())
}
