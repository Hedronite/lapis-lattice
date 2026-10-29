//! Error type mirroring the CLI contract: `0` ok, `1` usage/user error,
//! `3` path escape or not found. The messages are part of the CLI's output
//! bytes and are never reformatted here.
use std::fmt;
pub type Result<T, E = Error> = std::result::Result<T, E>;
/// Agent-facing error for note creation.
#[derive(Debug)]
pub enum Error {
    /// Bad arguments, malformed input, unreadable overlay. `usage`, exit 1.
    Usage(String),
    /// A vault-relative path escaped the root or does not exist. `path`, exit 3.
    Path(String),
    /// Anything else on a file we were allowed to touch. `internal`, exit 1.
    Internal(String),
    /// Filesystem I/O on the one file we were allowed to touch. `internal`, exit 1.
    Io(std::io::Error),
}
impl Error {
    pub fn message(&self) -> String {
        match self {
            Error::Usage(m) | Error::Path(m) | Error::Internal(m) => m.clone(),
            Error::Io(e) => e.to_string(),
        }
    }
    /// The CLI's exit-code contract.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) | Error::Internal(_) | Error::Io(_) => 1,
            Error::Path(_) => 3,
        }
    }
    /// The CLI's `error.kind` for the `--json` envelope.
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Usage(_) => "usage",
            Error::Internal(_) | Error::Io(_) => "internal",
            Error::Path(_) => "path",
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::Path(e.to_string()),
            _ => Error::Internal(e.to_string()),
        }
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Internal(format!("json: {e}"))
    }
}
