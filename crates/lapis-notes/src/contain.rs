//! Vault-relative paths with escape rejection, and the containment check for
//! new files. Verbatim from the CLI so every error message stays byte-identical.
use crate::{Error, Result};
use std::path::{Component, Path, PathBuf};
/// Normalize a vault-relative path: POSIX separators, no leading `./`,
/// no `..`, not absolute. Returns the cleaned relative string.
pub fn clean_rel(rel: &str) -> Result<String, Error> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(Error::Usage("path is required".into()));
    }
    let p = Path::new(rel);
    if p.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') {
        return Err(Error::Path(format!("path must be vault-relative, got absolute: {rel}")));
    }
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_str().ok_or_else(|| Error::Path("non-UTF-8 path".into()))?;
                parts.push(s.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(Error::Path(format!("path escapes the vault: {rel}")));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Path(format!("path must be vault-relative: {rel}")));
            }
        }
    }
    if parts.is_empty() {
        return Err(Error::Path(format!("path resolves to the vault root: {rel}")));
    }
    Ok(parts.join("/"))
}
/// Where a new file at `rel` may be written, never following a folder symlink
/// out of the vault.
///
/// The deepest folder on the target's way that already exists must resolve
/// inside the vault, and nothing may exist at the target yet, not even a
/// dangling symlink. Missing folders below that base are new, so the caller's
/// `create_dir_all` makes real directories. A folder symlinked out of the
/// vault cannot carry a create, restore, trash or copy outside it, and the
/// write does not re-follow the links that were just checked.
///
/// `path` (`kind = "path"`) on escape, on an existing target, or on an
/// unreadable root; [`std::io::Error`] passthrough otherwise.
pub fn contained_new(root: &Path, rel: &str) -> Result<PathBuf> {
    let clean = clean_rel(rel)?;
    let root_canon = root.canonicalize().map_err(|e| Error::Path(format!("{clean}: {e}")))?;
    let target = root.join(&clean);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(Error::Path(format!("{clean}: already exists")));
    }
    let mut missing = Vec::new();
    let mut base = target.as_path();
    while std::fs::symlink_metadata(base).is_err() {
        let (Some(name), Some(parent)) = (base.file_name(), base.parent()) else { break };
        missing.push(name.to_os_string());
        base = parent;
    }
    let base = base.canonicalize().map_err(|e| Error::Path(format!("{clean}: {e}")))?;
    if !base.starts_with(&root_canon) {
        return Err(Error::Path(format!("{clean}: a folder on the way escapes the vault")));
    }
    Ok(missing.iter().rev().fold(base, |p, name| p.join(name)))
}
