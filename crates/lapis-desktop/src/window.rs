//! GPUI window: graph canvas, sim tick, and input.
//!
//! Built only with `--features desktop`. Paint list from [`crate::view`];
//! positions from [`crate::sim`].

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    App, AppContext, Bounds, Context, Corners, Edges, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Path,
    Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, Size, StatefulInteractiveElement, Styled, Window,
    WindowOptions, canvas, div, point, px,
};
use gpui_omarchy::{ActiveTheme, panel};

use crate::scene::{Filters, Scene};
use crate::sim::ForceSim;
use crate::view::{
    CLICK_SLOP_PX, Camera, DIM_ALPHA, FULL_ALPHA, Group, GroupColour, KeyAction, Prim, ZOOM_STEP, highlight,
    hit_test, key_action, paint,
};
use crate::{DesktopError, Options, graph_data};

/// Fallback board size, used for the single frame before the canvas has
/// reported its real bounds. The strokes, the camera and the labels all read
/// one measured size after that: if the canvas mapped by its live bounds
/// while the labels used constants, the lines would not meet the discs.
const BOARD_W: f32 = 760.0;
const BOARD_H: f32 = 520.0;
/// Drawn height of a label box, shared with the placer that reserves it.
const LABEL_H: f32 = crate::view::LABEL_HEIGHT_PX;

