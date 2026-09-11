//! Error type and the agent-facing exit-code contract.
//!
//! SPEC.md § CLI: `0` ok, `1` usage / user error, `2` lattice down,
//! `3` path escape / not found.

use std::fmt;

#[derive(Debug)]
pub enum LapisError {
    /// Bad arguments, unreadable config, malformed input.
    Usage(String),
    /// Lattice HTTP is unreachable, timed out, or returned a non-2xx.
    LatticeDown(String),
    /// A vault-relative path escaped the root or does not exist.
    Path(String),
    /// Anything else (I/O on a file we were allowed to touch, JSON encode).
    Internal(String),
    /// Embedded index does not implement this op. `kind` is `http_only` (B11).
    HttpOnly { op: &'static str },
}

impl LapisError {
    pub fn exit_code(&self) -> i32 {
        match self {
            LapisError::Usage(_) | LapisError::Internal(_) | LapisError::HttpOnly { .. } => 1,
            LapisError::LatticeDown(_) => 2,
            LapisError::Path(_) => 3,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            LapisError::Usage(_) => "usage",
            LapisError::LatticeDown(_) => "lattice_down",
            LapisError::Path(_) => "path",
            LapisError::Internal(_) => "internal",
            LapisError::HttpOnly { .. } => "http_only",
        }
    }

    pub fn message(&self) -> String {
        match self {
            LapisError::Usage(m)
            | LapisError::LatticeDown(m)
            | LapisError::Path(m)
            | LapisError::Internal(m) => m.clone(),
            LapisError::HttpOnly { op } => format!(
                "{op} is not implemented by the embedded index; it requires `lattice.mode = \"http\"` \
                 (set it in ~/.config/lapis/config.toml or pass --lattice <url>)"
            ),
        }
    }
}

impl fmt::Display for LapisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for LapisError {}

impl From<std::io::Error> for LapisError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => LapisError::Path(e.to_string()),
            _ => LapisError::Internal(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for LapisError {
    fn from(e: serde_json::Error) -> Self {
        LapisError::Internal(format!("json: {e}"))
    }
}

impl From<lapis_lattice::Error> for LapisError {
    fn from(e: lapis_lattice::Error) -> Self {
        match e {
            lapis_lattice::Error::Usage(m) => LapisError::Usage(m),
            lapis_lattice::Error::Io(io) => LapisError::from(io),
            lapis_lattice::Error::Sqlite(s) => LapisError::LatticeDown(format!("index: {s}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, LapisError>;
