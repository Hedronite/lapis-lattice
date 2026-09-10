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
#[cfg(feature = "gpui")]
pub mod graph_data;
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
    use std::path::PathBuf;

    use gpui::{Bounds, Corners, Edges, Hsla, PaintQuad, Pixels, Point, Size, canvas, px};
    use gpui_kit::{
        AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
        StatefulInteractiveElement, Styled, Window, WindowOptions, div,
    };
    use gpui_omarchy::{ActiveTheme, panel};

    use super::scene::{Prim, Scene};
    use super::{DesktopError, Options, graph_data};

    // The board is a fixed size so the painted edges and the positioned nodes
    // can share one mapping. If the canvas mapped by its live bounds while the
    // nodes used constants, the lines would not meet the discs.
    const BOARD_W: f32 = 680.0;
    const BOARD_H: f32 = 460.0;
    /// Scene units (roughly -2..2) to pixels.
    const UNIT: f32 = 120.0;

    /// Scene coordinates to an offset inside the board.
    fn map(x: f32, y: f32) -> (f32, f32) {
        (BOARD_W / 2.0 + x * UNIT, BOARD_H / 2.0 + y * UNIT)
    }

    struct Root {
        title: String,
        vault: PathBuf,
        seed: String,
        scene: Scene,
        error: Option<String>,
        /// Node the operator clicked, and the head of that note.
        selected: Option<String>,
        peek: Option<String>,
        /// Filters (Gate C): dangling on/off, and one domain at a time.
        show_dangling: bool,
        domain: Option<String>,
        hops: u32,
    }

    impl Root {
        fn reload(&mut self) {
            match graph_data::scene_for(&self.vault, &self.seed, self.hops, !self.show_dangling) {
                Ok(s) => {
                    self.scene = s;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }

        /// Domains present in the current scene, for the filter to cycle.
        fn domains(&self) -> Vec<String> {
            let mut d: Vec<String> = self
                .scene
                .nodes
                .iter()
                .filter_map(|n| n.id.split('/').next().filter(|s| *s != n.id).map(str::to_string))
                .collect();
            d.sort();
            d.dedup();
            d
        }

        /// Gate C filters: one domain at a time, and dangling on or off.
        fn passes(&self, id: &str, dangling: bool) -> bool {
            if dangling && !self.show_dangling {
                return false;
            }
            match &self.domain {
                None => true,
                // a dangling node has no path, so it belongs to no domain
                Some(d) => id.split('/').next().filter(|s| *s != id) == Some(d.as_str()),
            }
        }
    }

    fn dot(at: Point<Pixels>, r: f32, color: Hsla) -> PaintQuad {
        let d = px(r * 2.0);
        PaintQuad {
            bounds: Bounds {
                origin: Point { x: at.x - px(r), y: at.y - px(r) },
                size: Size { width: d, height: d },
            },
            corner_radii: Corners::all(px(r)),
            background: color.into(),
            border_widths: Edges::all(px(0.0)),
            border_color: color,
            border_style: Default::default(),
        }
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.omarchy();
            let (edge, node, seed_c, dangling_c, label_c) =
                (theme.border, theme.foreground, theme.accent, theme.danger, theme.secondary);
            let prims = super::scene::draw(&self.scene);
            let scene_nodes = self.scene.nodes.clone();
            // Edges are drawn only between nodes that survive the filters.
            let kept: std::collections::HashSet<String> =
                scene_nodes.iter().filter(|n| self.passes(&n.id, n.dangling)).map(|n| n.id.clone()).collect();
            let edge_ok: Vec<bool> = self
                .scene
                .edges
                .iter()
                .map(|e| {
                    kept.contains(&self.scene.nodes[e.from].id) && kept.contains(&self.scene.nodes[e.to].id)
                })
                .collect();

            // Edges are painted: no element can draw a line at an arbitrary
            // angle, so they are laid down as a run of dots along each segment —
            // sparse for a dangling link, which is what "dashed" means here.
            let painted = canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    let mut line_i = 0usize;
                    for p in &prims {
                        if let Prim::Line { x0, y0, x1, y1, dashed } = p {
                            let keep = edge_ok.get(line_i).copied().unwrap_or(true);
                            line_i += 1;
                            if !keep {
                                continue;
                            }
                            let (ax, ay) = map(*x0, *y0);
                            let (bx, by) = map(*x1, *y1);
                            let a = Point { x: bounds.origin.x + px(ax), y: bounds.origin.y + px(ay) };
                            let b = Point { x: bounds.origin.x + px(bx), y: bounds.origin.y + px(by) };
                            let steps = if *dashed { 9 } else { 28 };
                            for i in 0..=steps {
                                let t = i as f32 / steps as f32;
                                let at = Point { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t };
                                window.paint_quad(dot(at, 1.2, edge));
                            }
                        }
                    }
                },
            )
            .absolute()
            .size_full();

            let mut board = div().relative().w(px(BOARD_W)).h(px(BOARD_H)).child(painted);

            for n in scene_nodes {
                if !self.passes(&n.id, n.dangling) {
                    continue;
                }
                let id = n.id.clone();
                let label = n.label.clone();
                let is_seed = n.is_seed;
                let dangling = n.dangling;
                let selected = self.selected.as_deref() == Some(id.as_str());
                let r = if is_seed { 11.0 } else { 7.0 };
                let colour = if dangling {
                    dangling_c
                } else if is_seed {
                    seed_c
                } else {
                    node
                };
                // Nodes are elements rather than paint so they can carry a label
                // and a click without hit-testing pixels by hand.
                let mut knob = div()
                    .id(gpui_kit::SharedString::from(id.clone()))
                    .w(px(r * 2.0))
                    .h(px(r * 2.0))
                    .rounded_full()
                    .bg(colour);
                if selected {
                    knob = knob.border_2().border_color(seed_c);
                }
                let (mx, my) = map(n.x, n.y);
                board = board.child(
                    div()
                        .absolute()
                        .left(px(mx - r))
                        .top(px(my - r))
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(knob.on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(id.clone());
                            this.peek = if id.starts_with("dangling:") {
                                Some(format!("{id} — this link resolves to nothing"))
                            } else {
                                Some(graph_data::peek(&this.vault, &id, 600).unwrap_or_else(|e| e))
                            };
                            cx.notify();
                        })))
                        .child(div().text_xs().text_color(label_c).child(label)),
                );
            }

            let dangling_label = format!("dangling: {}", if self.show_dangling { "shown" } else { "hidden" });
            let domain_label = format!("domain: {}", self.domain.clone().unwrap_or_else(|| "all".into()));
            let hop_label = format!("hop-{}", self.hops);
            let controls = div()
                .flex()
                .gap_2()
                .child(div().id("f-dangling").text_sm().text_color(label_c).child(dangling_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_dangling = !this.show_dangling;
                        this.reload();
                        cx.notify();
                    }),
                ))
                .child(div().id("f-domain").text_sm().text_color(label_c).child(domain_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        // cycle: all -> each domain -> all
                        let all = this.domains();
                        this.domain = match &this.domain {
                            None => all.first().cloned(),
                            Some(cur) => {
                                let i = all.iter().position(|d| d == cur).map(|i| i + 1);
                                i.and_then(|i| all.get(i).cloned())
                            }
                        };
                        cx.notify();
                    }),
                ))
                .child(div().id("f-hops").text_sm().text_color(label_c).child(hop_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.hops = if this.hops == 2 { 1 } else { 2 };
                        this.reload();
                        cx.notify();
                    }),
                ));

            panel(self.title.as_str(), cx)
                .size_full()
                .bg(theme.background)
                .child(controls)
                .child(board)
                .child(div().text_sm().text_color(label_c).child(match (&self.error, &self.peek) {
                    (Some(e), _) => e.clone(),
                    (None, Some(p)) => p.clone(),
                    (None, None) => format!(
                        "{}  ·  {} nodes, {} links  ·  hop-{}",
                        self.seed,
                        self.scene.nodes.len(),
                        self.scene.edges.len(),
                        self.hops
                    ),
                }))
        }
    }

    pub fn open(opts: Options) -> Result<(), DesktopError> {
        let seed = opts.seed.clone().unwrap_or_else(|| "Welcome.md".to_string());
        let vault = opts.vault_root.clone();
        let title = opts.title.clone();
        let (scene, error) = match graph_data::scene_for(&vault, &seed, 2, true) {
            Ok(s) => (s, None),
            Err(e) => (Scene::default(), Some(e)),
        };
        gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
            gpui_omarchy::init(cx);
            let opened = cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| Root {
                    title: title.clone(),
                    vault: vault.clone(),
                    seed: seed.clone(),
                    scene: scene.clone(),
                    error: error.clone(),
                    selected: None,
                    peek: None,
                    show_dangling: true,
                    domain: None,
                    hops: 2,
                })
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
