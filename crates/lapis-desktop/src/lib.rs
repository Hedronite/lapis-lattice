//! Lapis desktop (N23–N26).
//!
//! * [`html`]: an HTML **renderer** — HTML text → a block/inline layout tree
//!   → plain text or draw list. Not a browser, not a webview, no JS, no CSS
//!   engine, no network. It exists so notes and captured pages render inside
//!   the app's own surface.
//! * [`scene`]: the graph-canvas **data** layer — nodes, edges, positions.
//!   [`scene::draw`] is the paint list the window consumes.
//! * [`sim`]: the 2D force tick (center, repel, link, distance) that lays out
//!   the whole-vault snapshot. The default canvas positions come from here;
//!   the hop rings in [`scene::build`] are a debug view.
//! * [`view`]: the camera (pan, zoom), hit testing, hover neighbourhoods and
//!   the paint list of rim-terminated strokes and discs.
//! * [`gitnexus`]: optional sidecar client. No vendored code; no-op when there
//!   is no `.gitnexus` in the vault.
//! * [`run`]: opens the GPUI window when built with the `gpui` feature, else
//!   returns [`DesktopError::NotBuilt`]. Window code lives in `window.rs`.

pub mod gitnexus;
pub mod graph_data;
pub mod html;
pub mod live;
mod motion;
#[cfg(feature = "gpui")]
mod pdf_reader;
pub mod scene;
pub mod services;
pub mod session;
pub mod sim;
pub mod view;
pub mod vim;

#[cfg(feature = "gpui")]
mod window;

#[cfg(feature = "gpui")]
mod workspace;

pub fn run_workspace(
    opts: Options,
    services: std::sync::Arc<dyn services::WorkspaceServices>,
) -> Result<(), DesktopError> {
    #[cfg(feature = "gpui")]
    {
        workspace::open(opts, services)
    }
    #[cfg(not(feature = "gpui"))]
    {
        let _ = (opts, services);
        Err(DesktopError::NotBuilt(NOT_BUILT.into()))
    }
}

use std::path::PathBuf;

use serde::Serialize;

#[derive(Debug, Clone)]
pub struct Options {
    pub vault_root: PathBuf,
    pub lattice_url: String,
    /// The active note (`--path`). It marks a seed and gives local mode
    /// somewhere to start; it does **not** scope the graph. With or without
    /// it, the window opens on the whole indexed vault.
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

    #[test]
    fn window_paints_shipped_draw_on_gpui_omarchy() {
        let src = format!("{}{}", include_str!("window.rs"), include_str!("window/lifecycle.rs"));
        assert!(src.contains("key_action, paint"), "window consumes the view paint list");
        assert!(src.contains("gpui_omarchy::init"), "window is gpui-omarchy, not Zed-gpui");
        assert!(src.contains("Prim::Stroke"), "edges are strokes, not dot runs");
        assert!(src.contains("push_triangle"), "hairlines are geometry, not dotted quads");
        assert!(src.contains("fn dashes"), "a dangling link is drawn dashed");
        assert!(src.contains("on_scroll_wheel"), "wheel zooms");
        assert!(src.contains("on_key_down"), "+/- zoom and arrows pan");
        assert!(src.contains("pan_by"), "drag on empty space pans the camera");
        assert!(src.contains("open_note"), "a short press still opens the note");
        assert!(src.contains("self.pins.insert"), "a dragged node pins where it is dropped");
        assert!(src.contains("f-unpin"), "pins can be released");
        assert!(src.contains("highlight"), "hover lights the neighbourhood");
        assert!(src.contains("show_dangling"), "dangling filter");
        assert!(src.contains("f-domain"), "domain filter");
        assert!(src.contains("graph_snapshot()"), "default layout is the whole-vault snapshot");
        assert!(src.contains("layout: Layout::Global"), "global is the opening mode, not an ego ring");
        assert!(src.contains("graph_data::within"), "local mode cuts the same snapshot");
        assert!(src.contains("f-layout") && src.contains("f-depth"), "layout and depth are reachable");
        assert!(src.contains("f-existing"), "existing-only filter");
        assert!(src.contains("f-orphans"), "orphans toggle");
        assert!(src.contains("f-query"), "query filter");
        assert!(src.contains("f-group") && src.contains("group_hsla"), "groups recolour by query");
        assert!(src.contains("request_animation_frame"), "the sim keeps running after the first frame");
        assert!(src.contains("LAPIS_GRAPH_DEBUG"), "the fps overlay is measurable, not a feeling");
        assert!(src.contains("camera.fit"), "reset frames the whole graph");
        assert!(src.contains("theme.background"), "the void is the Omarchy background");
        assert!(src.contains("f-theme"), "the live theme name is on screen");
        // A label is drawn in exactly the box the placer reserved for it.
        assert!(src.contains("view::label_width_px"), "the drawn width is the reserved width");
        assert!(src.contains("line_height(px(LABEL_H))"), "the line box is pinned, not gpui's default");
        assert!(src.contains("overflow_hidden"), "a label cannot grow past its box");
        assert!(src.contains("view::label_box"), "the debug overlay draws the reserved boxes");
        assert!(src.contains("is_running()"), "a pointer event does not book a second render");
        assert!(src.contains("FRAME_BUDGET_MS"), "a late frame skips physics rather than compounding");
        assert!(src.contains("board_px"), "the board reports its real size back to the camera");
        assert!(src.contains(".w_full()"), "the board fills the window, so no node is laid out off it");
        // Every colour on the board comes from the live Omarchy palette, so a
        // theme swap restyles the graph with no restart. The only literal is a
        // group's opt-in hex.
        for role in ["theme.border", "theme.foreground", "theme.accent", "theme.danger"] {
            assert!(src.contains(role), "{role} is read from cx.omarchy()");
        }
        // The needle is assembled at runtime so this assertion does not match
        // itself in the source it is reading.
        let literal_colour = format!("gpui{}rgb(", "_kit::");
        assert_eq!(
            src.matches(literal_colour.as_str()).count(),
            1,
            "the only literal colour in the window is a group's opt-in hex"
        );
        // The chrome's mode string comes from `Layout`, whose global variant
        // reads "global". Nothing in the default view can say hop-2 unless the
        // operator has cycled to the rings debug layout.
        assert!(src.contains("Layout::Global => \"global\""), "the default chrome says global");
        assert!(src.contains("format!(\"local d{}\""), "local mode shows its depth");
        let toml = include_str!("../Cargo.toml");
        assert!(toml.contains("gpui-omarchy"));
        assert!(!toml.contains("gpui = \"0.2"));
    }
}
