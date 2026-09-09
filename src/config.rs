//! User config: `~/.config/lapis/config.toml`.
//!
//! Only the `[lattice]` section is read in L0. The full schema is
//! `foundry/lapis/schema/lapis-config.schema.json`; unknown sections are
//! ignored so later slices can add theirs without breaking this parser.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::error::{LapisError, Result};

pub const DEFAULT_LATTICE_URL: &str = "http://127.0.0.1:8080";
pub const DEFAULT_TIMEOUT_MS: u64 = 8000;
/// SHIP.md § J: the dogfood vault.
pub const DEFAULT_VAULT: &str = "~/Obsidian/Atrium/Atrium";

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub lattice: LatticeConfig,
    #[serde(default)]
    pub vault: Option<String>,
    #[serde(default)]
    pub operator: OperatorConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct OperatorConfig {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LatticeConfig {
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_url() -> String {
    DEFAULT_LATTICE_URL.to_string()
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

impl Default for LatticeConfig {
    fn default() -> Self {
        Self { url: default_url(), timeout_ms: default_timeout() }
    }
}

impl LatticeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms.max(100))
    }
}

pub fn config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("lapis").join("config.toml"));
    }
    std::env::home_dir().map(|h| h.join(".config").join("lapis").join("config.toml"))
}

/// Load config if present. A missing file is not an error; a malformed one is
/// a usage error so the operator sees it rather than silently getting defaults.
pub fn load() -> Result<Config> {
    let Some(path) = config_path() else {
        return Ok(Config::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            toml::from_str(&text).map_err(|e| LapisError::Usage(format!("config {}: {e}", path.display())))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(LapisError::Usage(format!("config {}: {e}", path.display()))),
    }
}

/// Expand a leading `~` to the home directory. No other shell expansion.
pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::home_dir() {
            return home.join(rest);
        }
    } else if p == "~"
        && let Some(home) = std::env::home_dir()
    {
        return home;
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_sections_missing() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.lattice.url, DEFAULT_LATTICE_URL);
        assert_eq!(c.lattice.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert!(c.vault.is_none());
    }

    #[test]
    fn reads_lattice_section_and_ignores_unknown() {
        let c: Config = toml::from_str(
            "[lattice]\nurl = \"http://localhost:9999\"\ntimeout_ms = 250\n[vim]\nenabled = true\n",
        )
        .unwrap();
        assert_eq!(c.lattice.url, "http://localhost:9999");
        assert_eq!(c.lattice.timeout_ms, 250);
    }

    #[test]
    fn tilde_expands_only_leading() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(expand_tilde("/a/~/b"), PathBuf::from("/a/~/b"));
    }
}
