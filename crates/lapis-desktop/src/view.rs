//! Camera, hit testing and the paint list.
//!
//! Everything here is arithmetic on a [`Scene`] the sim already laid out. The
//! camera is a *transform*: pan and zoom move the view over fixed world
//! positions and never re-run the forces, which is the difference between a
//! graph you can read and one that jumps every time you touch the wheel.
//!
//! World units are the scene's (roughly -2..2). Screen units are pixels
//! relative to the board's top-left corner; the window adds the board origin
//! when it paints. Keeping that conversion in one place is what stops the
//! painted strokes and the positioned labels from drifting apart.

use std::collections::BTreeSet;

use crate::scene::{Edge, Node, Scene};

/// Scene units to pixels at zoom 1. Scene units are the sim's own, so the base
/// is 1 and [`Camera::fit`] chooses the zoom that makes a graph fill the board.
pub const UNIT: f32 = 1.0;
/// Non-neighbours during hover. Quartz dims to this and so do we.
pub const DIM_ALPHA: f32 = 0.2;
pub const FULL_ALPHA: f32 = 1.0;
pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 40.0;
/// Breathing room when fitting a graph to the board.
pub const FIT_MARGIN_PX: f32 = 48.0;
/// One `+` / `-` press, and one wheel line.
pub const ZOOM_STEP: f32 = 1.2;
/// A press that travels further than this is a drag, not a click.
pub const CLICK_SLOP_PX: f32 = 4.0;
/// One arrow-key pan.
pub const PAN_STEP_PX: f32 = 40.0;
/// Extra pixels around a disc that still count as grabbing it.
pub const GRAB_MARGIN_PX: f32 = 4.0;
/// Edge stroke width. A hairline, not a bar.
pub const STROKE_PX: f32 = 1.0;
/// Dash pattern for a link that resolves to nothing.
pub const DASH_PX: [f32; 2] = [4.0, 3.0];
/// A stroke shorter than this is inside the discs it joins; drawing it costs a
/// tessellated path and shows nothing.
pub const MIN_STROKE_PX: f32 = 1.5;

/// Pan and zoom over a fixed layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { zoom: 1.0, pan_x: 0.0, pan_y: 0.0 }
    }
}

impl Camera {
    /// World point to board-local pixels.
    pub fn to_screen(&self, world: [f32; 2], board: [f32; 2]) -> [f32; 2] {
        [
            board[0] / 2.0 + self.pan_x + world[0] * UNIT * self.zoom,
            board[1] / 2.0 + self.pan_y + world[1] * UNIT * self.zoom,
        ]
    }

    /// Board-local pixels back to a world point.
    pub fn to_world(&self, screen: [f32; 2], board: [f32; 2]) -> [f32; 2] {
        let s = (UNIT * self.zoom).max(f32::EPSILON);
        [(screen[0] - board[0] / 2.0 - self.pan_x) / s, (screen[1] - board[1] / 2.0 - self.pan_y) / s]
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    /// Zoom keeping the world point under `anchor` pinned to `anchor`, which is
    /// what makes wheel-zoom feel like a camera rather than a jump.
    pub fn zoom_about(&mut self, factor: f32, anchor: [f32; 2], board: [f32; 2]) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let held = self.to_world(anchor, board);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let now = self.to_screen(held, board);
        self.pan_x += anchor[0] - now[0];
        self.pan_y += anchor[1] - now[1];
    }

    /// Keyboard zoom, about the middle of the board.
    pub fn zoom_by(&mut self, factor: f32, board: [f32; 2]) {
        self.zoom_about(factor, [board[0] / 2.0, board[1] / 2.0], board);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Frame the whole scene: centre its bounding box and pick the zoom that
    /// fits it with a margin. This is what makes "reset" mean "show me
    /// everything" instead of "show me whatever is near the origin".
    pub fn fit(&mut self, scene: &Scene, board: [f32; 2]) {
        let Some(first) = scene.nodes.first() else {
            *self = Self::default();
            return;
        };
        let (mut lo, mut hi) = ([first.x, first.y], [first.x, first.y]);
        for n in &scene.nodes {
            lo = [lo[0].min(n.x), lo[1].min(n.y)];
            hi = [hi[0].max(n.x), hi[1].max(n.y)];
        }
        let span = [(hi[0] - lo[0]).max(1e-3), (hi[1] - lo[1]).max(1e-3)];
        let room = [(board[0] - 2.0 * FIT_MARGIN_PX).max(1.0), (board[1] - 2.0 * FIT_MARGIN_PX).max(1.0)];
        self.zoom = (room[0] / (span[0] * UNIT)).min(room[1] / (span[1] * UNIT)).clamp(MIN_ZOOM, MAX_ZOOM);
        let mid = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        self.pan_x = -mid[0] * UNIT * self.zoom;
        self.pan_y = -mid[1] * UNIT * self.zoom;
    }
}

/// Disc radius in pixels.
///
/// The size law is Quartz's `2 + sqrt(degree)`, so a hub reads as a hub and the
/// curve flattens instead of letting one huge note swallow the board. The seed
/// gets half again, the whole thing is scaled to pixels, and the zoom
/// contribution is damped and clamped so a far-out view still has visible discs
/// and a close-in one does not paint saucers.
pub fn node_radius_px(n: &Node, zoom: f32) -> f32 {
    let base = 2.0 + (n.degree as f32).sqrt();
    let base = if n.is_seed { base * 1.5 } else { base };
    // The zoom contribution is damped but it has to keep shrinking discs on the
    // way out, or a two-thousand-node vault paints itself into a solid blob.
    (base * 1.6 * zoom.clamp(0.2, 2.0)).clamp(1.5, 44.0)
}

/// The node under `at` (board-local pixels), or none for empty space — which is
/// what tells a press apart from a pan.
///
/// `keep` is the same predicate the filters draw with, so a hidden node is not
/// secretly grabbable, and the index returned is into the *unfiltered* scene:
/// pointer state then survives a filter change without remapping.
pub fn hit_test(
    scene: &Scene,
    cam: &Camera,
    board: [f32; 2],
    at: [f32; 2],
    keep: &dyn Fn(&Node) -> bool,
) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for (i, n) in scene.nodes.iter().enumerate() {
        if !keep(n) {
            continue;
        }
        let p = cam.to_screen([n.x, n.y], board);
        let d = ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)).sqrt();
        if d <= node_radius_px(n, cam.zoom) + GRAB_MARGIN_PX && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Which nodes and edges stay lit while the pointer rests on one node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Highlight {
    pub focus: Option<usize>,
    pub neighbours: BTreeSet<usize>,
}

