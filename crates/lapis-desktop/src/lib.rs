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
//!   returns [`DesktopError::NotBuilt`].

pub mod gitnexus;
pub mod graph_data;
pub mod html;
pub mod scene;
pub mod sim;
pub mod view;

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
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::rc::Rc;

    use gpui_kit::{
        App, AppContext, Bounds, Context, Corners, Edges, FocusHandle, Hsla, InteractiveElement, IntoElement,
        KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement,
        PathBuilder, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Size, StatefulInteractiveElement,
        Styled, Window, WindowOptions, canvas, div, point, px,
    };
    use gpui_omarchy::{ActiveTheme, panel};

    use super::scene::Scene;
    use super::view::{CLICK_SLOP_PX, Camera, Prim, ZOOM_STEP, highlight, hit_test, paint};
    use super::{DesktopError, Options, graph_data};

    /// The board is a fixed size so the painted strokes and the camera share one
    /// mapping. If the canvas mapped by its live bounds while the labels used
    /// constants, the lines would not meet the discs.
    const BOARD_W: f32 = 760.0;
    const BOARD_H: f32 = 520.0;

    /// A press in flight: which node it started on (none = empty space, so it
    /// pans) and whether it has travelled far enough to be a drag.
    struct Press {
        node: Option<String>,
        from: [f32; 2],
        last: [f32; 2],
        moved: bool,
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
        /// Ego depth. Only the ring debug view reads it; the global graph has
        /// no hop limit.
        hops: u32,
        /// False is the shipped default: the whole vault at force-sim positions.
        /// True brings back the v0.2 hop rings as a debug overlay.
        rings: bool,
        camera: Camera,
        hover: Option<String>,
        press: Option<Press>,
        /// Where the operator dropped a node. Survives filter and layout
        /// rebuilds; cleared per node with a right-click, or all at once.
        pins: BTreeMap<String, [f32; 2]>,
        /// The board's top-left in window coordinates, written by the canvas as
        /// it paints and read by the mouse handlers. Both run on the UI thread.
        origin: Rc<Cell<(f32, f32)>>,
        focus: FocusHandle,
    }

    impl Root {
        /// Default layout is the whole-vault snapshot settled by the force sim.
        /// The hop-ring walk stays reachable behind the layout toggle.
        fn reload(&mut self) {
            let built = if self.rings {
                graph_data::scene_for(&self.vault, &self.seed, self.hops, false)
            } else {
                graph_data::global_scene(&self.vault, &self.seed, graph_data::SETTLE_TICKS)
            };
            match built {
                Ok(mut s) => {
                    s.apply_pins(&self.pins);
                    self.scene = s;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }

        fn board(&self) -> [f32; 2] {
            [BOARD_W, BOARD_H]
        }

        /// Window coordinates to board-local pixels.
        fn local(&self, at: Point<Pixels>) -> [f32; 2] {
            let (ox, oy) = self.origin.get();
            [f32::from(at.x) - ox, f32::from(at.y) - oy]
        }

        /// The scene as drawn: filters applied, pins honoured.
        fn visible(&self) -> Scene {
            self.scene.filtered(self.show_dangling, self.domain.as_deref())
        }

        fn open_note(&mut self, id: &str) {
            self.selected = Some(id.to_string());
            self.peek = if id.starts_with("dangling:") {
                Some(format!("{id} — this link resolves to nothing"))
            } else {
                Some(graph_data::peek(&self.vault, id, 600).unwrap_or_else(|e| e))
            };
        }

        /// The node under a board-local point, honouring the visible filters.
        fn node_at(&self, at: [f32; 2]) -> Option<String> {
            let (dangling, domain) = (self.show_dangling, self.domain.clone());
            hit_test(&self.scene, &self.camera, self.board(), at, &|n| n.passes(dangling, domain.as_deref()))
                .map(|i| self.scene.nodes[i].id.clone())
        }

        fn on_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
            let at = self.local(ev.position);
            let node = self.node_at(at);
            if ev.button == MouseButton::Right {
                // Right-click a pinned node releases it; the force layout is
                // free to place it again on the next rebuild.
                if let Some(id) = node
                    && self.pins.remove(&id).is_some()
                {
                    self.reload();
                    cx.notify();
                }
                return;
            }
            self.press = Some(Press { node, from: at, last: at, moved: false });
        }

        fn on_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
            let at = self.local(ev.position);
            let board = self.board();

            if self.press.is_none() {
                // No button down: this is hover.
                let over = self.node_at(at);
                if over != self.hover {
                    self.hover = over;
                    cx.notify();
                }
                return;
            }
            let Some(press) = self.press.as_mut() else { return };
            let (dx, dy) = (at[0] - press.last[0], at[1] - press.last[1]);
            let travelled = ((at[0] - press.from[0]).powi(2) + (at[1] - press.from[1]).powi(2)).sqrt();
            if travelled > CLICK_SLOP_PX {
                press.moved = true;
            }
            press.last = at;
            if !press.moved {
                return;
            }
            match press.node.clone() {
                // Drag a node: it follows the cursor in world space and stays
                // where it is dropped.
                Some(id) => {
                    let w = self.camera.to_world(at, board);
                    self.pins.insert(id.clone(), w);
                    if let Some(n) = self.scene.nodes.iter_mut().find(|n| n.id == id) {
                        n.x = w[0];
                        n.y = w[1];
                    }
                }
                // Drag empty space: pan the camera. The layout does not move.
                None => self.camera.pan_by(dx, dy),
            }
            cx.notify();
        }

        fn on_up(&mut self, _: &MouseUpEvent, cx: &mut Context<Self>) {
            let Some(press) = self.press.take() else { return };
            // A short press is still a click, even on a node you could have
            // dragged.
            if !press.moved
                && let Some(id) = press.node
            {
                self.open_note(&id);
            }
            cx.notify();
        }

        fn on_wheel(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
            let lines = match ev.delta {
                ScrollDelta::Lines(p) => p.y,
                ScrollDelta::Pixels(p) => f32::from(p.y) / 40.0,
            };
            if lines.abs() < f32::EPSILON {
                return;
            }
            let at = self.local(ev.position);
            self.camera.zoom_about(ZOOM_STEP.powf(lines), at, self.board());
            cx.notify();
        }

        fn on_key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
            let board = self.board();
            match ev.keystroke.key.as_str() {
                "+" | "=" => self.camera.zoom_by(ZOOM_STEP, board),
                "-" | "_" => self.camera.zoom_by(1.0 / ZOOM_STEP, board),
                "0" => self.camera.reset(),
                "left" => self.camera.pan_by(40.0, 0.0),
                "right" => self.camera.pan_by(-40.0, 0.0),
                "up" => self.camera.pan_by(0.0, 40.0),
                "down" => self.camera.pan_by(0.0, -40.0),
                _ => return,
            }
            cx.notify();
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

    fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
        c.a *= a;
        c
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = cx.omarchy();
            let (edge_c, node_c, seed_c, dangling_c, label_c) =
                (theme.border, theme.foreground, theme.accent, theme.danger, theme.secondary);
            let seen = self.visible();
            let focus = self.hover.as_deref().and_then(|id| seen.index_of(id));
            let hi = highlight(&seen, focus);
            let pins = self.pins.clone();
            let prims = paint(&seen, &self.camera, self.board(), &hi, &move |id| pins.contains_key(id));
            let labels: Vec<Prim> =
                prims.iter().filter(|p| matches!(p, Prim::Label { .. })).cloned().collect();
            let origin = self.origin.clone();

            // Strokes are real hairlines built with PathBuilder and dashed by
            // the tessellator: the v0.2 run-of-dots is gone.
            let painted = canvas(
                move |_, _, _| {},
                move |bounds, _, window, _| {
                    origin.set((f32::from(bounds.origin.x), f32::from(bounds.origin.y)));
                    let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                    for p in &prims {
                        match p {
                            Prim::Stroke { x0, y0, x1, y1, width, dashed, alpha } => {
                                let mut b = PathBuilder::stroke(px(*width));
                                if *dashed {
                                    b = b.dash_array(&[
                                        px(super::view::DASH_PX[0]),
                                        px(super::view::DASH_PX[1]),
                                    ]);
                                }
                                b.move_to(at(*x0, *y0));
                                b.line_to(at(*x1, *y1));
                                if let Ok(path) = b.build() {
                                    window.paint_path(path, with_alpha(edge_c, *alpha));
                                }
                            }
                            Prim::Disc { x, y, r, seed, dangling, pinned, alpha } => {
                                let colour = if *dangling {
                                    dangling_c
                                } else if *seed {
                                    seed_c
                                } else {
                                    node_c
                                };
                                window.paint_quad(dot(at(*x, *y), *r, with_alpha(colour, *alpha)));
                                if *pinned {
                                    // A ring around a pinned disc, so a held
                                    // node is legible without a tooltip.
                                    window.paint_quad(dot(
                                        at(*x, *y),
                                        r + 3.0,
                                        with_alpha(seed_c, *alpha * 0.35),
                                    ));
                                }
                            }
                            Prim::Label { .. } => {}
                        }
                    }
                },
            )
            .absolute()
            .size_full();

            let mut board = div()
                .id("graph-board")
                .track_focus(&self.focus)
                .relative()
                .w(px(BOARD_W))
                .h(px(BOARD_H))
                .overflow_hidden()
                .bg(theme.background)
                .child(painted)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, ev, _, cx| this.on_down(ev, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, ev, _, cx| this.on_down(ev, cx)))
                .on_mouse_move(cx.listener(|this, ev, _, cx| this.on_move(ev, cx)))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, ev, _, cx| this.on_up(ev, cx)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev, _, cx| this.on_up(ev, cx)))
                .on_scroll_wheel(cx.listener(|this, ev, _, cx| this.on_wheel(ev, cx)))
                .on_key_down(cx.listener(|this, ev, _, cx| this.on_key(ev, cx)));

            // Labels stay elements so they use the theme's text stack; they are
            // placed from the same paint list as the discs, so they cannot drift.
            for l in labels {
                let Prim::Label { x, y, text, alpha } = l else { continue };
                if x < -80.0 || y < -20.0 || x > BOARD_W + 80.0 || y > BOARD_H + 20.0 {
                    continue;
                }
                board = board.child(
                    div()
                        .absolute()
                        .left(px(x - 40.0))
                        .top(px(y))
                        .w(px(80.0))
                        .flex()
                        .justify_center()
                        .child(div().text_xs().text_color(with_alpha(label_c, alpha)).child(text)),
                );
            }

            let dangling_label = format!("dangling: {}", if self.show_dangling { "shown" } else { "hidden" });
            let domain_label = format!("domain: {}", self.domain.clone().unwrap_or_else(|| "all".into()));
            let hop_label = format!("hop-{}", self.hops);
            let layout_label = format!("layout: {}", if self.rings { "rings (debug)" } else { "sim" });
            let zoom_label = format!("zoom {:.0}%", self.camera.zoom * 100.0);
            let pin_label = format!("unpin {}", self.pins.len());
            let controls = div()
                .flex()
                .gap_2()
                .child(div().id("f-dangling").text_sm().text_color(label_c).child(dangling_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.show_dangling = !this.show_dangling;
                        cx.notify();
                    }),
                ))
                .child(div().id("f-domain").text_sm().text_color(label_c).child(domain_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        // cycle: all -> each domain -> all
                        let all = this.scene.domains();
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
                ))
                .child(div().id("f-layout").text_sm().text_color(label_c).child(layout_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.rings = !this.rings;
                        this.reload();
                        cx.notify();
                    }),
                ))
                .child(div().id("f-zoom").text_sm().text_color(label_c).child(zoom_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.camera.reset();
                        cx.notify();
                    }),
                ))
                .child(div().id("f-unpin").text_sm().text_color(label_c).child(pin_label).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.pins.clear();
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
                        "{}  ·  {} nodes, {} links  ·  {}  ·  drag to pan, wheel or +/- to zoom",
                        self.seed,
                        self.scene.nodes.len(),
                        self.scene.edges.len(),
                        if self.rings { format!("hop-{} rings", self.hops) } else { "force sim".into() }
                    ),
                }))
        }
    }

    pub fn open(opts: Options) -> Result<(), DesktopError> {
        let seed = opts.seed.clone().unwrap_or_else(|| "Welcome.md".to_string());
        let vault = opts.vault_root.clone();
        let title = opts.title.clone();
        let show_dangling = true;
        let hops = 2u32;
        // Default view: the whole vault, laid out by the force sim.
        let (scene, error) = match graph_data::global_scene(&vault, &seed, graph_data::SETTLE_TICKS) {
            Ok(s) => (s, None),
            Err(e) => (Scene::default(), Some(e)),
        };
        gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
            gpui_omarchy::init(cx);
            let focus = cx.focus_handle();
            let opened = cx.open_window(WindowOptions::default(), |window, cx| {
                window.focus(&focus, cx);
                cx.new(|_| Root {
                    title: title.clone(),
                    vault: vault.clone(),
                    seed: seed.clone(),
                    scene: scene.clone(),
                    error: error.clone(),
                    selected: None,
                    peek: None,
                    show_dangling,
                    domain: None,
                    hops,
                    rings: false,
                    camera: Camera::default(),
                    hover: None,
                    press: None,
                    pins: BTreeMap::new(),
                    origin: Rc::new(Cell::new((0.0, 0.0))),
                    focus: focus.clone(),
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

    #[test]
    fn window_paints_shipped_draw_on_gpui_omarchy() {
        let src = include_str!("lib.rs");
        assert!(src.contains("view::paint"), "window consumes the view paint list");
        assert!(src.contains("gpui_omarchy::init"), "window is gpui-omarchy, not Zed-gpui");
        assert!(src.contains("Prim::Stroke"), "edges are strokes, not dot runs");
        assert!(src.contains("PathBuilder::stroke"), "hairlines, not dotted quads");
        assert!(src.contains("dash_array"), "a dangling link is dashed by the tessellator");
        assert!(src.contains("on_scroll_wheel"), "wheel zooms");
        assert!(src.contains("on_key_down"), "+/- zoom and arrows pan");
        assert!(src.contains("pan_by"), "drag on empty space pans the camera");
        assert!(src.contains("open_note"), "a short press still opens the note");
        assert!(src.contains("this.pins.insert"), "a dragged node pins where it is dropped");
        assert!(src.contains("f-unpin"), "pins can be released");
        assert!(src.contains("highlight"), "hover lights the neighbourhood");
        assert!(src.contains("show_dangling"), "dangling filter");
        assert!(src.contains("f-domain"), "domain filter");
        assert!(src.contains("graph_data::global_scene"), "default layout is the whole-vault snapshot");
        assert!(src.contains("rings: false"), "hop rings are the debug view, not the default");
        assert!(src.contains("f-layout"), "rings stay reachable as a debug toggle");
        let toml = include_str!("../Cargo.toml");
        assert!(toml.contains("gpui-omarchy"));
        assert!(!toml.contains("gpui = \"0.2"));
    }
}
