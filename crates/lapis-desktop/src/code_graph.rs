//! Phantom code-graph overlay.
//!
//! `Options::phantom_binary == None` is an empty board and the line
//! [`PHANTOM_NOT_FOUND`]. Nothing is started. A `Some` path is kept for a
//! later step and is not executed here.

use std::path::Path;

use crate::Options;

/// Visible line on the empty code-graph overlay.
pub const PHANTOM_NOT_FOUND: &str = "Phantom not found";

/// What the desktop draws for the code graph in this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeGraphOverlay {
    /// No binary. Empty board, [`PHANTOM_NOT_FOUND`], no process.
    Absent,
    /// A binary path was supplied. This step does not run it.
    Present,
}

impl CodeGraphOverlay {
    pub fn from_options(opts: &Options) -> Self {
        Self::from_binary(opts.phantom_binary.as_deref())
    }

    pub fn from_binary(binary: Option<&Path>) -> Self {
        match binary {
            None => Self::Absent,
            Some(_) => Self::Present,
        }
    }

    pub fn is_empty(self) -> bool {
        matches!(self, Self::Absent)
    }

    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::Absent => Some(PHANTOM_NOT_FOUND),
            Self::Present => None,
        }
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
        }
    }

    #[test]
    fn none_is_an_empty_overlay() {
        let overlay = CodeGraphOverlay::from_options(&opts(None));
        assert!(overlay.is_empty());
        assert_eq!(overlay.message(), Some("Phantom not found"));
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
    fn the_desktop_shows_the_absent_line_and_not_a_dialog() {
        let render = include_str!("workspace/render.rs");
        assert!(render.contains("code-graph-overlay"));
        assert!(render.contains("PHANTOM_NOT_FOUND"));
        assert!(!render.contains("code-graph-dialog"));
    }
}