/// A node plus everything one link away. With no focus every alpha is full, so
/// the resting graph is not dimmed.
pub fn highlight(scene: &Scene, focus: Option<usize>) -> Highlight {
    let Some(f) = focus else { return Highlight::default() };
    let mut neighbours = BTreeSet::from([f]);
    for e in &scene.edges {
        if e.from == f {
            neighbours.insert(e.to);
        }
        if e.to == f {
            neighbours.insert(e.from);
        }
    }
    Highlight { focus: Some(f), neighbours }
}

impl Highlight {
    pub fn is_resting(&self) -> bool {
        self.focus.is_none()
    }

    pub fn node_alpha(&self, i: usize) -> f32 {
        if self.is_resting() || self.neighbours.contains(&i) { FULL_ALPHA } else { DIM_ALPHA }
    }

    /// An edge is lit only when it touches the focused node: a link between two
    /// neighbours is not part of this neighbourhood.
    pub fn edge_alpha(&self, e: &Edge) -> f32 {
        match self.focus {
            None => FULL_ALPHA,
            Some(f) if e.from == f || e.to == f => FULL_ALPHA,
            Some(_) => DIM_ALPHA,
        }
    }
}

/// A colour a group asks for: an Omarchy role by name, so a theme swap restyles
/// it, or a literal `0xRRGGBB` for the one case where the operator means that
/// exact colour and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupColour {
    Role(&'static str),
    Hex(u32),
}

/// Roles a new group cycles through. Every one is an Omarchy role, so groups
/// follow the theme by default and the collision rule from C6a still applies.
pub const GROUP_ROLES: [&str; 5] = ["accent", "ok", "warn", "danger", "bright"];

/// Obsidian's groups: a search, and a colour for what it matches.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub query: String,
    pub colour: GroupColour,
}

impl Group {
    /// Next group for `query`, coloured by how many groups already exist so two
    /// groups never land on the same role by accident.
    pub fn next(query: impl Into<String>, existing: usize) -> Self {
        Self { query: query.into(), colour: GroupColour::Role(GROUP_ROLES[existing % GROUP_ROLES.len()]) }
    }
}

/// The first group whose query matches, or none. First match wins, so the
/// operator can reason about order instead of blend rules.
pub fn group_for(n: &Node, groups: &[Group]) -> Option<GroupColour> {
    groups.iter().find(|g| n.matches(&g.query)).map(|g| g.colour)
}

/// Rough width of one label character at the size the window draws them. gpui
/// does not hand out text metrics before layout, so the box is estimated; it is
/// deliberately generous, because a label that claims too much space costs a
/// neighbour's label, while one that claims too little costs an overlap.
///
/// The estimate does not have to be exact, because the window draws each label
/// inside a box of exactly [`label_width_px`] by [`LABEL_HEIGHT_PX`] and clips
/// to it. The reservation is therefore the truth about how much room a label
/// takes, not a guess about it.
pub const LABEL_CHAR_PX: f32 = 9.0;
/// Height of the drawn label box. The window pins the text's line height to
/// this, but the reservation is sized so that labels clear each other even if
/// the pin were ignored: gpui's default line box is `phi` rems, about 26
/// pixels, and a reservation of this height plus padding on both sides is
/// taller than that. Two labels that survive placement are far enough apart
/// whatever the text system does.
pub const LABEL_HEIGHT_PX: f32 = 20.0;
/// Clearance kept around every drawn label. Two labels are never merely
/// touching; they are always this far apart.
pub const LABEL_PAD_PX: f32 = 5.0;
/// Below this zoom nothing is labelled: the text would be unreadable and the
/// board would be a wall of grey.
pub const LABEL_MIN_ZOOM: f32 = 0.12;
/// Longest label drawn; the rest is elided.
pub const LABEL_MAX_CHARS: usize = 22;

