//! Vault root resolution. Precedence: `--vault`, `$LAPIS_VAULT`, config
//! `vault`, then the Ship-1 dogfood default `~/Obsidian/Atrium/Atrium`.

use std::path::PathBuf;

use crate::config::{Config, DEFAULT_VAULT, expand_tilde};
use crate::error::{LapisError, Result};

#[derive(Debug, Clone)]
pub struct Vault {
    pub root: PathBuf,
    pub source: &'static str,
}

pub fn resolve(flag: Option<&str>, cfg: &Config) -> Result<Vault> {
    let (raw, source) = if let Some(v) = flag {
        (v.to_string(), "flag")
    } else if let Ok(v) = std::env::var("LAPIS_VAULT").map(|s| s.trim().to_string())
        && !v.is_empty()
    {
        (v, "env")
    } else if let Some(v) = &cfg.vault {
        (v.clone(), "config")
    } else {
        (DEFAULT_VAULT.to_string(), "default")
    };
    let root = expand_tilde(&raw);
    if !root.is_dir() {
        return Err(LapisError::Usage(format!(
            "vault is not a directory: {} (from {source})",
            root.display()
        )));
    }
    // Canonicalize so `vault info` reports a stable absolute root; the
    // per-note escape check in `notes` is lexical and does not need this.
    let root = root.canonicalize().unwrap_or(root);
    Ok(Vault { root, source })
}
