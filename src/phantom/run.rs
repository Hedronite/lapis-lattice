//! Spawn `phantom-fractal --json` once and parse its stdout (G0c, G3c).
//!
//! The path from [`super::discover`] is stat-ed again immediately before the
//! spawn: absolute, a file, named `phantom-fractal`, executable. A failed
//! re-check spawns nothing. There is one spawn per call; `meta.next` is not
//! followed and no Phantom database is read.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::discover;
use super::envelope::{self, EXITS, ParseError, Reply};

/// Why a call produced no [`Reply`]. Every case is reported, never an empty overlay.
#[derive(Debug)]
pub enum RunError {
    /// G0c: the path failed its re-check before spawn. Nothing was spawned.
    Recheck { path: PathBuf, reason: &'static str },
    /// The OS could not start the process.
    Spawn { path: PathBuf, source: std::io::Error },
    /// The process exited outside 0 through 6, or was killed (`None`).
    UnknownStatus { status: Option<i32>, stderr: String },
    /// A non-zero status whose stdout is not an envelope.
    Failed { status: i32, stderr: String },
    /// A zero status whose stdout is not a valid envelope, or an envelope
    /// with an `error.exit` outside 0 through 6.
    Envelope(ParseError),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Recheck { path, reason } => {
                write!(f, "{} failed its check before spawn: {reason}", path.display())
            }
            RunError::Spawn { path, source } => write!(f, "could not start {}: {source}", path.display()),
            RunError::UnknownStatus { status: Some(n), stderr } => {
                write!(f, "phantom-fractal exited {n}, outside 0-6{}", tail(stderr))
            }
            RunError::UnknownStatus { status: None, stderr } => {
                write!(f, "phantom-fractal was killed by a signal{}", tail(stderr))
            }
            RunError::Failed { status, stderr } => {
                write!(f, "phantom-fractal exited {status} without an envelope{}", tail(stderr))
            }
            RunError::Envelope(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RunError {}

fn tail(stderr: &str) -> String {
    let line = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.is_empty() { String::new() } else { format!(": {line}") }
}

/// G0c: the checks [`run`] makes immediately before spawn.
pub fn recheck(path: &Path) -> Result<(), RunError> {
    let fail = |reason| Err(RunError::Recheck { path: path.to_path_buf(), reason });
    if !path.is_absolute() {
        return fail("not an absolute path");
    }
    if !discover::named(path) {
        return fail("file name is not phantom-fractal");
    }
    match std::fs::metadata(path) {
        Err(_) => fail("no such file"),
        Ok(m) if !m.is_file() => fail("not a file"),
        Ok(_) if !discover::is_executable(path) => fail("not executable"),
        Ok(_) => Ok(()),
    }
}

/// Run `phantom-fractal --json <args>` once and parse the envelope it prints.
///
/// An envelope with `error.exit` 0 through 6 is a [`Reply`], whatever the
/// process status. Anything else is a [`RunError`].
pub fn run(path: &Path, args: &[&str]) -> Result<Reply, RunError> {
    recheck(path)?;
    let out = Command::new(path)
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| RunError::Spawn { path: path.to_path_buf(), source })?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let status = out.status.code();
    match status {
        Some(n) if EXITS.contains(&i64::from(n)) => {}
        _ => return Err(RunError::UnknownStatus { status, stderr }),
    }
    match envelope::parse(&out.stdout) {
        Ok(reply) => Ok(reply),
        Err(ParseError::NotEnvelope(_)) if status != Some(0) => {
            Err(RunError::Failed { status: status.unwrap_or_default(), stderr })
        }
        Err(e) => Err(RunError::Envelope(e)),
    }
}