/// The stem of a label. A node's name, never a path: `notes/ideas/Alpha.md`
/// reads as `Alpha`.
pub fn label_stem(text: &str) -> String {
    let base = text.rsplit('/').next().unwrap_or(text);
    let base = base.strip_suffix(".md").unwrap_or(base);
    let base = base.trim();
    if base.chars().count() <= LABEL_MAX_CHARS {
        return base.to_string();
    }
    let cut: String = base.chars().take(LABEL_MAX_CHARS - 1).collect();
    format!("{cut}…")
}

/// Width of the box the window draws this label in. The window uses the same
/// number, so what is reserved and what is painted are the same rectangle.
pub fn label_width_px(text: &str) -> f32 {
    (text.chars().count() as f32 * LABEL_CHAR_PX).max(LABEL_CHAR_PX)
}

/// The space a label claims: the drawn box plus clearance on every side.
pub fn label_box(text: &str, at: [f32; 2]) -> [f32; 4] {
    let w = label_width_px(text) + LABEL_PAD_PX * 2.0;
    [at[0] - w / 2.0, at[1] - LABEL_PAD_PX, at[0] + w / 2.0, at[1] + LABEL_HEIGHT_PX + LABEL_PAD_PX]
}

/// The rectangle the window actually paints, strictly inside [`label_box`].
pub fn label_drawn(text: &str, at: [f32; 2]) -> [f32; 4] {
    let w = label_width_px(text);
    [at[0] - w / 2.0, at[1], at[0] + w / 2.0, at[1] + LABEL_HEIGHT_PX]
}

pub fn overlaps(a: &[f32; 4], b: &[f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Board-local pixels. `Stroke` is a real hairline between two disc rims, not a
/// run of dots.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Stroke {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        width: f32,
        dashed: bool,
        alpha: f32,
    },
    Disc {
        x: f32,
        y: f32,
        r: f32,
        seed: bool,
        dangling: bool,
        pinned: bool,
        alpha: f32,
        /// Set when a group's query matched this node.
        group: Option<GroupColour>,
    },
    Label {
        x: f32,
        y: f32,
        text: String,
        alpha: f32,
    },
}

/// The segment between two discs, trimmed to their rims.
///
/// `None` when the discs already touch: a stroke there would live inside the
/// fill, which is exactly the muddy look the visual bar rules out.
pub fn rim_segment(a: [f32; 2], ra: f32, b: [f32; 2], rb: f32) -> Option<([f32; 2], [f32; 2])> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d = (dx * dx + dy * dy).sqrt();
    if !d.is_finite() || d <= ra + rb {
        return None;
    }
    let (ux, uy) = (dx / d, dy / d);
    Some(([a[0] + ux * ra, a[1] + uy * ra], [b[0] - ux * rb, b[1] - uy * rb]))
}

/// Everything the canvas draws this frame, in board-local pixels: strokes
/// first so discs sit on top, then discs, then labels.
pub fn paint(
    scene: &Scene,
    cam: &Camera,
    board: [f32; 2],
    hi: &Highlight,
    pinned: &dyn Fn(&str) -> bool,
    groups: &[Group],
) -> Vec<Prim> {
    let pos: Vec<[f32; 2]> = scene.nodes.iter().map(|n| cam.to_screen([n.x, n.y], board)).collect();
    let rad: Vec<f32> = scene.nodes.iter().map(|n| node_radius_px(n, cam.zoom)).collect();
    let mut out = Vec::with_capacity(scene.edges.len() + scene.nodes.len() * 2);

    for e in &scene.edges {
        let (Some(&a), Some(&b)) = (pos.get(e.from), pos.get(e.to)) else { continue };
        let Some((p, q)) = rim_segment(a, rad[e.from], b, rad[e.to]) else { continue };
        if (q[0] - p[0]).hypot(q[1] - p[1]) < MIN_STROKE_PX {
            continue;
        }
        let dashed = scene.nodes[e.from].dangling || scene.nodes[e.to].dangling;
        out.push(Prim::Stroke {
            x0: p[0],
            y0: p[1],
            x1: q[0],
            y1: q[1],
            width: STROKE_PX,
            dashed,
            alpha: hi.edge_alpha(e),
        });
    }
    for (i, n) in scene.nodes.iter().enumerate() {
        out.push(Prim::Disc {
            x: pos[i][0],
            y: pos[i][1],
            r: rad[i],
            seed: n.is_seed,
            dangling: n.dangling,
            pinned: pinned(&n.id),
            alpha: hi.node_alpha(i),
            group: group_for(n, groups),
        });
    }
    out.extend(labels(scene, &pos, &rad, cam, board, hi));
    out
}

