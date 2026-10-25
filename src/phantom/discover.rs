//! Find the Phantom code-graph binary (D1).
//!
//! Only an executable named `phantom-fractal` counts: first on `PATH`, then a
//! configured path whose file name is `phantom-fractal`. `fract` is never looked
//! up, so another tool by that name cannot shadow Phantom. Nothing here runs the
//! binary or reads Phantom's data; absence is `None`, not an error.
//!
//! Std only, so `tests/phantom_discover.rs` can include this file directly.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The executable name Lapis looks for, and the only one.
pub const BINARY: &str = "phantom-fractal";

/// Environment variable naming a configured `phantom-fractal` path.
pub const ENV: &str = "LAPIS_PHANTOM_BINARY";

/// Discovery against this process's `PATH` and `$LAPIS_PHANTOM_BINARY`.
pub fn from_env() -> Option<PathBuf> {
    let configured = std::env::var_os(ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
    discover(std::env::var_os("PATH").as_deref(), configured.as_deref())
}

/// `phantom-fractal` on `path_var` (a `PATH`-style list), else `configured` when
/// its file name is `phantom-fractal`. `None` when neither is an executable file.
pub fn discover(path_var: Option<&OsStr>, configured: Option<&Path>) -> Option<PathBuf> {
    on_path(path_var).or_else(|| configured.filter(|p| named(p) && is_executable(p)).map(Path::to_path_buf))
}

fn on_path(path_var: Option<&OsStr>) -> Option<PathBuf> {
    // An empty entry would mean the current directory; skip it rather than
    // pick up a binary from wherever `lapis` happens to be run.
    std::env::split_paths(path_var?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(file_name()))
        .find(|p| is_executable(p))
}

fn file_name() -> String {
    format!("{BINARY}{}", std::env::consts::EXE_SUFFIX)
}

fn named(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n == OsStr::new(&file_name()))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file())
}
