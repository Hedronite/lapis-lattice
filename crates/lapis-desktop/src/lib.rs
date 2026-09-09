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
    use gpui::{
        App, Application, Bounds, Context, SharedString, TitlebarOptions, Window, WindowBounds,
        WindowOptions, div, prelude::*, px, rgb, size,
    };

    /// Brand palette, same values the TUI asserts.
    const BLUE: u32 = 0x1F_2D68;
    const CREAM: u32 = 0xF3_E9D2;
    const REGENT: u32 = 0x80_9DAF;

    struct Root {
        title: SharedString,
        vault: SharedString,
        seed: SharedString,
    }

    impl Render for Root {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .size_full()
                .bg(rgb(BLUE))
                .text_color(rgb(CREAM))
                .p_4()
                .child(div().text_xl().child(self.title.clone()))
                .child(div().text_color(rgb(REGENT)).child(self.vault.clone()))
                .child(div().text_color(rgb(REGENT)).child(self.seed.clone()))
        }
    }

    pub fn open(opts: Options) -> Result<(), DesktopError> {
        Application::new().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1100.0), px(720.0)), cx);
            let title: SharedString = opts.title.clone().into();
            let vault: SharedString = opts.vault_root.display().to_string().into();
            let seed: SharedString =
                opts.seed.clone().unwrap_or_else(|| "no note selected".to_string()).into();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.clone()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Root { title, vault, seed }),
            );
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