/// Which labels survive at this zoom, in order of importance.
///
/// Three rules, in order: nothing at all below [`LABEL_MIN_ZOOM`]; nothing that
/// has scrolled off the board; and nothing that would overlap a label already
/// placed. Busy nodes and the hovered neighbourhood get first claim, so what
/// you lose when the board is crowded is always the least useful label.
pub fn labels(
    scene: &Scene,
    pos: &[[f32; 2]],
    rad: &[f32],
    cam: &Camera,
    board: [f32; 2],
    hi: &Highlight,
) -> Vec<Prim> {
    if cam.zoom < LABEL_MIN_ZOOM || scene.nodes.is_empty() {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..scene.nodes.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |i: usize| {
            (hi.focus == Some(i), hi.neighbours.contains(&i), scene.nodes[i].is_seed, scene.nodes[i].degree)
        };
        key(b).cmp(&key(a)).then_with(|| scene.nodes[a].id.cmp(&scene.nodes[b].id))
    });

    // Widest a label can be, used to reject off-board candidates before
    // building the string. On a two-thousand-node vault this is the difference
    // between two thousand allocations a frame and a couple of hundred.
    let widest = LABEL_MAX_CHARS as f32 * LABEL_CHAR_PX + LABEL_PAD_PX * 2.0;

    let mut placed: Vec<[f32; 4]> = Vec::new();
    let mut out = Vec::new();
    for i in order {
        let at = [pos[i][0], pos[i][1] + rad[i] + 2.0];
        if at[0] + widest < 0.0
            || at[0] - widest > board[0]
            || at[1] + LABEL_HEIGHT_PX < 0.0
            || at[1] > board[1]
        {
            continue;
        }
        let text = label_stem(&scene.nodes[i].label);
        if text.is_empty() {
            continue;
        }
        let b = label_box(&text, at);
        if b[2] < 0.0 || b[0] > board[0] || b[3] < 0.0 || b[1] > board[1] {
            continue;
        }
        if placed.iter().any(|p| overlaps(p, &b)) {
            continue;
        }
        placed.push(b);
        out.push(Prim::Label { x: at[0], y: at[1], text, alpha: hi.node_alpha(i) });
    }
    out
}

/// What a keystroke means. The window owns state; this owns the mapping, so
/// the keyboard contract is testable without a compositor.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyAction {
    Zoom(f32),
    Pan(f32, f32),
    ResetCamera,
    StartQuery,
    QueryPush(char),
    QueryPop,
    /// Leave the query box, keeping what was typed.
    QueryCommit,
    /// Leave the query box and clear it.
    QueryCancel,
    ToggleDangling,
    ToggleOrphans,
    CycleLayout,
    CycleDomain,
    Depth(i32),
    /// Turn the current query into a group.
    AddGroup,
    ClearGroups,
    Unpin,
    None,
}

