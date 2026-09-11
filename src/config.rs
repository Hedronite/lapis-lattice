//! User config: `~/.config/lapis/config.toml`.
//!
//! Unknown sections are ignored so later slices can add theirs without
//! breaking this parser.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{LapisError, Result};

pub const DEFAULT_LATTICE_URL: &str = "http://127.0.0.1:8080";
pub const DEFAULT_TIMEOUT_MS: u64 = 8000;

#[derive(Debug, Default, Deserialize, Clone)]
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
    /// Operator HTTP API (`lapis api` / `/v1`).
    #[serde(default)]
    pub api: ApiConfig,
}

pub const DEFAULT_API_BIND: &str = "127.0.0.1";
pub const DEFAULT_API_PORT: u16 = 18765;

/// `[api]`: bind/port for the operator HTTP face. Distinct from lattice `:8080`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiConfig {
    #[serde(default = "default_api_bind")]
    pub bind: String,
    #[serde(default = "default_api_port")]
    pub port: u16,
}

fn default_api_bind() -> String {
    DEFAULT_API_BIND.to_string()
}
fn default_api_port() -> u16 {
    DEFAULT_API_PORT
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { bind: default_api_bind(), port: default_api_port() }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ThemeConfig {
    /// `omarchy` (default) follows `~/.local/state/omarchy/current/`;
    /// `lapis` pins the brand palettes. Off Omarchy, `omarchy` falls back to
    /// brand on its own, so the default is safe everywhere.
    #[serde(default = "default_theme_mode")]
    pub mode: String,
    /// `lapis` (default), `parchment`, `obsidian`. Only when `mode = "lapis"`.
    #[serde(default)]
    pub name: Option<String>,
    /// Role → `#RRGGBB`: blue, blue_deep, blue_soft, regent, cream, gold, copper, muted, ok, warn.
    #[serde(default)]
    pub custom: std::collections::BTreeMap<String, String>,
}

fn default_theme_mode() -> String {
    "omarchy".to_string()
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self { mode: default_theme_mode(), name: None, custom: Default::default() }
    }
}

impl ThemeConfig {
    /// True unless the operator pinned the brand palettes.
    pub fn is_omarchy(&self) -> bool {
        !self.mode.eq_ignore_ascii_case("lapis")
    }
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

#[derive(Debug, Default, Deserialize, Clone)]
pub struct OperatorConfig {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LatticeConfig {
    /// `embedded` (default) or `http`. Embedded needs no daemon.
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_url")]
    pub url: String,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_mode() -> String {
    "embedded".to_string()
}

impl LatticeConfig {
    /// True unless the operator explicitly asked for the HTTP lattice.
    pub fn is_embedded(&self) -> bool {
        !self.mode.eq_ignore_ascii_case("http")
    }
}

fn default_url() -> String {
    DEFAULT_LATTICE_URL.to_string()
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_MS
}

impl Default for LatticeConfig {
    fn default() -> Self {
        Self { mode: default_mode(), url: default_url(), timeout_ms: default_timeout() }
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

/// What [`remember_vault`] did, so the caller can say it out loud.
#[derive(Debug, Clone, PartialEq)]
pub enum Remembered {
    /// `vault` was written into the config file at this path.
    Wrote(PathBuf),
    /// The config already names a vault. An existing choice is never
    /// overwritten by an `init` of somewhere else.
    AlreadySet(String),
    /// There is no home or `XDG_CONFIG_HOME` to write into.
    NoConfigDir,
}

/// Record `vault` as the configured default, so the next bare `lapis` resolves.
///
/// Creating a vault and then leaving nothing behind that points at it is what
/// made `no vault configured` a loop: the error told you to run `lapis init`,
/// and running it changed nothing the resolver reads. This closes that.
///
/// It is not an implicit default: the operator named the path. An existing
/// `vault` key is left alone.
pub fn remember_vault(vault: &str) -> Result<Remembered> {
    let Some(path) = config_path() else {
        return Ok(Remembered::NoConfigDir);
    };
    remember_vault_at(&path, vault)
}

/// [`remember_vault`] against an explicit config path.
///
/// An existing file is edited rather than rewritten: the `vault` line is
/// inserted above the first table header so it lands in the top-level table,
/// and every other byte, comment and ordering included, is kept.
pub fn remember_vault_at(path: &Path, vault: &str) -> Result<Remembered> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(LapisError::Usage(format!("config {}: {e}", path.display()))),
    };

    if let Some(text) = &existing {
        let parsed: Config =
            toml::from_str(text).map_err(|e| LapisError::Usage(format!("config {}: {e}", path.display())))?;
        if let Some(v) = parsed.vault.filter(|v| !v.trim().is_empty()) {
            return Ok(Remembered::AlreadySet(v));
        }
    }

