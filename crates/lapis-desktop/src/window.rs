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
use gpui_omarchy::ActiveTheme;

mod board;
mod lifecycle;
mod render;
pub(crate) use lifecycle::OpenNote;

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

pub(crate) struct Root {
    services: Option<std::sync::Arc<dyn crate::services::WorkspaceServices>>,
    snapshot: Option<std::sync::Arc<lapis_lattice::GraphSnapshot>>,
    loading: bool,
    load_epoch: u64,
    preview_epoch: u64,
    visible: bool,
    vault: PathBuf,
    /// The active note. `--path` sets it; without one the global graph has
    /// no seed and nothing is highlighted.
    seed: Option<String>,
    scene: Scene,
    error: Option<String>,
    /// Node the operator clicked, and the head of that note.
    selected: Option<String>,
    /// The previewed note's id and the text under it, kept apart so a path that
    /// contains the card's separator still shows whole.
    peek: Option<(String, String)>,
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
    fit_zoom: f32,
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
    /// Where the whole-vault map last came to rest, and the pins on it. Seeds
    /// the next layout, so the map keeps its shape across saves and launches.
    memory: Option<std::sync::Arc<crate::graph_memory::Remembered>>,
    /// A drag or an unpin changed the map; remember it once it is still again.
    layout_dirty: bool,
    /// A layout in progress: the background settle leaves its newest scene and
    /// positions here, and the board draws them, so the graph unfolds in view.
    unfolding: Option<std::sync::Arc<std::sync::Mutex<graph_data::Unfolding>>>,
    /// True once the scene on the board came from the layout in progress.
    unfolded: bool,
    /// The operator moved the camera during this load, so it is theirs: the
    /// board stops framing the graph for them.
    camera_held: bool,
    /// Where the camera is gliding to, if anywhere.
    camera_to: Option<Camera>,
    /// The finished layout sliding in: where each node started, and when.
    glide: Option<(Vec<[f32; 2]>, Instant)>,
    /// The busiest top-level folders and their note counts; their order is their colour.
    folders: Vec<(String, usize)>,
    focus: FocusHandle,
}