/// `LAPIS_GRAPH_DEBUG=1` puts a frame-time overlay on the board. It is the
/// only way to answer "is this 60 fps" with a number instead of a feeling.
fn debug_overlay_on() -> bool {
    std::env::var("LAPIS_GRAPH_DEBUG").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Frames kept for the rolling average.
const FPS_WINDOW: usize = 60;
/// Ticks the sim is allowed per frame. One keeps the loop honest: the
/// screen shows what the physics just did, not a batch of it.
const TICKS_PER_FRAME: usize = 1;
/// A frame longer than this has missed its slot at 60 Hz. The next one skips
/// physics rather than compounding the miss.
const FRAME_BUDGET_MS: f32 = 14.0;

/// Rolling frame timing, so the overlay reports measured fps.
#[derive(Debug, Default)]
struct Meter {
    last: Option<Instant>,
    frames: std::collections::VecDeque<f32>,
    /// Physics.
    tick_ms: f32,
    /// Building the paint list.
    build_ms: f32,
}

impl Meter {
    fn frame(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last.replace(now) {
            let ms = now.duration_since(prev).as_secs_f32() * 1000.0;
            // A frame after an idle pause is not a dropped frame.
            if ms < 500.0 {
                self.frames.push_back(ms);
                if self.frames.len() > FPS_WINDOW {
                    self.frames.pop_front();
                }
            }
        }
    }
    fn fps(&self) -> f32 {
        if self.frames.is_empty() {
            return 0.0;
        }
        let mean = self.frames.iter().sum::<f32>() / self.frames.len() as f32;
        if mean <= 0.0 { 0.0 } else { 1000.0 / mean }
    }
    fn worst_ms(&self) -> f32 {
        self.frames.iter().copied().fold(0.0f32, f32::max)
    }
    fn idle(&mut self) {
        self.last = None;
        self.frames.clear();
    }
}

/// A press in flight: which node it started on (none = empty space, so it
/// pans) and whether it has travelled far enough to be a drag.
struct Press {
    node: Option<String>,
    /// Index of that node, resolved once. Looking it up by id on every
    /// pointer event is a linear scan of string comparisons.
    index: Option<usize>,
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
    /// The layout, still running. The scene holds what was drawn last
    /// frame; this is what moves it.
    sim: ForceSim,
    meter: Meter,
    camera: Camera,
    hover: Option<String>,
    press: Option<Press>,
    /// Where the operator dropped a node. Survives filter and layout
    /// rebuilds; cleared per node with a right-click, or all at once.
    pins: BTreeMap<String, [f32; 2]>,
    /// The board's top-left in window coordinates, written by the canvas as
    /// it paints and read by the mouse handlers. Both run on the UI thread.
    origin: Rc<Cell<(f32, f32)>>,
    /// The board's measured size, written the same way. The board fills the
    /// window's width, so a narrow window must not leave part of the graph
    /// laid out past its right edge where nobody can reach it.
    board_px: Rc<Cell<(f32, f32)>>,
    /// Size the camera was last framed for, so a resize refits and a settle
    /// does not.
    fitted_for: (f32, f32),
    /// Time spent inside the canvas last frame, written there and read by
    /// the overlay on the next one.
    draw_ms: Rc<Cell<f32>>,
    /// True when the last frame skipped physics to protect the frame slot.
    skipped_tick: bool,
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
            Layout::Global => graph_data::global_live(&self.vault, &seed, ticks),
            Layout::Local if seed.is_empty() => {
                Err("local mode needs a note: open with `--path <note>` or click one".into())
            }
            Layout::Local => graph_data::local_live(&self.vault, &seed, self.depth, ticks),
            // The retired ego walk has no sim behind it: it is a fixed ring
            // by definition, so it gets an empty one.
            Layout::Rings => graph_data::scene_for(&self.vault, &seed, 2, false)
                .map(|scene| graph_data::Laid { scene, sim: ForceSim::empty() }),
        };
        match built {
            Ok(laid) => {
                self.adopt(laid);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Take a freshly laid-out graph: restore the pins onto both the scene
    /// and the sim, then frame it.
    fn adopt(&mut self, mut laid: graph_data::Laid) {
        laid.scene.apply_pins(&self.pins);
        for (i, n) in laid.scene.nodes.iter().enumerate() {
            if let Some(p) = self.pins.get(&n.id) {
                laid.sim.place(i, p[0], p[1]);
                laid.sim.set_pinned(i, true);
            }
        }
        self.scene = laid.scene;
        self.sim = laid.sim;
        let board = self.board();
        self.camera.fit(&self.scene, board);
        self.fitted_for = (board[0], board[1]);
        self.meter.idle();
    }

    /// One frame of physics. Returns true while the graph is still moving,
    /// which is what keeps the render loop alive and what stops it.
    ///
    /// When the previous frame overran its slot, this one draws and skips
    /// the physics: a tick that pushes a frame past the budget costs the
    /// operator a smooth drag and buys settling nobody can see. Never twice
    /// in a row, so a slow machine still converges.
    fn advance(&mut self) -> bool {
        let overran = self.meter.frames.back().copied().unwrap_or(0.0) > FRAME_BUDGET_MS;
        if overran && !self.skipped_tick && self.sim.is_running() {
            self.skipped_tick = true;
            self.meter.tick_ms = 0.0;
            return true;
        }
        self.skipped_tick = false;
        let t0 = Instant::now();
        let mut moving = false;
        for _ in 0..TICKS_PER_FRAME {
            moving |= self.sim.tick();
        }
        if moving {
            graph_data::sync_positions(&mut self.scene, &self.sim);
        }
        self.meter.tick_ms = t0.elapsed().as_secs_f32() * 1000.0;
        moving
    }

    fn board(&self) -> [f32; 2] {
        let (w, h) = self.board_px.get();
        if w > 1.0 && h > 1.0 { [w, h] } else { [BOARD_W, BOARD_H] }
    }

    /// Window coordinates to board-local pixels.
    fn local(&self, at: Point<Pixels>) -> [f32; 2] {
        let (ox, oy) = self.origin.get();
        [f32::from(at.x) - ox, f32::from(at.y) - oy]
    }

    /// The scene as drawn. With no filter set this borrows: cloning two
    /// thousand nodes every frame to change nothing is a real cost.
    fn visible(&self) -> std::borrow::Cow<'_, Scene> {
        if self.filters.is_open() {
            std::borrow::Cow::Borrowed(&self.scene)
        } else {
            std::borrow::Cow::Owned(self.scene.filtered(&self.filters))
        }
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
                if let Some(i) = self.scene.index_of(&id) {
                    self.sim.set_pinned(i, false);
                }
                cx.notify();
            }
            return;
        }
        let index = node.as_deref().and_then(|id| self.scene.index_of(id));
        self.press = Some(Press { node, index, from: at, last: at, moved: false });
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
        let held = press.node.clone();
        let index = press.index;
        match held {
            // Drag a node: it follows the cursor in world space and stays
            // where it is dropped.
            Some(id) => {
                let w = self.camera.to_world(at, board);
                match self.pins.get_mut(&id) {
                    Some(slot) => *slot = w,
                    None => {
                        self.pins.insert(id, w);
                    }
                }
                if let Some(i) = index {
                    self.scene.nodes[i].x = w[0];
                    self.scene.nodes[i].y = w[1];
                    // The sim holds it there and lets the rest settle
                    // around it, which is what makes a drag feel live.
                    self.sim.place(i, w[0], w[1]);
                    self.sim.set_pinned(i, true);
                }
            }
            // Drag empty space: pan the camera. The layout does not move.
            None => self.camera.pan_by(dx, dy),
        }
        // While the graph is moving the next animation frame is already
        // booked. Asking for a second render per pointer event on top of it
        // is how a drag loses frames it did not need to lose.
        if !self.sim.is_running() {
            cx.notify();
        }
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
            KeyAction::ResetCamera => {
                let board = self.board();
                self.camera.fit(&self.scene, board);
            }
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
                self.sim.unpin_all();
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

/// One path holding every hairline as a pair of triangles.
///
/// `PathBuilder` runs each segment through lyon's stroke tessellator, which
/// costs about a microsecond apiece; at four thousand edges that was most of
/// the frame budget. A straight hairline is a rectangle and a rectangle is
/// two triangles, so the geometry goes to gpui directly instead of being
/// derived by a tessellator that is built for curves we do not draw. The
/// `st` coordinates are the ones gpui's own `line_to` uses for a solid
/// triangle, and every quad is wound the same way.
fn segments_path(segs: &[[f32; 4]], width: f32, origin: Point<Pixels>) -> Option<Path<Pixels>> {
    let first = segs.first()?;
    let at = |x: f32, y: f32| Point { x: origin.x + px(x), y: origin.y + px(y) };
    let solid = (point(0., 1.), point(0., 1.), point(0., 1.));
    let mut path = Path::new(at(first[0], first[1]));
    let half = (width / 2.0).max(0.35);
    for s in segs {
        let (dx, dy) = (s[2] - s[0], s[3] - s[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len <= f32::EPSILON || !len.is_finite() {
            continue;
        }
        let (nx, ny) = (-dy / len * half, dx / len * half);
        let a = at(s[0] + nx, s[1] + ny);
        let b = at(s[0] - nx, s[1] - ny);
        let c = at(s[2] - nx, s[3] - ny);
        let d = at(s[2] + nx, s[3] + ny);
        path.push_triangle((a, b, c), solid);
        path.push_triangle((a, c, d), solid);
    }
    Some(path)
}

/// Cut a segment into dash runs. A link that resolves to nothing is drawn
/// dashed, and a hand-built path has to produce the gaps itself.
fn dashes(seg: [f32; 4], on: f32, off: f32, out: &mut Vec<[f32; 4]>) {
    let (dx, dy) = (seg[2] - seg[0], seg[3] - seg[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= f32::EPSILON || !len.is_finite() {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let step = (on + off).max(0.5);
    let mut t = 0.0f32;
    while t < len {
        let end = (t + on).min(len);
        out.push([seg[0] + ux * t, seg[1] + uy * t, seg[0] + ux * end, seg[1] + uy * end]);
        t += step;
    }
}

/// A hollow rectangle, for the debug overlay's label boxes.
fn outline(x: f32, y: f32, w: f32, h: f32, origin: Point<Pixels>, colour: Hsla) -> PaintQuad {
    PaintQuad {
        bounds: Bounds {
            origin: Point { x: origin.x + px(x), y: origin.y + px(y) },
            size: Size { width: px(w), height: px(h) },
        },
        corner_radii: Corners::all(px(2.0)),
        background: gpui_kit::transparent_black().into(),
        border_widths: Edges::all(px(1.0)),
        border_color: colour,
        border_style: Default::default(),
    }
}

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a *= a;
    c
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.omarchy();
        // The first frame lays out against the fallback size; once the
        // canvas reports its real bounds, frame the graph for those.
        let board_now = self.board();
        if self.fitted_for != (board_now[0], board_now[1]) {
            self.camera.fit(&self.scene, board_now);
            self.fitted_for = (board_now[0], board_now[1]);
        }
        // The graph keeps moving until it settles; each frame asks for the
        // next one, and a graph at rest stops asking.
        let moving = self.advance();
        if moving {
            self.meter.frame();
            window.request_animation_frame();
        } else {
            self.meter.idle();
        }
        let (edge_c, node_c, seed_c, dangling_c, label_c) =
            (theme.border, theme.foreground, theme.accent, theme.danger, theme.secondary);
        let seen = self.visible();
        let focus = self.hover.as_deref().and_then(|id| seen.index_of(id));
        let hi = highlight(&seen, focus);
        let pins = self.pins.clone();
        let groups = self.groups.clone();
        let palette = theme.clone();
        let group_of = move |g: GroupColour| group_hsla(g, &palette);
        let prims = paint(&seen, &self.camera, self.board(), &hi, &move |id| pins.contains_key(id), &groups);
        let labels: Vec<Prim> = prims.iter().filter(|p| matches!(p, Prim::Label { .. })).cloned().collect();
        let label_count = labels.len();
        let draw_last = self.draw_ms.get();
        let origin = self.origin.clone();
        let board_px = self.board_px.clone();
        let show_boxes = debug_overlay_on();
        let draw_ms = self.draw_ms.clone();

        // Strokes are real hairlines built with PathBuilder and dashed by
        // the tessellator: the v0.2 run-of-dots is gone.
        let painted = canvas(
            move |_, _, _| {},
            move |bounds, _, window, _| {
                let t_draw = Instant::now();
                origin.set((f32::from(bounds.origin.x), f32::from(bounds.origin.y)));
                board_px.set((f32::from(bounds.size.width), f32::from(bounds.size.height)));
                let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));

                // Every stroke of one colour goes into one path. The
                // buckets are (solid|dashed) x (lit|dimmed), which is every
                // distinct colour a stroke can have.
                let mut batch: [Vec<[f32; 4]>; 4] = Default::default();
                let mut width = crate::view::STROKE_PX;
                for p in &prims {
                    if let Prim::Stroke { x0, y0, x1, y1, width: w, dashed, alpha } = p {
                        width = *w;
                        let slot = usize::from(*dashed) | (usize::from(*alpha < FULL_ALPHA) << 1);
                        if *dashed {
                            dashes(
                                [*x0, *y0, *x1, *y1],
                                crate::view::DASH_PX[0],
                                crate::view::DASH_PX[1],
                                &mut batch[slot],
                            );
                        } else {
                            batch[slot].push([*x0, *y0, *x1, *y1]);
                        }
                    }
                }
                for (slot, segs) in batch.iter().enumerate() {
                    let alpha = if slot & 2 != 0 { DIM_ALPHA } else { FULL_ALPHA };
                    if let Some(path) = segments_path(segs, width, bounds.origin) {
                        window.paint_path(path, with_alpha(edge_c, alpha));
                    }
                }

                if show_boxes {
                    // The rectangle each label reserved, so a screenshot
                    // shows whether the drawn text stays inside its claim
                    // instead of leaving it to be argued about.
                    for p in &prims {
                        if let Prim::Label { x, y, text, .. } = p {
                            let b = crate::view::label_box(text, [*x, *y]);
                            window.paint_quad(outline(
                                b[0],
                                b[1],
                                b[2] - b[0],
                                b[3] - b[1],
                                bounds.origin,
                                with_alpha(seed_c, 0.45),
                            ));
                        }
                    }
                }

                for p in &prims {
                    if let Prim::Disc { x, y, r, seed, dangling, pinned, alpha, group } = p {
                        // A group wins over the default roles: the operator
                        // asked for that colour by query.
                        let colour = match group {
                            Some(g) => group_of(*g),
                            None if *dangling => dangling_c,
                            None if *seed => seed_c,
                            None => node_c,
                        };
                        if *pinned {
                            // A ring around a pinned disc, so a held node is
                            // legible without a tooltip.
                            window.paint_quad(dot(at(*x, *y), r + 3.0, with_alpha(seed_c, *alpha * 0.35)));
                        }
                        window.paint_quad(dot(at(*x, *y), *r, with_alpha(colour, *alpha)));
                    }
                }
                draw_ms.set(t_draw.elapsed().as_secs_f32() * 1000.0);
            },
        )
        .absolute()
        .size_full();

        let mut board = div()
            .id("graph-board")
            .track_focus(&self.focus)
            .relative()
            .w_full()
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

        if debug_overlay_on() {
            let (lo, hi) = self.sim.extent();
            let overlay = format!(
                "{:.0} fps · worst {:.1} ms · tick {:.2} · build {:.2} · draw {:.2} ms · \
                 {} nodes / {} edges / {} labels · energy {:.4} · {}",
                self.meter.fps(),
                self.meter.worst_ms(),
                self.meter.tick_ms,
                self.meter.build_ms,
                draw_last,
                self.scene.nodes.len(),
                self.scene.edges.len(),
                label_count,
                self.sim.energy(),
                if moving {
                    format!("settling, span {:.0}", (hi[0] - lo[0]).max(hi[1] - lo[1]))
                } else {
                    "at rest".to_string()
                },
            );
            board = board.child(
                div().absolute().left(px(6.0)).top(px(6.0)).text_xs().text_color(theme.accent).child(overlay),
            );
        }

        // Labels stay elements so they use the theme's text stack; they are
        // placed from the same paint list as the discs, so they cannot drift.
        // Each label is drawn in exactly the rectangle the placer reserved
        // for it: same width, same height, clipped, with the line height
        // pinned. gpui's default line box is over two ems tall, so a label
        // that reserved thirteen pixels and drew twenty-six is how they
        // ended up stacked. Nothing here is free to grow past its box.
        for l in labels {
            let Prim::Label { x, y, text, alpha } = l else { continue };
            let w = crate::view::label_width_px(&text);
            if x + w < 0.0 || y + LABEL_H < 0.0 || x - w > board_now[0] || y > board_now[1] {
                continue;
            }
            board = board.child(
                div()
                    .absolute()
                    .left(px(x - w / 2.0))
                    .top(px(y))
                    .w(px(w))
                    .h(px(LABEL_H))
                    .overflow_hidden()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(LABEL_H))
                            .whitespace_nowrap()
                            .text_color(with_alpha(label_c, alpha))
                            .child(text),
                    ),
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
        let domain_label = format!("domain: {}", self.filters.domain.clone().unwrap_or_else(|| "all".into()));
        let zoom_label = format!("zoom {:.0}%", self.camera.zoom * 100.0);
        let pin_label = format!("unpin {}", self.pins.len());
        let theme_label = format!("theme: {}", theme.name);

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
                let board = this.board();
                this.camera.fit(&this.scene, board);
                cx.notify();
            })))
            .child(chip("f-unpin", pin_label, label_c).on_click(cx.listener(|this, _, _, cx| {
                this.pins.clear();
                this.sim.unpin_all();
                cx.notify();
            })))
            // The live Omarchy theme by name, so a swap is visible in a
            // screenshot and not just a claim.
            .child(chip("f-theme", theme_label, label_c));

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

        panel(self.title.as_str(), cx).size_full().bg(theme.background).child(controls).child(board).child(
            div().text_sm().text_color(label_c).child(match (&self.error, &self.peek) {
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
            }),
        )
    }
}

