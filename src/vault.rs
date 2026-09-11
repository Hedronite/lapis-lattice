//! Vault root resolution. Precedence: `--vault`, `$LAPIS_VAULT`, config `vault`.
//! There is no implicit default directory: a missing vault is a usage error.

use std::path::PathBuf;

use crate::config::{Config, expand_tilde};
use crate::error::{LapisError, Result};

/// The remedy has to be something that actually leaves a vault configured.
/// The old wording offered `lapis init` for a vault that already existed, and
/// init used to record nothing, so following it landed you back here.
pub const NO_VAULT: &str = "no vault configured. Pass --vault <path>, set $LAPIS_VAULT, or run \
`lapis init <path>` to create a vault and record it in the config.";

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
        return Err(LapisError::Usage(NO_VAULT.into()));
    };
    let root = expand_tilde(&raw);
    if !root.is_dir() {
        return Err(LapisError::Usage(format!(
            "vault is not a directory: {} (from {source}). Run `lapis init {raw}` to create one.",
            root.display()
        )));
    }
    // Canonicalize so `vault info` reports a stable absolute root; the
    // per-note escape check in `notes` is lexical and does not need this.
    let root = root.canonicalize().unwrap_or(root);
    Ok(Vault { root, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn missing_config_is_usage_not_a_hidden_home_path() {
        let e = resolve(None, &Config::default()).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.message().contains("lapis init"), "{}", e.message());
        assert!(!e.message().contains("Obsidian"));
        // The remedy has to change what the resolver reads. Offering `init` on
        // a path that already exists, back when init recorded nothing, sent the
        // operator straight back to this same error.
        assert!(
            e.message().contains("record it in the config"),
            "the suggested command must leave a vault configured: {}",
            e.message()
        );
    }

    #[test]
    fn config_vault_resolves_when_no_flag_or_env_is_given() {
        let d = std::env::temp_dir().join(format!(
            "lapis-vault-cfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        let cfg = Config { vault: Some(d.display().to_string()), ..Config::default() };
        let v = resolve(None, &cfg).unwrap();
        assert_eq!(v.source, "config");
        assert_eq!(v.root, d.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn flag_missing_dir_suggests_init() {
        let e = resolve(Some("/no/such/lapis-vault-dir"), &Config::default()).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.message().contains("lapis init"));
    }
}
