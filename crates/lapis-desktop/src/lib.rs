//! Lapis desktop (N23–N26).
//!
//! * [`html`]: an HTML **renderer** — HTML text → a block/inline layout tree
//!   → plain text or draw list. Not a browser, not a webview, no JS, no CSS
//!   engine, no network. It exists so notes and captured pages render inside
//!   the app's own surface.
//! * [`scene`]: the graph-canvas **data** layer — `/graph/ego` JSON → nodes,
//!   edges, positions. The GPU draw is a stub until the window lands.
//! * [`gitnexus`]: optional sidecar client. No vendored code; no-op when there
//!   is no `.gitnexus` in the vault.
//! * [`run`]: opens the GPUI window when built with the `gpui` feature, else
//!   returns [`DesktopError::NotBuilt`].

pub mod gitnexus;
pub mod html;
pub mod scene;

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone)]
pub struct Options {
    pub vault_root: PathBuf,
    pub lattice_url: String,
    /// Note to open / seed the canvas with.
    pub seed: Option<String>,
    pub title: String,
}

#[derive(Debug)]
pub enum DesktopError {
    /// The binary was built without the `desktop` feature.
    NotBuilt(String),
    Runtime(String),
}

impl std::fmt::Display for DesktopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DesktopError::NotBuilt(m) | DesktopError::Runtime(m) => f.write_str(m),
        }
    }
}
impl std::error::Error for DesktopError {}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub gpui_available: bool,
    /// Path of the GitNexus sidecar config when present.
    pub gitnexus: Option<String>,
    pub seed: Option<String>,
}

/// What `lapis desktop --check` reports.
pub fn preflight(opts: &Options) -> Preflight {
    Preflight {
        gpui_available: cfg!(feature = "gpui"),
        gitnexus: gitnexus::Sidecar::discover(&opts.vault_root)
            .config_path()
            .map(|p| p.display().to_string()),
        seed: opts.seed.clone(),
    }
}

pub const NOT_BUILT: &str = "lapis desktop: this binary was built without the `desktop` feature \
(GPUI). Rebuild with `--features desktop`, or use `lapis tui`.";

#[cfg(not(feature = "gpui"))]
pub fn run(_opts: Options) -> Result<(), DesktopError> {
    Err(DesktopError::NotBuilt(NOT_BUILT.into()))
}

#[cfg(feature = "gpui")]
pub fn run(opts: Options) -> Result<(), DesktopError> {
    window::open(opts)
}

#[cfg(feature = "gpui")]
mod window {
    use super::{DesktopError, Options};
    use gpui_kit::{
        AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowOptions,
    };
    use gpui_omarchy::{ActiveTheme, panel};

    struct Root {
        title: String,
        vault: String,
        seed: String,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            panel(&self.title, cx)
                .size_full()
                .bg(cx.omarchy().background)
                .child(self.vault.clone())
                .child(self.seed.clone())
        }
    }

    pub fn open(opts: Options) -> Result<(), DesktopError> {
        let title = opts.title.clone();
        let vault = opts.vault_root.display().to_string();
        let seed = opts.seed.clone().unwrap_or_else(|| "no note selected".into());
        gpui_kit::application()
            .with_assets(gpui_kit::assets::Assets)
            .run(move |cx| {
                gpui_omarchy::init(cx);
                let opened = cx.open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| Root { title: title.clone(), vault: vault.clone(), seed: seed.clone() })
                });
                match opened {
                    Ok(_) => cx.activate(true),
                    Err(e) => eprintln!("lapis desktop: could not open a window: {e}"),
                }
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_built_is_a_clear_error() {
        let o = Options {
            vault_root: std::env::temp_dir(),
            lattice_url: "http://127.0.0.1:8080".into(),
            seed: None,
            title: "t".into(),
        };
        let p = preflight(&o);
        assert_eq!(p.gpui_available, cfg!(feature = "gpui"));
        assert!(p.gitnexus.is_none());
        if !cfg!(feature = "gpui") {
            match run(o) {
                Err(DesktopError::NotBuilt(m)) => assert!(m.contains("--features desktop")),
                other => panic!("expected NotBuilt, got {other:?}"),
            }
        }
    }
}
