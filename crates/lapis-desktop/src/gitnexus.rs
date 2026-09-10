//! GitNexus sidecar client (N26). Nothing vendored: GitNexus, when present,
//! is a separate process the vault points at via `.gitnexus` (a JSON file or a
//! directory holding `config.json`). Without it every call is a no-op that
//! returns an empty overlay, so the app never depends on it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct SidecarConfig {
    /// Base URL of the running sidecar, e.g. `http://127.0.0.1:7477`.
    #[serde(default)]
    pub url: Option<String>,
    /// Optional repo root the sidecar indexes (defaults to the vault).
    #[serde(default)]
    pub repo: Option<String>,
}

/// Per-note overlay the sidecar can contribute: commits and branches that touched it.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Overlay {
    pub path: String,
    pub commits: Vec<String>,
    pub branches: Vec<String>,
    /// `false` when no sidecar is configured (an empty overlay, not an error).
    pub available: bool,
}

#[derive(Debug, Clone)]
pub enum Sidecar {
    Missing,
    Configured { path: PathBuf, config: SidecarConfig },
}

impl Sidecar {
    /// Look for `.gitnexus` (file) or `.gitnexus/config.json` under `root`.
    pub fn discover(root: &Path) -> Sidecar {
        let candidates = [root.join(".gitnexus"), root.join(".gitnexus").join("config.json")];
        for c in candidates {
            if c.is_file() {
                let config = std::fs::read_to_string(&c)
                    .ok()
                    .and_then(|t| serde_json::from_str::<SidecarConfig>(&t).ok())
                    .unwrap_or_default();
                return Sidecar::Configured { path: c, config };
            }
        }
        Sidecar::Missing
    }

    pub fn is_configured(&self) -> bool {
        matches!(self, Sidecar::Configured { .. })
    }

    pub fn config_path(&self) -> Option<&Path> {
        match self {
            Sidecar::Configured { path, .. } => Some(path),
            Sidecar::Missing => None,
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Sidecar::Configured { config, .. } => config.url.as_deref(),
            Sidecar::Missing => None,
        }
    }

    /// Overlay for one note. Missing sidecar → `Ok(empty)`. A configured
    /// sidecar is contacted lazily by the window; this data layer only
    /// answers what it can without a network round-trip.
    pub fn overlay(&self, rel: &str) -> Result<Overlay, String> {
        Ok(Overlay {
            path: rel.to_string(),
            commits: vec![],
            branches: vec![],
            available: self.is_configured(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("lapis-gitnexus-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_sidecar_is_ok_empty_overlay() {
        let d = tmp();
        let s = Sidecar::discover(&d);
        assert!(!s.is_configured() && s.config_path().is_none() && s.url().is_none());
        let o = s.overlay("notes/SPEC.md").unwrap();
        assert_eq!(
            o,
            Overlay { path: "notes/SPEC.md".into(), commits: vec![], branches: vec![], available: false }
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn discovers_file_or_dir_config() {
        let d = tmp();
        std::fs::write(d.join(".gitnexus"), r#"{"url":"http://127.0.0.1:7477"}"#).unwrap();
        let s = Sidecar::discover(&d);
        assert!(s.is_configured());
        assert_eq!(s.url(), Some("http://127.0.0.1:7477"));
        assert!(s.overlay("a.md").unwrap().available);
        std::fs::remove_file(d.join(".gitnexus")).unwrap();
        std::fs::create_dir_all(d.join(".gitnexus")).unwrap();
        std::fs::write(d.join(".gitnexus/config.json"), "not json").unwrap();
        let s = Sidecar::discover(&d);
        assert!(s.is_configured(), "present but unparsable still counts as configured");
        assert_eq!(s.url(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
