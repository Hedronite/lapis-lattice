//! Phantom code-graph overlay.
//!
//! `Options::phantom_binary == None` is an empty board and the line
//! [`PHANTOM_NOT_FOUND`]. Nothing is started. Rows are text the caller already
//! capped. They are drawn and not opened.

use std::path::Path;

use crate::Options;

/// Visible line on the empty code-graph overlay.
pub const PHANTOM_NOT_FOUND: &str = "Phantom not found";

/// What the desktop draws for the code graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeGraphOverlay {
    absent: bool,
    rows: Vec<String>,
    truncated: bool,
}

impl CodeGraphOverlay {
    pub fn absent() -> Self {
        Self { absent: true, rows: Vec::new(), truncated: false }
    }

    pub fn from_options(opts: &Options) -> Self {
        Self {
            absent: opts.phantom_binary.is_none(),
            rows: opts.phantom_rows.clone(),
            truncated: opts.phantom_truncated,
        }
    }

    pub fn from_binary(binary: Option<&Path>) -> Self {
        Self { absent: binary.is_none(), rows: Vec::new(), truncated: false }
    }

    pub fn is_empty(&self) -> bool {
        self.absent
    }

    pub fn message(&self) -> Option<&'static str> {
        self.absent.then_some(PHANTOM_NOT_FOUND)
    }

    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn opts(binary: Option<PathBuf>) -> Options {
        Options {
            vault_root: std::env::temp_dir(),
            lattice_url: "http://127.0.0.1:8080".into(),
            seed: None,
            title: "t".into(),
            phantom_binary: binary,
            phantom_rows: Vec::new(),
            phantom_truncated: false,
        }
    }

    #[test]
    fn none_is_an_empty_overlay() {
        let overlay = CodeGraphOverlay::from_options(&opts(None));
        assert!(overlay.is_empty());
        assert_eq!(overlay.message(), Some("Phantom not found"));
        assert!(overlay.rows().is_empty());
        assert!(!overlay.truncated());
        assert_eq!(PHANTOM_NOT_FOUND, "Phantom not found");
    }

    #[test]
    fn some_is_not_run() {
        let missing = PathBuf::from("/no/such/phantom-binary");
        let overlay = CodeGraphOverlay::from_options(&opts(Some(missing)));
        assert!(!overlay.is_empty());
        assert_eq!(overlay.message(), None);
        let src = include_str!("code_graph.rs");
        let command = format!("{}{}", "Command", "::new");
        let process = format!("{}{}", "std::", "process");
        assert!(!src.contains(&command), "a supplied path is not executed");
        assert!(!src.contains(&process), "a supplied path is not executed");
    }

    #[test]
    fn capped_rows_are_text_and_not_opened() {
        let mut opts = opts(Some(PathBuf::from("/no/such/phantom-binary")));
        opts.phantom_rows = vec!["/tmp/not-opened".into(), "second row".into()];
        opts.phantom_truncated = true;
        let overlay = CodeGraphOverlay::from_options(&opts);
        assert_eq!(overlay.rows(), &["/tmp/not-opened".to_string(), "second row".to_string()]);
        assert!(overlay.truncated());
        let src = include_str!("code_graph.rs");
        let render = include_str!("workspace/render.rs");
        assert!(render.contains("code-graph-rows"));
        assert!(render.contains("code-graph-truncated"));
        let open = format!("{}{}", "File", "::open");
        let fs = format!("{}{}", "std::", "fs");
        let json = format!("{}{}", "serde_", "json");
        assert!(!src.contains(&open) && !render.contains(&open));
        assert!(!src.contains(&fs) && !render.contains(&fs));
        assert!(!src.contains(&json));
    }

    #[test]
    fn the_desktop_shows_the_absent_line_and_not_a_dialog() {
        let render = include_str!("workspace/render.rs");
        assert!(render.contains("code-graph-overlay"));
        assert!(render.contains("PHANTOM_NOT_FOUND"));
        assert!(!render.contains("code-graph-dialog"));
    }
}