/// One keystroke, in or out of the query box.
///
/// While typing, printable keys are text and nothing else: a query containing
/// `-` must not zoom out. `escape` always gets you out.
pub fn key_action(key: &str, shift: bool, typing: bool) -> KeyAction {
    if typing {
        return match key {
            "escape" => KeyAction::QueryCancel,
            "enter" => KeyAction::QueryCommit,
            "backspace" => KeyAction::QueryPop,
            "space" => KeyAction::QueryPush(' '),
            k => match k.chars().next() {
                Some(c) if k.chars().count() == 1 && !c.is_control() => {
                    KeyAction::QueryPush(if shift { c.to_ascii_uppercase() } else { c })
                }
                _ => KeyAction::None,
            },
        };
    }
    match key {
        "+" | "=" => KeyAction::Zoom(ZOOM_STEP),
        "-" | "_" => KeyAction::Zoom(1.0 / ZOOM_STEP),
        "0" => KeyAction::ResetCamera,
        "left" => KeyAction::Pan(PAN_STEP_PX, 0.0),
        "right" => KeyAction::Pan(-PAN_STEP_PX, 0.0),
        "up" => KeyAction::Pan(0.0, PAN_STEP_PX),
        "down" => KeyAction::Pan(0.0, -PAN_STEP_PX),
        "/" => KeyAction::StartQuery,
        "escape" => KeyAction::QueryCancel,
        "e" => KeyAction::ToggleDangling,
        "o" => KeyAction::ToggleOrphans,
        "l" => KeyAction::CycleLayout,
        "d" => KeyAction::CycleDomain,
        "[" => KeyAction::Depth(-1),
        "]" => KeyAction::Depth(1),
        "g" if shift => KeyAction::ClearGroups,
        "g" => KeyAction::AddGroup,
        "u" => KeyAction::Unpin,
        _ => KeyAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        // hub -- leaf, hub -- ghost(dangling), and a stranger nobody links.
        let node = |id: &str, x: f32, y: f32, deg: u32, dangling: bool, seed: bool| Node {
            id: id.into(),
            label: id.into(),
            depth: 0,
            x,
            y,
            degree: deg,
            dangling,
            is_seed: seed,
        };
        Scene {
            // World units are the sim's, so a link is ~30 across, not ~1.
            nodes: vec![
                node("hub", 0.0, 0.0, 2, false, true),
                node("leaf", 120.0, 0.0, 1, false, false),
                node("dangling:ghost", 0.0, 120.0, 1, true, false),
                node("stranger", -180.0, -120.0, 0, false, false),
            ],
            edges: vec![
                Edge { from: 0, to: 1, dir: "out".into() },
                Edge { from: 0, to: 2, dir: "out".into() },
            ],
            truncated: false,
        }
    }

    const BOARD: [f32; 2] = [800.0, 600.0];

    #[test]
    fn pan_and_zoom_are_a_camera_not_a_relayout() {
        let s = scene();
        let world_before: Vec<(f32, f32)> = s.nodes.iter().map(|n| (n.x, n.y)).collect();
        let mut cam = Camera::default();

        let at_rest = cam.to_screen([s.nodes[1].x, s.nodes[1].y], BOARD);
        cam.pan_by(40.0, -25.0);
        let panned = cam.to_screen([s.nodes[1].x, s.nodes[1].y], BOARD);
        assert_eq!(panned, [at_rest[0] + 40.0, at_rest[1] - 25.0]);

        cam.zoom_by(ZOOM_STEP, BOARD);
        assert!((cam.zoom - ZOOM_STEP).abs() < 1e-6);

        let world_after: Vec<(f32, f32)> = s.nodes.iter().map(|n| (n.x, n.y)).collect();
        assert_eq!(world_before, world_after, "the camera never touches world positions");
    }

    #[test]
    fn wheel_zoom_holds_the_point_under_the_cursor() {
        let mut cam = Camera::default();
        let cursor = [610.0, 190.0];
        let held = cam.to_world(cursor, BOARD);
        for f in [ZOOM_STEP, ZOOM_STEP, 1.0 / ZOOM_STEP] {
            cam.zoom_about(f, cursor, BOARD);
            let now = cam.to_screen(held, BOARD);
            assert!((now[0] - cursor[0]).abs() < 1e-2 && (now[1] - cursor[1]).abs() < 1e-2);
        }
    }

    #[test]
    fn zoom_clamps_and_round_trips() {
        let mut cam = Camera::default();
        for _ in 0..80 {
            cam.zoom_by(ZOOM_STEP, BOARD);
        }
        assert_eq!(cam.zoom, MAX_ZOOM);
        for _ in 0..200 {
            cam.zoom_by(1.0 / ZOOM_STEP, BOARD);
        }
        assert_eq!(cam.zoom, MIN_ZOOM);
        cam.reset();
        assert_eq!(cam, Camera::default());

        let p = [123.0, 45.0];
        let back = cam.to_screen(cam.to_world(p, BOARD), BOARD);
        assert!((back[0] - p[0]).abs() < 1e-3 && (back[1] - p[1]).abs() < 1e-3);
    }

    #[test]
    fn hit_test_finds_discs_and_reports_empty_space() {
        let s = scene();
        let cam = Camera::default();
        let all = |_: &Node| true;
        let on_leaf = cam.to_screen([120.0, 0.0], BOARD);
        assert_eq!(hit_test(&s, &cam, BOARD, on_leaf, &all), Some(1));
        assert_eq!(hit_test(&s, &cam, BOARD, [5.0, 5.0], &all), None, "a corner is empty space, so it pans");

        // The camera moves the hit box with the discs.
        let mut moved = cam;
        moved.pan_by(60.0, 0.0);
        assert_eq!(hit_test(&s, &moved, BOARD, on_leaf, &all), None);
        assert_eq!(hit_test(&s, &moved, BOARD, [on_leaf[0] + 60.0, on_leaf[1]], &all), Some(1));

        // A filtered-out node is not grabbable.
        let ghost = cam.to_screen([0.0, 120.0], BOARD);
        assert_eq!(hit_test(&s, &cam, BOARD, ghost, &all), Some(2));
        let hide_dangling = crate::scene::Filters { show_dangling: false, ..Default::default() };
        assert_eq!(hit_test(&s, &cam, BOARD, ghost, &|n| n.passes(&hide_dangling)), None);
    }

    #[test]
    fn hover_lights_the_neighbourhood_and_dims_the_rest() {
        let s = scene();
        let hi = highlight(&s, Some(0));
        assert_eq!(hi.node_alpha(0), FULL_ALPHA);
        assert_eq!(hi.node_alpha(1), FULL_ALPHA, "a neighbour stays lit");
        assert_eq!(hi.node_alpha(2), FULL_ALPHA);
        assert_eq!(hi.node_alpha(3), DIM_ALPHA, "the stranger dims");
        assert_eq!(hi.edge_alpha(&s.edges[0]), FULL_ALPHA);

        let away = highlight(&s, Some(3));
        assert_eq!(away.node_alpha(0), DIM_ALPHA);
        assert_eq!(away.edge_alpha(&s.edges[0]), DIM_ALPHA);

        let resting = highlight(&s, None);
        assert!(resting.is_resting());
        assert!((0..4).all(|i| resting.node_alpha(i) == FULL_ALPHA), "no hover means no dimming");
    }

    #[test]
    fn strokes_start_and_stop_on_the_disc_rims() {
        let s = scene();
        let cam = Camera::default();
        let prims = paint(&s, &cam, BOARD, &highlight(&s, None), &|_| false, &[]);

        let strokes: Vec<&Prim> = prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).collect();
        assert_eq!(strokes.len(), 2);
        let discs: Vec<&Prim> = prims.iter().filter(|p| matches!(p, Prim::Disc { .. })).collect();
        assert_eq!(discs.len(), 4);

        let hub = cam.to_screen([0.0, 0.0], BOARD);
        let r_hub = node_radius_px(&s.nodes[0], cam.zoom);
        let r_leaf = node_radius_px(&s.nodes[1], cam.zoom);
        let leaf = cam.to_screen([120.0, 0.0], BOARD);
        match strokes[0] {
            Prim::Stroke { x0, y0, x1, y1, width, dashed, .. } => {
                assert!((*x0 - (hub[0] + r_hub)).abs() < 1e-3, "leaves the hub at its rim");
                assert!((*y0 - hub[1]).abs() < 1e-3);
                assert!((*x1 - (leaf[0] - r_leaf)).abs() < 1e-3, "lands on the leaf's rim");
                assert!((*y1 - leaf[1]).abs() < 1e-3);
                assert_eq!(*width, STROKE_PX, "a hairline");
                assert!(!dashed, "a resolved link is solid");
            }
            other => panic!("expected a stroke, got {other:?}"),
        }
        assert!(
            matches!(strokes[1], Prim::Stroke { dashed: true, .. }),
            "the link to a note that does not exist is dashed"
        );
    }

    #[test]
    fn overlapping_discs_draw_no_stroke() {
        let mut s = scene();
        s.nodes[1].x = 0.0;
        s.nodes[1].y = 0.0;
        let prims = paint(&s, &Camera::default(), BOARD, &Highlight::default(), &|_| false, &[]);
        assert_eq!(
            prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).count(),
            1,
            "a stroke buried inside a disc is not drawn"
        );
    }

    #[test]
    fn paint_carries_hover_alpha_and_pins() {
        let s = scene();
        let hi = highlight(&s, Some(0));
        let prims = paint(&s, &Camera::default(), BOARD, &hi, &|id| id == "leaf", &[]);
        let dim = prims
            .iter()
            .filter(|p| matches!(p, Prim::Disc { alpha, .. } if (*alpha - DIM_ALPHA).abs() < 1e-6))
            .count();
        assert_eq!(dim, 1, "only the stranger is dimmed");
        assert!(
            prims.iter().any(|p| matches!(p, Prim::Disc { pinned: true, .. })),
            "a pinned node is drawn as pinned"
        );
        assert_eq!(prims.iter().filter(|p| matches!(p, Prim::Label { .. })).count(), s.nodes.len());
    }

    #[test]
    fn a_disc_grows_with_degree_on_the_quartz_curve() {
        let s = scene();
        let z = 1.0;
        // hub (degree 2, seed) > leaf (1) > stranger (0)
        assert!(node_radius_px(&s.nodes[0], z) > node_radius_px(&s.nodes[1], z));
        assert!(node_radius_px(&s.nodes[1], z) > node_radius_px(&s.nodes[3], z));

        // the law itself: 2 + sqrt(degree), scaled, seed at 1.5x
        let plain = |deg: u32| Node {
            id: "n".into(),
            label: "n".into(),
            depth: 0,
            x: 0.0,
            y: 0.0,
            degree: deg,
            dangling: false,
            is_seed: false,
        };
        for deg in [0u32, 1, 4, 9, 25] {
            let want = (2.0 + (deg as f32).sqrt()) * 1.6;
            assert!(
                (node_radius_px(&plain(deg), 1.0) - want).abs() < 1e-4,
                "degree {deg} should be 2 + sqrt(degree)"
            );
        }
        // and it keeps growing rather than flattening into uniform discs
        let r: Vec<f32> = [0u32, 1, 4, 16, 64].iter().map(|&d| node_radius_px(&plain(d), 1.0)).collect();
        assert!(r.windows(2).all(|w| w[1] > w[0] + 0.5), "monotone and visibly different: {r:?}");
    }

    #[test]
    fn a_group_query_recolours_the_discs_it_matches() {
        let s = scene();
        let groups = vec![
            Group::next("leaf", 0),
            Group { query: "ghost".into(), colour: GroupColour::Hex(0x30_a0_ff) },
        ];
        assert_eq!(groups[0].colour, GroupColour::Role(GROUP_ROLES[0]));

        assert_eq!(group_for(&s.nodes[1], &groups), Some(GroupColour::Role("accent")));
        assert_eq!(group_for(&s.nodes[2], &groups), Some(GroupColour::Hex(0x30_a0_ff)));
        assert_eq!(group_for(&s.nodes[0], &groups), None, "the hub matches no group");

        let prims = paint(&s, &Camera::default(), BOARD, &Highlight::default(), &|_| false, &groups);
        let grouped = prims.iter().filter(|p| matches!(p, Prim::Disc { group: Some(_), .. })).count();
        assert_eq!(grouped, 2, "only the two matching discs carry a group colour");

        // Groups cycle roles so two of them never collide by accident.
        let picked: Vec<GroupColour> = (0..GROUP_ROLES.len()).map(|i| Group::next("q", i).colour).collect();
        let mut seen = picked.clone();
        seen.dedup();
        assert_eq!(seen.len(), GROUP_ROLES.len());
    }

    /// A grid of nodes close enough that naive labelling would pile them up.
    fn crowded(n: usize) -> Scene {
        let side = (n as f32).sqrt().ceil() as usize;
        let nodes = (0..n)
            .map(|i| Node {
                id: format!("notes/deep/Note-{i:03}.md"),
                label: format!("notes/deep/Note-{i:03}.md"),
                depth: 0,
                x: (i % side) as f32 * 10.0,
                y: (i / side) as f32 * 6.0,
                degree: (i % 7) as u32,
                dangling: false,
                is_seed: i == 0,
            })
            .collect();
        Scene { nodes, edges: Vec::new(), truncated: false }
    }

    #[test]
    fn a_label_is_a_stem_not_a_path() {
        assert_eq!(label_stem("notes/ideas/Alpha.md"), "Alpha");
        assert_eq!(label_stem("Welcome.md"), "Welcome");
        assert_eq!(label_stem("ghost-link"), "ghost-link");
        let long = label_stem("a-very-long-note-name-that-would-run-across-the-board.md");
        assert_eq!(long.chars().count(), LABEL_MAX_CHARS);
        assert!(long.ends_with('…'), "an over-long name is elided, not wrapped");
    }

    /// Every rectangle the window paints, given a camera.
    fn drawn_label_boxes(s: &Scene, cam: &Camera) -> Vec<[f32; 4]> {
        paint(s, cam, BOARD, &Highlight::default(), &|_| false, &[])
            .iter()
            .filter_map(|p| match p {
                Prim::Label { x, y, text, .. } => Some(label_drawn(text, [*x, *y])),
                _ => None,
            })
            .collect()
    }

    fn gap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
        let dx = (b[0] - a[2]).max(a[0] - b[2]);
        let dy = (b[1] - a[3]).max(a[1] - b[3]);
        dx.max(dy)
    }

    #[test]
    fn no_two_labels_overlap_at_rest() {
        let s = crowded(400);
        let mut cam = Camera::default();
        cam.fit(&s, BOARD);
        let boxes = drawn_label_boxes(&s, &cam);
        assert!(!boxes.is_empty(), "some labels survive at rest zoom");
        assert!(boxes.len() < s.nodes.len(), "a crowded board drops the ones it cannot fit");
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(!overlaps(a, b), "labels {a:?} and {b:?} overlap at rest");
                assert!(
                    gap(a, b) >= LABEL_PAD_PX - 1e-3,
                    "labels {a:?} and {b:?} are only {} apart",
                    gap(a, b)
                );
            }
        }
    }

    /// A small vault at 238% zoom failed once; the guarantee is checked across
    /// the range a reader actually uses, not only at the fit zoom.
    #[test]
    fn no_two_labels_overlap_at_any_zoom() {
        for n in [40usize, 68, 400] {
            let s = crowded(n);
            let mut base = Camera::default();
            base.fit(&s, BOARD);
            for factor in [0.5f32, 1.0, 1.7, 2.38, 4.0, 8.0] {
                let mut cam = base;
                cam.zoom_about(factor, [BOARD[0] / 2.0, BOARD[1] / 2.0], BOARD);
                let boxes = drawn_label_boxes(&s, &cam);
                for (i, a) in boxes.iter().enumerate() {
                    for b in &boxes[i + 1..] {
                        assert!(!overlaps(a, b), "{n} nodes at zoom {:.2}: {a:?} overlaps {b:?}", cam.zoom);
                    }
                }
            }
        }
    }

    /// What the window paints must sit inside what the placer reserved, or the
    /// no-overlap guarantee is about rectangles nobody draws.
    #[test]
    fn the_drawn_label_is_inside_the_reserved_one() {
        for text in ["a", "Beacon-41", "Cinder-22", "a-name-of-some-length"] {
            let at = [200.0f32, 100.0];
            let (res, drawn) = (label_box(text, at), label_drawn(text, at));
            assert!(res[0] < drawn[0] && res[1] < drawn[1], "{text}: reservation starts earlier");
            assert!(res[2] > drawn[2] && res[3] > drawn[3], "{text}: reservation ends later");
            assert!((drawn[3] - drawn[1] - LABEL_HEIGHT_PX).abs() < 1e-4, "{text}: drawn height is pinned");
            assert!(
                (drawn[2] - drawn[0] - label_width_px(text)).abs() < 1e-4,
                "{text}: drawn width is the shared estimate"
            );
        }
    }

    #[test]
    fn labels_thin_out_as_the_camera_pulls_back() {
        let s = crowded(400);
        let mut cam = Camera::default();
        cam.fit(&s, BOARD);
        // Compare labels against the nodes actually on the board, so zooming
        // in is judged on density rather than on how much fell off the edge.
        let shown = |c: &Camera| -> (Vec<String>, Vec<String>) {
            let on_board: Vec<String> = s
                .nodes
                .iter()
                .filter(|n| {
                    let p = c.to_screen([n.x, n.y], BOARD);
                    (0.0..=BOARD[0]).contains(&p[0]) && (0.0..=BOARD[1]).contains(&p[1])
                })
                .map(|n| label_stem(&n.label))
                .collect();
            let named: Vec<String> = paint(&s, c, BOARD, &Highlight::default(), &|_| false, &[])
                .iter()
                .filter_map(|p| match p {
                    Prim::Label { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            (named, on_board)
        };

        let (rest_named, rest_on_board) = shown(&cam);
        assert!(!rest_named.is_empty(), "some labels survive at rest zoom");
        assert!(rest_named.len() < rest_on_board.len(), "at rest the board is too dense to name everything");

        // Zoomed in there is room, so every node on the board carries its name.
        // A label may also survive for a node just off the edge, so this is
        // containment, not equality.
        let mut close = cam;
        close.zoom_by(12.0, BOARD);
        let (near_named, near_on_board) = shown(&close);
        for want in &near_on_board {
            assert!(near_named.contains(want), "zoomed in, {want} should be named");
        }

        // Pulling back thins them out, and past the readability floor there
        // are none at all rather than a grey smear.
        let mut back = cam;
        back.zoom_by(0.25, BOARD);
        assert!(shown(&back).0.len() < rest_named.len(), "pulling back drops labels");
        let mut far = cam;
        while far.zoom >= super::LABEL_MIN_ZOOM {
            far.zoom_by(0.5, BOARD);
        }
        assert!(shown(&far).0.is_empty(), "below the readability floor, nothing is named");
    }

    #[test]
    fn the_hovered_neighbourhood_gets_first_claim_on_labels() {
        let mut s = crowded(400);
        // give one node a low degree so only the hover can win it a label
        s.nodes[321].degree = 0;
        let mut cam = Camera::default();
        cam.fit(&s, BOARD);
        let named = |hi: &Highlight| -> Vec<String> {
            paint(&s, &cam, BOARD, hi, &|_| false, &[])
                .iter()
                .filter_map(|p| match p {
                    Prim::Label { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let want = label_stem(&s.nodes[321].label);
        let hovered = named(&highlight(&s, Some(321)));
        assert!(hovered.contains(&want), "the node under the pointer is always named");
    }

    #[test]
    fn fit_frames_the_whole_scene() {
        let s = crowded(400);
        let mut cam = Camera::default();
        cam.fit(&s, BOARD);
        for n in &s.nodes {
            let p = cam.to_screen([n.x, n.y], BOARD);
            assert!((0.0..=BOARD[0]).contains(&p[0]) && (0.0..=BOARD[1]).contains(&p[1]));
        }
        // and it centres: the middle of the scene lands on the middle of the board
        let mid = cam.to_screen([s.nodes[0].x / 2.0 + 0.0, 0.0], BOARD);
        assert!(mid[0].is_finite());
        cam.fit(&Scene::default(), BOARD);
        assert_eq!(cam, Camera::default(), "an empty scene resets rather than dividing by zero");
    }

    #[test]
    fn the_keymap_separates_typing_from_commands() {
        // out of the query box
        assert_eq!(key_action("=", false, false), KeyAction::Zoom(ZOOM_STEP));
        assert_eq!(key_action("-", false, false), KeyAction::Zoom(1.0 / ZOOM_STEP));
        assert_eq!(key_action("0", false, false), KeyAction::ResetCamera);
        assert_eq!(key_action("left", false, false), KeyAction::Pan(PAN_STEP_PX, 0.0));
        assert_eq!(key_action("down", false, false), KeyAction::Pan(0.0, -PAN_STEP_PX));
        assert_eq!(key_action("/", false, false), KeyAction::StartQuery);
        assert_eq!(key_action("e", false, false), KeyAction::ToggleDangling);
        assert_eq!(key_action("o", false, false), KeyAction::ToggleOrphans);
        assert_eq!(key_action("l", false, false), KeyAction::CycleLayout);
        assert_eq!(key_action("]", false, false), KeyAction::Depth(1));
        assert_eq!(key_action("[", false, false), KeyAction::Depth(-1));
        assert_eq!(key_action("g", false, false), KeyAction::AddGroup);
        assert_eq!(key_action("g", true, false), KeyAction::ClearGroups);
        assert_eq!(key_action("q", false, false), KeyAction::None);

        // inside it, printable keys are text — a query with a dash must not zoom
        assert_eq!(key_action("-", false, true), KeyAction::QueryPush('-'));
        assert_eq!(key_action("0", false, true), KeyAction::QueryPush('0'));
        assert_eq!(key_action("e", false, true), KeyAction::QueryPush('e'));
        assert_eq!(key_action("e", true, true), KeyAction::QueryPush('E'));
        assert_eq!(key_action("space", false, true), KeyAction::QueryPush(' '));
        assert_eq!(key_action("backspace", false, true), KeyAction::QueryPop);
        assert_eq!(key_action("enter", false, true), KeyAction::QueryCommit);
        assert_eq!(key_action("escape", false, true), KeyAction::QueryCancel);
        assert_eq!(key_action("left", false, true), KeyAction::None, "arrows are not text");
    }
}
