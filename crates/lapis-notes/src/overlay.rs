//! Per-vault operator overlay: `.lapis/vault.json`, the slice the inbox needs.
//!
//! Inbox / quick / archive / trash are overlays on existing directories,
//! never a rewrite of the vault tree.
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
pub const OVERLAY_REL: &str = ".lapis/vault.json";
/// Overlay buckets, defaults included. The inbox bucket is what `create`
/// lands in without an explicit `--path`.
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
/// Document kinds the vault accepts.
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
/// The per-vault overlay, partially written files included.
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
pub fn load_overlay(root: &Path) -> Result<(Overlay, OverlaySource)> {
    let path = root.join(OVERLAY_REL);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let o: Overlay = serde_json::from_str(&text)
                .map_err(|e| crate::Error::Usage(format!("overlay {}: {e}", path.display())))?;
            Ok((o, OverlaySource::File))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((Overlay::default(), OverlaySource::Default))
        }
        Err(e) => Err(crate::Error::Internal(format!("overlay {}: {e}", path.display()))),
    }
}
/// Read a vault's inbox bucket without an explicit `--path` create target.
///
/// Kept alongside [`load_overlay`] because byte-parity depends on the same
/// defaults: no overlay means `inbox`.
pub fn inbox_bucket(root: &Path) -> Result<String> {
    Ok(load_overlay(root)?.0.buckets.inbox)
}