impl Root {
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
        // After an unfold the board already shows this graph part way there: slide
        // from what is on screen to the final places, and glide the camera to match.
        let shown: Vec<[f32; 2]> = self.scene.nodes.iter().map(|n| [n.x, n.y]).collect();
        let slide = self.unfolded && shown.len() == laid.scene.nodes.len();
        self.scene = laid.scene;
        self.sim = laid.sim;
        self.folders = crate::graph_style::top_folders(&self.scene);
        let board = self.board();
        let mut framed = self.camera;
        framed.fit(&self.scene, board);
        self.fit_zoom = framed.zoom;
        self.fitted_for = (board[0], board[1]);
        if slide {
            for (n, p) in self.scene.nodes.iter_mut().zip(&shown) {
                (n.x, n.y) = (p[0], p[1]);
            }
            self.glide = Some((shown, Instant::now()));
            self.camera_to = (!self.camera_held).then_some(framed);
        } else {
            self.camera = framed;
            self.camera_to = None;
        }
        self.unfolding = None;
        self.unfolded = false;
        self.meter.idle();
    }

    /// Draw the newest of a layout in progress. Returns true while one is running,
    /// which keeps frames coming until it lands.
    fn unfold(&mut self) -> bool {
        let Some(slot) = self.unfolding.clone() else { return false };
        let (scene, positions) = match slot.lock() {
            Ok(mut latest) => (latest.scene.take(), latest.positions.take()),
            Err(_) => return true,
        };
        let board = self.board();
        if let Some(mut scene) = scene {
            scene.apply_pins(&self.pins);
            self.scene = scene;
            self.folders = crate::graph_style::top_folders(&self.scene);
            self.unfolded = true;
            if !self.camera_held {
                self.camera.fit(&self.scene, board);
                self.fit_zoom = self.camera.zoom;
            }
        }
        if let Some(xy) = positions.filter(|xy| xy.len() == self.scene.nodes.len()) {
            for (n, p) in self.scene.nodes.iter_mut().zip(&xy) {
                (n.x, n.y) = (p[0], p[1]);
            }
            self.scene.apply_pins(&self.pins);
            if !self.camera_held {
                let mut framed = self.camera;
                framed.fit(&self.scene, board);
                self.fit_zoom = framed.zoom;
                self.camera_to = Some(framed);
            }
        }
        if let Some(to) = self.camera_to
            && !crate::glide::camera_toward(&mut self.camera, &to, crate::glide::CAMERA_GLIDE)
        {
            self.camera_to = None;
        }
        true
    }

    /// Keep the whole-vault map as it rests now, in this session and in the
    /// cache directory. Local and ring views have their own shapes and are not kept.
    fn remember(&mut self, cx: &mut Context<Self>) {
        self.layout_dirty = false;
        if self.layout != Layout::Global || self.scene.nodes.is_empty() {
            return;
        }
        // The sim holds the final places; the scene may still be gliding toward them.
        let xy: Vec<[f32; 2]> = if self.sim.len() == self.scene.nodes.len() {
            self.sim.positions().to_vec()
        } else {
            self.scene.nodes.iter().map(|n| [n.x, n.y]).collect()
        };
        let nodes = self.scene.nodes.iter().zip(xy).map(|(n, p)| (n.id.clone(), p)).collect();
        let memory = std::sync::Arc::new(crate::graph_memory::Remembered::new(nodes, self.pins.clone()));
        self.memory = Some(memory.clone());
        let vault = self.vault.clone();
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = crate::graph_memory::save(&vault, &memory) {
                    eprintln!("lapis desktop: graph layout not remembered: {e}");
                }
            })
            .detach();
    }

    /// The operator panned or zoomed: during a load, stop framing for them.
    fn hold_camera(&mut self) {
        self.camera_held = true;
        self.camera_to = None;
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
        if let Some((from, start)) = &self.glide {
            let t =
                crate::glide::smooth(start.elapsed().as_secs_f32() * 1000.0 / crate::glide::SETTLE_GLIDE_MS);
            let xy = crate::glide::blend(from, self.sim.positions(), t);
            for (n, p) in self.scene.nodes.iter_mut().zip(&xy) {
                (n.x, n.y) = (p[0], p[1]);
            }
            if t >= 1.0 {
                self.glide = None;
            }
            moving = true;
        } else if moving {
            graph_data::sync_positions(&mut self.scene, &self.sim);
        }
        if let Some(to) = self.camera_to {
            if crate::glide::camera_toward(&mut self.camera, &to, crate::glide::CAMERA_GLIDE) {
                moving = true;
            } else {
                self.camera_to = None;
            }
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

    /// The node under a board-local point, honouring the visible filters.
    fn node_at(&self, at: [f32; 2]) -> Option<String> {
        let f = self.filters.clone();
        hit_test(&self.scene, &self.camera, self.board(), at, &|n| n.passes(&f))
            .map(|i| self.scene.nodes[i].id.clone())
    }

    fn on_down(&mut self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        if (self.loading && !self.unfolded) || self.error.is_some() {
            return;
        }
        let at = self.local(ev.position);
        // While the graph unfolds a press only pans: nodes are still on the move.
        let node = if self.loading { None } else { self.node_at(at) };
        if ev.button == MouseButton::Right {
            // Right-click a pinned node releases it; the force layout is
            // free to place it again on the next rebuild.
            if let Some(id) = node
                && self.pins.remove(&id).is_some()
            {
                if let Some(i) = self.scene.index_of(&id) {
                    self.sim.set_pinned(i, false);
                }
                self.layout_dirty = true;
                cx.notify();
            }
            return;
        }
        let index = node.as_deref().and_then(|id| self.scene.index_of(id));
        self.press = Some(Press { node, index, from: at, last: at, moved: false });
    }

    fn on_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.local(ev.position);
        let board = self.board();

        if self.press.is_none() {
            // No button down: this is hover.
            let over = self.node_at(at);
            if over != self.hover {
                self.hover = over.clone();
                self.preview(over, window, cx);
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
                    // The sim holds it there and keeps the layout gently warm,
                    // so the neighbours follow and nothing else wanders.
                    self.sim.drag_to(i, w[0], w[1]);
                    self.sim.set_pinned(i, true);
                    self.sim.hold_warm(true);
                }
            }
            // Drag empty space: pan the camera. The layout does not move.
            None => {
                self.hold_camera();
                self.camera.pan_by(dx, dy);
            }
        }
        // While the graph is moving the next animation frame is already
        // booked. Asking for a second render per pointer event on top of it
        // is how a drag loses frames it did not need to lose.
        if !self.sim.is_running() && !self.loading {
            cx.notify();
        }
    }

    fn on_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else { return };
        self.layout_dirty |= press.moved && press.node.is_some();
        // A drag ends: the layout cools back to stillness around the dropped node.
        self.sim.hold_warm(false);
        // A short press is still a click, even on a node you could have
        // dragged.
        if !press.moved
            && let Some(id) = press.node
        {
            self.open_note(&id, window, cx);
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
        self.hold_camera();
        self.camera.zoom_about(ZOOM_STEP.powf(lines), at, self.board());
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control || ev.keystroke.modifiers.alt {
            return;
        }
        if !self.typing && ev.keystroke.key == "tab" {
            let visible = self.visible();
            let ids: Vec<String> = visible.nodes.iter().map(|n| n.id.clone()).collect();
            if !ids.is_empty() {
                let current = ids.iter().position(|id| Some(id) == self.selected.as_ref());
                let index = match (current, ev.keystroke.modifiers.shift) {
                    (Some(i), false) => (i + 1) % ids.len(),
                    (Some(i), true) => (i + ids.len() - 1) % ids.len(),
                    (None, false) => 0,
                    (None, true) => ids.len() - 1,
                };
                self.selected = Some(ids[index].clone());
                self.preview(Some(ids[index].clone()), window, cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if !self.typing && ev.keystroke.key == "enter" {
            if let Some(id) = self.selected.clone().filter(|id| self.visible().index_of(id).is_some()) {
                self.open_note(&id, window, cx);
            }
            cx.stop_propagation();
            return;
        }
        let board = self.board();
        let shift = ev.keystroke.modifiers.shift;
        let mut rebuild = false;
        match key_action(&ev.keystroke.key, shift, self.typing) {
            KeyAction::None => return,
            KeyAction::Zoom(f) => {
                self.hold_camera();
                self.camera.zoom_by(f, board);
            }
            KeyAction::Pan(dx, dy) => {
                self.hold_camera();
                self.camera.pan_by(dx, dy);
            }
            KeyAction::ResetCamera => {
                // Fit hands the camera back: a load in progress frames it again.
                let board = self.board();
                self.camera.fit(&self.scene, board);
                self.fit_zoom = self.camera.zoom;
                self.camera_held = false;
                self.camera_to = None;
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
                self.layout = self.next_layout();
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
            self.reload(window, cx);
        }
        cx.stop_propagation();
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
pub(crate) fn segments_path(segs: &[[f32; 4]], width: f32, origin: Point<Pixels>) -> Option<Path<Pixels>> {
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

pub fn open(opts: Options) -> Result<(), DesktopError> {
    crate::log_to_stderr();
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        gpui_omarchy::init(cx);
        crate::theme::init(cx);
        let opened = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| {
                let mut root = Root::new(opts.vault_root.clone(), None, opts.seed.clone(), cx);
                root.show(opts.seed.clone(), window, cx);
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
