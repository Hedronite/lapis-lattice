//! Per-vault operator overlay: `.lapis/vault.json`.
//!
//! Inbox / quick / archive / trash are overlays on existing directories,
//! never a rewrite of the vault tree.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{LapisError, Result};

pub const OVERLAY_REL: &str = ".lapis/vault.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Buckets {
    #[serde(default = "d_inbox")]
    pub inbox: String,
    #[serde(default = "d_quick")]
    pub quick: String,
    #[serde(default = "d_archive")]
    pub archive: String,
    #[serde(default = "d_trash")]
    pub trash: String,
}

fn d_inbox() -> String {
    "inbox".into()
}
fn d_quick() -> String {
    "quick".into()
}
fn d_archive() -> String {
    "archive".into()
}
fn d_trash() -> String {
    ".lapis/trash".into()
}

impl Default for Buckets {
    fn default() -> Self {
        Self { inbox: d_inbox(), quick: d_quick(), archive: d_archive(), trash: d_trash() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Corpus {
    #[serde(default = "t")]
    pub markdown: bool,
    #[serde(default = "t")]
    pub pdf: bool,
    #[serde(default = "t")]
    pub html: bool,
    #[serde(default)]
    pub source: bool,
}

fn t() -> bool {
    true
}

impl Default for Corpus {
    fn default() -> Self {
        Self { markdown: true, pdf: true, html: true, source: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Overlay {
    #[serde(default)]
    pub buckets: Buckets,
    /// Directory names treated as attachments. Default empty: `assets/` is
    /// not reserved (LAP-14).
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub corpus: Corpus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    /// Kept as raw JSON so a later slice can own its schema without this
    /// parser rejecting an overlay it does not yet understand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub periodic: Option<serde_json::Value>,
}

/// Where the overlay came from, reported in `vault info`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OverlaySource {
    File,
    Default,
}

pub fn load(root: &Path) -> Result<(Overlay, OverlaySource)> {
    let path = root.join(OVERLAY_REL);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let o: Overlay = serde_json::from_str(&text)
                .map_err(|e| LapisError::Usage(format!("overlay {}: {e}", path.display())))?;
            Ok((o, OverlaySource::File))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((Overlay::default(), OverlaySource::Default))
        }
        Err(e) => Err(LapisError::Internal(format!("overlay {}: {e}", path.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_schema() {
        let o = Overlay::default();
        assert_eq!(o.buckets.inbox, "inbox");
        assert_eq!(o.buckets.archive, "archive");
        assert_eq!(o.buckets.trash, ".lapis/trash");
        assert!(o.attachments.is_empty(), "assets/ must not be reserved by default");
        assert!(!o.corpus.source);
    }

    #[test]
    fn partial_overlay_fills_defaults() {
        let o: Overlay = serde_json::from_str(r#"{"buckets":{"inbox":"Inbox"},"operator":"Halo"}"#).unwrap();
        assert_eq!(o.buckets.inbox, "Inbox");
        assert_eq!(o.buckets.quick, "quick");
        assert_eq!(o.operator.as_deref(), Some("Halo"));
    }

    #[test]
    fn missing_file_is_default() {
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lapis-overlay-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (o, src) = load(&dir).unwrap();
        assert_eq!(src, OverlaySource::Default);
        assert_eq!(o, Overlay::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