pub fn open(opts: Options) -> Result<(), DesktopError> {
    // No `--path` means no seed: the graph is still the whole vault, and
    // nothing is marked active. `--path` only chooses which note is.
    let seed = opts.seed.clone();
    let vault = opts.vault_root.clone();
    let title = opts.title.clone();
    // Default view: the whole indexed vault, laid out by the force sim.
    let (laid, error) = match graph_data::global_live(
        &vault,
        seed.as_deref().unwrap_or_default(),
        graph_data::SETTLE_TICKS,
    ) {
        Ok(l) => (l, None),
        Err(e) => (graph_data::Laid { scene: Scene::default(), sim: ForceSim::empty() }, Some(e)),
    };
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        gpui_omarchy::init(cx);
        let focus = cx.focus_handle();
        let opened = cx.open_window(WindowOptions::default(), |window, cx| {
            window.focus(&focus, cx);
            cx.new(|_| {
                let mut root = Root {
                    title: title.clone(),
                    vault: vault.clone(),
                    seed: seed.clone(),
                    scene: Scene::default(),
                    error: error.clone(),
                    selected: None,
                    peek: None,
                    filters: Filters::default(),
                    typing: false,
                    depth: 2,
                    layout: Layout::Global,
                    groups: Vec::new(),
                    sim: ForceSim::empty(),
                    meter: Meter::default(),
                    camera: Camera::default(),
                    hover: None,
                    press: None,
                    pins: BTreeMap::new(),
                    origin: Rc::new(Cell::new((0.0, 0.0))),
                    board_px: Rc::new(Cell::new((0.0, 0.0))),
                    fitted_for: (0.0, 0.0),
                    draw_ms: Rc::new(Cell::new(0.0)),
                    skipped_tick: false,
                    focus: focus.clone(),
                };
                root.adopt(graph_data::Laid { scene: laid.scene.clone(), sim: laid.sim.clone() });
                root
            })
        });
        match opened {
            Ok(_) => cx.activate(true),
            Err(e) => eprintln!("lapis desktop: could not open a window: {e}"),
        }
    });
    Ok(())
}
