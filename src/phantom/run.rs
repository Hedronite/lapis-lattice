//! Spawn `phantom-fractal --json` once and parse its stdout (G0c, G3c).
//!
//! The path from [`super::discover`] is stat-ed again immediately before the
//! spawn: absolute, a file, named `phantom-fractal`, executable. A failed
//! re-check spawns nothing. There is one spawn per call; `meta.next` is not
//! followed and no Phantom database is read.
//!
//! The child is bounded: it sees only an allowlisted environment, its output is
//! read up to a byte cap, and it is killed if it outlives [`TIMEOUT`]. A slow or
//! noisy binary costs Lapis a typed error, never a hang or an unbounded buffer.
//! On unix the child leads its own process group, and stopping it kills the
//! whole group, so nothing it started outlives it holding the pipes.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    /// The process outlived the time limit and was killed.
    Timeout { after: Duration },
    /// The process wrote more than `cap` bytes to `stream` and was killed.
    Overflow { stream: &'static str, cap: usize },
}

/// How long one call may run before the child is killed.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes read from the child's stdout; an envelope is far smaller.
pub const STDOUT_CAP: usize = 1 << 20;
/// Bytes read from the child's stderr, which only feeds an error's last line.
pub const STDERR_CAP: usize = 64 << 10;

/// The environment a child keeps. Everything else, tokens included, is cleared.
/// Phantom finds its data under `$PHANTOM_FRACTAL_DATA`, else `$HOME` (macOS)
/// or `$XDG_DATA_HOME` (elsewhere), so those pass.
fn keep_env(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    matches!(name.as_ref(), "PATH" | "HOME" | "XDG_DATA_HOME" | "TMPDIR" | "USER" | "LANG" | "TZ")
        || name.starts_with("LC_")
        || name.starts_with("PHANTOM_")
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
            RunError::Timeout { after } => {
                write!(f, "phantom-fractal did not finish within {} s and was stopped", after.as_secs())
            }
            RunError::Overflow { stream, cap } => {
                write!(f, "phantom-fractal wrote more than {cap} bytes to {stream} and was stopped")
            }
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
    run_within(path, args, TIMEOUT)
}

/// [`run`] with its own time limit.
pub fn run_within(path: &Path, args: &[&str], timeout: Duration) -> Result<Reply, RunError> {
    recheck(path)?;
    let mut command = Command::new(path);
    command
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .envs(std::env::vars_os().filter(|(k, _)| keep_env(k)));
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn().map_err(|source| RunError::Spawn { path: path.to_path_buf(), source })?;
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        drain("stdout", out, STDOUT_CAP, tx.clone());
    }
    if let Some(err) = child.stderr.take() {
        drain("stderr", err, STDERR_CAP, tx);
    }
    // Both streams end when the child exits, unless something it started still
    // holds them open; the deadline covers that too.
    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Drained { stream: "stdout", bytes, over: false }) => stdout = bytes,
            Ok(Drained { bytes, over: false, .. }) => stderr = bytes,
            Ok(Drained { stream, over: true, .. }) => {
                stop(&mut child);
                let cap = if stream == "stdout" { STDOUT_CAP } else { STDERR_CAP };
                return Err(RunError::Overflow { stream, cap });
            }
            Err(_) => {
                stop(&mut child);
                return Err(RunError::Timeout { after: timeout });
            }
        }
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                stop(&mut child);
                return Err(RunError::Timeout { after: timeout });
            }
            Err(source) => {
                stop(&mut child);
                return Err(RunError::Spawn { path: path.to_path_buf(), source });
            }
        }
    };
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let status = status.code();
    match status {
        Some(n) if EXITS.contains(&i64::from(n)) => {}
        _ => return Err(RunError::UnknownStatus { status, stderr }),
    }
    match envelope::parse(&stdout) {
        Ok(reply) => Ok(reply),
        Err(ParseError::NotEnvelope(_)) if status != Some(0) => {
            Err(RunError::Failed { status: status.unwrap_or_default(), stderr })
        }
        Err(e) => Err(RunError::Envelope(e)),
    }
}

/// One stream read to its end, or to one byte past its cap.
struct Drained {
    stream: &'static str,
    bytes: Vec<u8>,
    over: bool,
}

fn drain(stream: &'static str, from: impl Read + Send + 'static, cap: usize, tx: mpsc::Sender<Drained>) {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // A read error ends the stream like EOF; the status still decides.
        let _ = from.take(cap as u64 + 1).read_to_end(&mut bytes);
        let over = bytes.len() > cap;
        let _ = tx.send(Drained { stream, bytes, over });
    });
}

/// Kill the child's process group, then reap the child, so neither a
/// grandchild holding the pipes nor a zombie is left behind.
fn stop(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: no pointers are passed. The child is not reaped yet, so its
        // pid is still the id of the group `process_group(0)` gave it.
        unsafe { libc::killpg(group, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
}
