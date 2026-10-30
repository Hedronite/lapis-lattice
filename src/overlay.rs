//! Per-vault operator overlay: `.lapis/vault.json`.
//!
//! Inbox / quick / archive / trash are overlays on existing directories,
//! never a rewrite of the vault tree. The types and the read live in
//! `lapis-notes`, which picks the inbox bucket for a create.

use std::path::Path;

use crate::error::Result;

pub use lapis_notes::{OVERLAY_REL, Overlay, OverlaySource};

pub fn load(root: &Path) -> Result<(Overlay, OverlaySource)> {
    Ok(lapis_notes::load_overlay(root)?)
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
        let o: Overlay = serde_json::from_str(r#"{"buckets":{"inbox":"Inbox"},"operator":"Ops"}"#).unwrap();
        assert_eq!(o.buckets.inbox, "Inbox");
        assert_eq!(o.buckets.quick, "quick");
        assert_eq!(o.operator.as_deref(), Some("Ops"));
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
