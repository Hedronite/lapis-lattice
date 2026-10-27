//! Find the Phantom code-graph binary (D1).
//!
//! Only an executable named `phantom-fractal` counts. A configured path
//! (`$LAPIS_PHANTOM_BINARY`) is used exclusively when set, so a writable folder
//! early on `PATH` cannot override the binary the operator chose; `PATH` is
//! searched only when nothing is configured. `fract` is never looked up, so
//! another tool by that name cannot shadow Phantom. Nothing here runs the
//! binary or reads Phantom's data; absence is `None`, not an error.
//!
//! A found path is always absolute (G0a): relative `PATH` entries are joined to
//! the current directory. A relative `$LAPIS_PHANTOM_BINARY` is refused (G4.2):
//! it is `None`, never joined to the current directory and never a reason to
//! search `PATH`, so the binary cannot change with where `lapis` is run. The
//! parent directory is canonicalized so `..` is gone. The file name itself
//! is kept, so a `phantom-fractal` symlink is not renamed by resolving it.
//! [`super::run`] re-checks that path immediately before it spawns (G0c).
//!
//! Std only, so the `tests/phantom_*.rs` files can include this module directly.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The executable name Lapis looks for, and the only one.
pub const BINARY: &str = "phantom-fractal";

/// Environment variable naming a configured `phantom-fractal` path.
pub const ENV: &str = "LAPIS_PHANTOM_BINARY";

/// Discovery against this process's `PATH`, `$LAPIS_PHANTOM_BINARY`, and
/// current directory.
pub fn from_env() -> Option<PathBuf> {
    let configured = std::env::var_os(ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
    discover(std::env::var_os("PATH").as_deref(), configured.as_deref())
}

/// [`discover_from`] with relative paths resolved against the current directory.
pub fn discover(path_var: Option<&OsStr>, configured: Option<&Path>) -> Option<PathBuf> {
    discover_from(std::env::current_dir().ok().as_deref(), path_var, configured)
}

/// `configured` when set, if it is absolute, its file name is `phantom-fractal`
/// and it is an executable file; otherwise, with nothing configured,
/// `phantom-fractal` on `path_var` (a `PATH`-style list). A configured path
/// that fails its checks, a relative one included, is `None`: it never falls
/// back to `PATH`. Relative `PATH` entries are joined to `cwd`; with no `cwd`
/// they are skipped.
pub fn discover_from(
    cwd: Option<&Path>,
    path_var: Option<&OsStr>,
    configured: Option<&Path>,
) -> Option<PathBuf> {
    let Some(configured) = configured else { return on_path(cwd, path_var) };
    if !configured.is_absolute() || !named(configured) {
        return None;
    }
    settle(configured).filter(|p| is_executable(p))
}

fn on_path(cwd: Option<&Path>, path_var: Option<&OsStr>) -> Option<PathBuf> {
    // An empty entry would mean the current directory; skip it rather than
    // pick up a binary from wherever `lapis` happens to be run.
    std::env::split_paths(path_var?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .filter_map(|dir| absolute(cwd, &dir.join(file_name())))
        .filter_map(|p| settle(&p))
        .find(|p| is_executable(p))
}

fn absolute(cwd: Option<&Path>, p: &Path) -> Option<PathBuf> {
    if p.is_absolute() { Some(p.to_path_buf()) } else { cwd.map(|c| c.join(p)) }
}

/// Canonical parent joined with the original file name. `None` when the
/// parent does not exist.
fn settle(p: &Path) -> Option<PathBuf> {
    let name = p.file_name()?;
    let parent = p.parent().filter(|d| !d.as_os_str().is_empty())?;
    Some(parent.canonicalize().ok()?.join(name))
}

pub(crate) fn file_name() -> String {
    format!("{BINARY}{}", std::env::consts::EXE_SUFFIX)
}

pub(crate) fn named(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n == OsStr::new(&file_name()))
}

#[cfg(unix)]
pub(crate) fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(crate) fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}
