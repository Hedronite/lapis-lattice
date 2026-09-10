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

    use super::scene::{Filters, Scene};
    use super::view::{
        CLICK_SLOP_PX, Camera, Group, GroupColour, KeyAction, Prim, ZOOM_STEP, highlight, hit_test,
        key_action, paint,
    };
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

    /// Which graph is on the board.
    ///
    /// `Global` is the default and it is the whole index — every note, whether
    /// or not `--path` was given. `Local` is the same snapshot cut to a depth
    /// around the seed. `Rings` is the retired v0.2 ego walk, kept reachable
    /// for debugging and never the default.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Layout {
        Global,
        Local,
        Rings,
    }

    impl Layout {
        fn next(self) -> Self {
            match self {
                Layout::Global => Layout::Local,
                Layout::Local => Layout::Rings,
                Layout::Rings => Layout::Global,
            }
        }
        fn label(self) -> &'static str {
            match self {
                Layout::Global => "global",
                Layout::Local => "local",
                Layout::Rings => "rings (debug)",
            }
        }
    }

    struct Root {
        title: String,
        vault: PathBuf,
        /// The active note. `--path` sets it; without one the global graph has
        /// no seed and nothing is highlighted.
        seed: Option<String>,
        scene: Scene,
        error: Option<String>,
        /// Node the operator clicked, and the head of that note.
        selected: Option<String>,
        peek: Option<String>,
        filters: Filters,
        /// True while the query box has the keyboard: printable keys are text.
        typing: bool,
        /// Local-mode depth, 1..8.
        depth: u32,
        layout: Layout,
        /// Query to colour. First match wins.
        groups: Vec<Group>,
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
        /// Local mode cuts the same snapshot; the hop-ring walk stays reachable
        /// behind the layout toggle and is never the default.
        fn reload(&mut self) {
            let seed = self.seed.clone().unwrap_or_default();
            let ticks = graph_data::SETTLE_TICKS;
            let built = match self.layout {
                Layout::Global => graph_data::global_scene(&self.vault, &seed, ticks),
                Layout::Local if seed.is_empty() => {
                    Err("local mode needs a note: open with `--path <note>` or click one".into())
                }
                Layout::Local => graph_data::local_scene(&self.vault, &seed, self.depth, ticks),
                Layout::Rings => graph_data::scene_for(&self.vault, &seed, 2, false),
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
            self.scene.filtered(&self.filters)
        }

        fn open_note(&mut self, id: &str) {
            self.selected = Some(id.to_string());
            // Clicking a note makes it the active one, so local mode has a seed
            // even when the window opened with no `--path`.
            if !id.starts_with("dangling:") {
                let changed = self.seed.as_deref() != Some(id);
                self.seed = Some(id.to_string());
                if changed && self.layout == Layout::Local {
                    self.reload();
                }
            }
            self.peek = if id.starts_with("dangling:") {
                Some(format!("{id} — this link resolves to nothing"))
            } else {
                Some(graph_data::peek(&self.vault, id, 600).unwrap_or_else(|e| e))
            };
        }

        /// The node under a board-local point, honouring the visible filters.
        fn node_at(&self, at: [f32; 2]) -> Option<String> {
            let f = self.filters.clone();
            hit_test(&self.scene, &self.camera, self.board(), at, &|n| n.passes(&f))
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
            let shift = ev.keystroke.modifiers.shift;
            let mut rebuild = false;
            match key_action(&ev.keystroke.key, shift, self.typing) {
                KeyAction::None => return,
                KeyAction::Zoom(f) => self.camera.zoom_by(f, board),
                KeyAction::Pan(dx, dy) => self.camera.pan_by(dx, dy),
                KeyAction::ResetCamera => self.camera.reset(),
                KeyAction::StartQuery => self.typing = true,
                KeyAction::QueryPush(c) => self.filters.query.push(c),
                KeyAction::QueryPop => {
                    self.filters.query.pop();
                }
                KeyAction::QueryCommit => self.typing = false,
                KeyAction::QueryCancel => {
                    self.typing = false;
                    self.filters.query.clear();
                }
                KeyAction::ToggleDangling => self.filters.show_dangling = !self.filters.show_dangling,
                KeyAction::ToggleOrphans => self.filters.show_orphans = !self.filters.show_orphans,
                KeyAction::CycleLayout => {
                    self.layout = self.layout.next();
                    rebuild = true;
                }
                KeyAction::CycleDomain => self.cycle_domain(),
                KeyAction::Depth(step) => {
                    let d = (self.depth as i32 + step)
                        .clamp(graph_data::MIN_DEPTH as i32, graph_data::MAX_DEPTH as i32);
                    rebuild = d as u32 != self.depth && self.layout == Layout::Local;
                    self.depth = d as u32;
                }
                KeyAction::AddGroup => self.add_group(),
                KeyAction::ClearGroups => self.groups.clear(),
                KeyAction::Unpin => {
                    self.pins.clear();
                    rebuild = true;
                }
            }
            if rebuild {
                self.reload();
            }
            cx.notify();
        }

        /// Turn the query into a group and clear the box, so one gesture goes
        /// from "show me these" to "keep these coloured".
        fn add_group(&mut self) {
            let q = self.filters.query.trim().to_string();
            if q.is_empty() || self.groups.iter().any(|g| g.query == q) {
                return;
            }
            self.groups.push(Group::next(q, self.groups.len()));
            self.filters.query.clear();
            self.typing = false;
        }

        fn cycle_domain(&mut self) {
            let all = self.scene.domains();
            self.filters.domain = match &self.filters.domain {
                None => all.first().cloned(),
                Some(cur) => all.iter().position(|d| d == cur).and_then(|i| all.get(i + 1).cloned()),
            };
        }
    }

    /// A group's colour in the live theme. Roles resolve through Omarchy, so a
    /// theme swap restyles every group; a hex is taken literally, which is the
    /// escape hatch for "that exact colour".
    fn group_hsla(c: GroupColour, theme: &gpui_omarchy::Theme) -> Hsla {
        match c {
            GroupColour::Hex(rgb) => gpui_kit::rgb(rgb).into(),
            GroupColour::Role(r) => match r {
                "accent" => theme.accent,
                "ok" => theme.success,
                "warn" => theme.warning,
                "danger" => theme.danger,
                "bright" => theme.bright,
                _ => theme.secondary,
            },
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
            let groups = self.groups.clone();
            let palette = theme.clone();
            let group_of = move |g: GroupColour| group_hsla(g, &palette);
            let prims =
                paint(&seen, &self.camera, self.board(), &hi, &move |id| pins.contains_key(id), &groups);
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
                            Prim::Disc { x, y, r, seed, dangling, pinned, alpha, group } => {
                                // A group wins over the default roles: the
                                // operator asked for that colour by query.
                                let colour = match group {
                                    Some(g) => group_of(*g),
                                    None if *dangling => dangling_c,
                                    None if *seed => seed_c,
                                    None => node_c,
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

            let mode_label = match self.layout {
                Layout::Local => format!("local d{}", self.depth),
                other => other.label().to_string(),
            };
            let query_label = if self.typing {
                format!("query: {}_", self.filters.query)
            } else if self.filters.query.is_empty() {
                "query: /".to_string()
            } else {
                format!("query: {}", self.filters.query)
            };
            let existing_label =
                format!("existing only: {}", if self.filters.show_dangling { "off" } else { "on" });
            let orphans_label =
                format!("orphans: {}", if self.filters.show_orphans { "shown" } else { "hidden" });
            let domain_label =
                format!("domain: {}", self.filters.domain.clone().unwrap_or_else(|| "all".into()));
            let zoom_label = format!("zoom {:.0}%", self.camera.zoom * 100.0);
            let pin_label = format!("unpin {}", self.pins.len());

            let chip = |id: &'static str, text: String, colour: Hsla| {
                div().id(id).text_sm().text_color(colour).child(text)
            };

            let mut controls = div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(chip("f-layout", format!("layout: {mode_label}"), label_c).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.layout = this.layout.next();
                        this.reload();
                        cx.notify();
                    },
                )))
                .child(chip("f-depth", format!("depth {}", self.depth), label_c).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.depth = if this.depth >= graph_data::MAX_DEPTH {
                            graph_data::MIN_DEPTH
                        } else {
                            this.depth + 1
                        };
                        if this.layout == Layout::Local {
                            this.reload();
                        }
                        cx.notify();
                    },
                )))
                .child(chip("f-query", query_label, if self.typing { seed_c } else { label_c }).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.typing = !this.typing;
                        cx.notify();
                    }),
                ))
                .child(chip("f-existing", existing_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.filters.show_dangling = !this.filters.show_dangling;
                    cx.notify();
                })))
                .child(chip("f-orphans", orphans_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.filters.show_orphans = !this.filters.show_orphans;
                    cx.notify();
                })))
                .child(chip("f-domain", domain_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.cycle_domain();
                    cx.notify();
                })))
                .child(chip("f-group", "+group".into(), label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.add_group();
                    cx.notify();
                })))
                .child(chip("f-zoom", zoom_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.camera.reset();
                    cx.notify();
                })))
                .child(chip("f-unpin", pin_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                    this.pins.clear();
                    this.reload();
                    cx.notify();
                })));

            // Each group shows in its own colour, so the legend is the swatch.
            for (n, g) in self.groups.iter().enumerate() {
                let colour = group_hsla(g.colour, theme);
                controls = controls.child(
                    div()
                        .id(gpui_kit::SharedString::from(format!("group-{n}")))
                        .text_sm()
                        .text_color(colour)
                        .child(format!("● {}", g.query))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if n < this.groups.len() {
                                this.groups.remove(n);
                            }
                            cx.notify();
                        })),
                );
            }

            panel(self.title.as_str(), cx)
                .size_full()
                .bg(theme.background)
                .child(controls)
                .child(board)
                .child(div().text_sm().text_color(label_c).child(match (&self.error, &self.peek) {
                    (Some(e), _) => e.clone(),
                    (None, Some(p)) => p.clone(),
                    (None, None) => format!(
                        "{}  ·  {} of {} nodes, {} links  ·  {}  ·  / query · e existing · o orphans \
                         · g group · [ ] depth · l layout · drag to pan · wheel or +/- to zoom",
                        self.seed.as_deref().unwrap_or("whole vault"),
                        seen.nodes.len(),
                        self.scene.nodes.len(),
                        seen.edges.len(),
                        mode_label,
                    ),
                }))
        }
    }

    pub fn open(opts: Options) -> Result<(), DesktopError> {
        // No `--path` means no seed: the graph is still the whole vault, and
        // nothing is marked active. `--path` only chooses which note is.
        let seed = opts.seed.clone();
        let vault = opts.vault_root.clone();
        let title = opts.title.clone();
        // Default view: the whole indexed vault, laid out by the force sim.
        let (scene, error) = match graph_data::global_scene(
            &vault,
            seed.as_deref().unwrap_or_default(),
            graph_data::SETTLE_TICKS,
        ) {
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
                    filters: Filters::default(),
                    typing: false,
                    depth: 2,
                    layout: Layout::Global,
                    groups: Vec::new(),
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
        assert!(src.contains("layout: Layout::Global"), "global is the opening mode, not an ego ring");
        assert!(src.contains("graph_data::local_scene"), "local mode cuts the same snapshot");
        assert!(src.contains("f-layout") && src.contains("f-depth"), "layout and depth are reachable");
        assert!(src.contains("f-existing"), "existing-only filter");
        assert!(src.contains("f-orphans"), "orphans toggle");
        assert!(src.contains("f-query"), "query filter");
        assert!(src.contains("f-group") && src.contains("group_hsla"), "groups recolour by query");
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