    let line = format!(
        "vault = {}
",
        toml_string(vault)
    );
    let next = match existing {
        None => format!(
            "# Written by `lapis init`.\n\
             # Remove this line to go back to naming a vault every time; both\n\
             # --vault and $LAPIS_VAULT still win over it.\n\
             {line}"
        ),
        Some(text) => insert_top_level(&text, &line),
    };

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, next)?;
    Ok(Remembered::Wrote(path.to_path_buf()))
}

/// Put `line` in the top-level table: above the first `[section]`, or at the
/// end when the file has none.
fn insert_top_level(text: &str, line: &str) -> String {
    match text.lines().position(|l| l.trim_start().starts_with('[')) {
        Some(i) => {
            let mut out: Vec<&str> = text.lines().collect();
            out.splice(i..i, [line.trim_end(), ""]);
            let mut joined = out.join("\n");
            joined.push('\n');
            joined
        }
        None => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line);
            out
        }
    }
}

/// A TOML basic string. Paths are the only thing we write, and a path may
/// contain a quote or a backslash on a bad day.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

    fn scratch(name: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-cfg-{name}-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("config.toml")
    }

    /// `init` has to leave something behind that the resolver reads, or
    /// "no vault configured" sends you to a command that changes nothing.
    #[test]
    fn remember_vault_writes_a_config_the_loader_reads_back() {
        let path = scratch("fresh");
        let wrote = remember_vault_at(&path, "/home/x/Notes").unwrap();
        assert_eq!(wrote, Remembered::Wrote(path.clone()));

        let text = std::fs::read_to_string(&path).unwrap();
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.vault.as_deref(), Some("/home/x/Notes"));
        assert!(text.starts_with('#'), "the file says who wrote it and how to undo it");

        // Running init again never overwrites a vault the operator already has.
        let again = remember_vault_at(&path, "/home/x/Somewhere-Else").unwrap();
        assert_eq!(again, Remembered::AlreadySet("/home/x/Notes".into()));
        let after: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after.vault.as_deref(), Some("/home/x/Notes"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// An existing config is edited, not rewritten: the new key has to land in
    /// the top-level table, above the first section, with everything else kept.
    #[test]
    fn remember_vault_preserves_an_existing_config() {
        let path = scratch("existing");
        let before = "# my notes\ntimeout_scratch = 1\n\n[lattice]\nmode = \"http\"\nurl = \"http://127.0.0.1:9999\"\n";
        std::fs::write(&path, before).unwrap();
        assert_eq!(remember_vault_at(&path, "/vaults/a").unwrap(), Remembered::Wrote(path.clone()));

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my notes"), "comments survive");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.vault.as_deref(), Some("/vaults/a"));
        assert_eq!(cfg.lattice.mode, "http", "the other sections are untouched");
        assert_eq!(cfg.lattice.url, "http://127.0.0.1:9999");
        assert!(
            text.find("vault = ").unwrap() < text.find("[lattice]").unwrap(),
            "the key is in the top-level table, not inside [lattice]"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn remember_vault_refuses_to_touch_a_malformed_config() {
        let path = scratch("broken");
        std::fs::write(&path, "this is not toml = = =\n").unwrap();
        let e = remember_vault_at(&path, "/vaults/a").unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "this is not toml = = =\n");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_path_with_a_quote_round_trips() {
        let path = scratch("quote");
        remember_vault_at(&path, r#"/vaults/od"d\path"#).unwrap();
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.vault.as_deref(), Some(r#"/vaults/od"d\path"#));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn defaults_when_sections_missing() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.lattice.url, DEFAULT_LATTICE_URL);
        assert_eq!(c.lattice.timeout_ms, DEFAULT_TIMEOUT_MS);
        assert_eq!(c.lattice.mode, "embedded", "embedded is the default backend");
        assert!(c.lattice.is_embedded());
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
        assert_eq!(c.theme.mode, "omarchy", "Omarchy-native is the default");
        assert!(c.theme.is_omarchy());
        assert!(c.agent.task_exclude.is_empty());
        let c: Config = toml::from_str(
            "[agent]\ntask_exclude = [\"assets/\", \"Archmagus-Stack/Sovereign-Bootcamp/\"]\n[theme]\nname = \"parchment\"\n[theme.custom]\ngold = \"#FFD700\"\n",
        )
        .unwrap();
        assert_eq!(c.theme.name.as_deref(), Some("parchment"));
        assert_eq!(c.theme.custom.get("gold").map(String::as_str), Some("#FFD700"));
        assert_eq!(c.agent.task_exclude, ["assets/", "Archmagus-Stack/Sovereign-Bootcamp/"]);
        // mode is independent of name: pinning brand palettes opts out of Omarchy
        let pinned: Config = toml::from_str("[theme]\nmode = \"lapis\"\n").unwrap();
        assert!(!pinned.theme.is_omarchy());
    }

    #[test]
    fn reads_lattice_section_and_ignores_unknown() {
        let c: Config = toml::from_str(
            "[lattice]\nurl = \"http://localhost:9999\"\ntimeout_ms = 250\n[vim]\nenabled = true\n",
        )
        .unwrap();
        assert_eq!(c.lattice.url, "http://localhost:9999");
        assert_eq!(c.lattice.timeout_ms, 250);
        let c: Config = toml::from_str("[lattice]\nmode = \"http\"\n").unwrap();
        assert!(!c.lattice.is_embedded());
    }

    #[test]
    fn tilde_expands_only_leading() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(expand_tilde("/a/~/b"), PathBuf::from("/a/~/b"));
    }
}
