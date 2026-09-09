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
    /// Defaults for agent surfaces (`--agent`, MCP).
    #[serde(default)]
    pub agent: AgentConfig,
    /// TUI palette: `name` picks a built-in, `custom` overrides roles with `#RRGGBB`.
    #[serde(default)]
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ThemeConfig {
    /// `lapis` (default), `parchment`, `obsidian`.
    #[serde(default)]
    pub name: Option<String>,
    /// Role → `#RRGGBB`: blue, blue_deep, blue_soft, regent, cream, gold, copper, muted, ok, warn.
    #[serde(default)]
    pub custom: std::collections::BTreeMap<String, String>,
}

/// `[agent]`: how the MCP server and `--agent` behave by default.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AgentConfig {
    /// Collapse search hits to one per document.
    #[serde(default = "default_true")]
    pub per_doc: bool,
    /// `both`, `out`, or `in`.
    #[serde(default = "default_direction")]
    pub neighbors_direction: String,
    /// `summary` (counts) or `full` (rows) for an unscoped task list.
    #[serde(default = "default_task_unscoped")]
    pub task_unscoped: String,
    /// Clip note bodies to this many chars (`meta.truncated`); unset = whole body.
    #[serde(default)]
    pub read_max_chars: Option<usize>,
    /// Extra vault-relative prefixes the task scan skips (on top of the built-in scan set).
    #[serde(default)]
    pub task_exclude: Vec<String>,
}

fn default_true() -> bool {
    true
}
fn default_direction() -> String {
    "both".into()
}
fn default_task_unscoped() -> String {
    "summary".into()
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            per_doc: true,
            neighbors_direction: default_direction(),
            task_unscoped: default_task_unscoped(),
            read_max_chars: None,
            task_exclude: Vec::new(),
        }
    }
}

impl AgentConfig {
    pub fn task_unscoped_full(&self) -> bool {
        self.task_unscoped.eq_ignore_ascii_case("full")
    }
    /// Validated direction; anything odd falls back to `both`.
    pub fn direction(&self) -> &str {
        match self.neighbors_direction.as_str() {
            d @ ("out" | "in" | "both") => d,
            _ => "both",
        }
    }
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
        assert_eq!(c.agent, AgentConfig::default());
        assert!(c.agent.per_doc);
        assert_eq!(c.agent.direction(), "both");
        assert!(!c.agent.task_unscoped_full());
        assert_eq!(c.agent.read_max_chars, None);
    }

    /// N14: `[agent]` keys, partial tables keep the other defaults.
    #[test]
    fn agent_section() {
        let c: Config = toml::from_str(
            "[agent]\nper_doc = false\nneighbors_direction = \"out\"\ntask_unscoped = \"full\"\nread_max_chars = 4000\n",
        )
        .unwrap();
        assert!(!c.agent.per_doc);
        assert_eq!(c.agent.direction(), "out");
        assert!(c.agent.task_unscoped_full());
        assert_eq!(c.agent.read_max_chars, Some(4000));
        let c: Config = toml::from_str("[agent]\nneighbors_direction = \"sideways\"\n").unwrap();
        assert!(c.agent.per_doc);
        assert_eq!(c.agent.direction(), "both");
    }

    /// N21 / N22: `[theme]` and `task_exclude` parse; both default to empty.
    #[test]
    fn theme_and_task_exclude() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.theme, ThemeConfig::default());
        assert!(c.agent.task_exclude.is_empty());
        let c: Config = toml::from_str(
            "[agent]\ntask_exclude = [\"assets/\", \"Archmagus-Stack/Sovereign-Bootcamp/\"]\n[theme]\nname = \"parchment\"\n[theme.custom]\ngold = \"#FFD700\"\n",
        )
        .unwrap();
        assert_eq!(c.theme.name.as_deref(), Some("parchment"));
        assert_eq!(c.theme.custom.get("gold").map(String::as_str), Some("#FFD700"));
        assert_eq!(c.agent.task_exclude, ["assets/", "Archmagus-Stack/Sovereign-Bootcamp/"]);
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
