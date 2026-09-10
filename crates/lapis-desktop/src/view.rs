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

/// Scene units to pixels at zoom 1.
pub const UNIT: f32 = 120.0;
/// Non-neighbours during hover. Quartz dims to this and so do we.
pub const DIM_ALPHA: f32 = 0.2;
pub const FULL_ALPHA: f32 = 1.0;
pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 8.0;
/// One `+` / `-` press, and one wheel line.
pub const ZOOM_STEP: f32 = 1.2;
/// A press that travels further than this is a drag, not a click.
pub const CLICK_SLOP_PX: f32 = 4.0;
/// Extra pixels around a disc that still count as grabbing it.
pub const GRAB_MARGIN_PX: f32 = 4.0;
/// Edge stroke width. A hairline, not a bar.
pub const STROKE_PX: f32 = 1.0;
/// Dash pattern for a link that resolves to nothing.
pub const DASH_PX: [f32; 2] = [4.0, 3.0];

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
}

/// Disc radius in pixels. Quartz's `2 + sqrt(degree)`, so a hub reads as a hub,
/// with the zoom contribution damped so a far-out view still has visible nodes.
pub fn node_radius_px(n: &Node, zoom: f32) -> f32 {
    let base = 2.0 + (n.degree as f32).sqrt();
    let base = if n.is_seed { base * 1.5 } else { base };
    (base * 1.6 * zoom.clamp(0.5, 2.0)).clamp(3.0, 44.0)
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

/// Board-local pixels. `Stroke` is a real hairline between two disc rims, not a
/// run of dots.
#[derive(Debug, Clone, PartialEq)]
pub enum Prim {
    Stroke { x0: f32, y0: f32, x1: f32, y1: f32, width: f32, dashed: bool, alpha: f32 },
    Disc { x: f32, y: f32, r: f32, seed: bool, dangling: bool, pinned: bool, alpha: f32 },
    Label { x: f32, y: f32, text: String, alpha: f32 },
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
) -> Vec<Prim> {
    let pos: Vec<[f32; 2]> = scene.nodes.iter().map(|n| cam.to_screen([n.x, n.y], board)).collect();
    let rad: Vec<f32> = scene.nodes.iter().map(|n| node_radius_px(n, cam.zoom)).collect();
    let mut out = Vec::with_capacity(scene.edges.len() + scene.nodes.len() * 2);

    for e in &scene.edges {
        let (Some(&a), Some(&b)) = (pos.get(e.from), pos.get(e.to)) else { continue };
        let Some((p, q)) = rim_segment(a, rad[e.from], b, rad[e.to]) else { continue };
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
        });
    }
    for (i, n) in scene.nodes.iter().enumerate() {
        out.push(Prim::Label {
            x: pos[i][0],
            y: pos[i][1] + rad[i] + 2.0,
            text: n.label.clone(),
            alpha: hi.node_alpha(i),
        });
    }
    out
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
            nodes: vec![
                node("hub", 0.0, 0.0, 2, false, true),
                node("leaf", 1.0, 0.0, 1, false, false),
                node("dangling:ghost", 0.0, 1.0, 1, true, false),
                node("stranger", -1.5, -1.0, 0, false, false),
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
        let on_leaf = cam.to_screen([1.0, 0.0], BOARD);
        assert_eq!(hit_test(&s, &cam, BOARD, on_leaf, &all), Some(1));
        assert_eq!(hit_test(&s, &cam, BOARD, [5.0, 5.0], &all), None, "a corner is empty space, so it pans");

        // The camera moves the hit box with the discs.
        let mut moved = cam;
        moved.pan_by(60.0, 0.0);
        assert_eq!(hit_test(&s, &moved, BOARD, on_leaf, &all), None);
        assert_eq!(hit_test(&s, &moved, BOARD, [on_leaf[0] + 60.0, on_leaf[1]], &all), Some(1));

        // A filtered-out node is not grabbable.
        let ghost = cam.to_screen([0.0, 1.0], BOARD);
        assert_eq!(hit_test(&s, &cam, BOARD, ghost, &all), Some(2));
        assert_eq!(hit_test(&s, &cam, BOARD, ghost, &|n| n.passes(false, None)), None);
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
        let prims = paint(&s, &cam, BOARD, &highlight(&s, None), &|_| false);

        let strokes: Vec<&Prim> = prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).collect();
        assert_eq!(strokes.len(), 2);
        let discs: Vec<&Prim> = prims.iter().filter(|p| matches!(p, Prim::Disc { .. })).collect();
        assert_eq!(discs.len(), 4);

        let hub = cam.to_screen([0.0, 0.0], BOARD);
        let r_hub = node_radius_px(&s.nodes[0], cam.zoom);
        let r_leaf = node_radius_px(&s.nodes[1], cam.zoom);
        let leaf = cam.to_screen([1.0, 0.0], BOARD);
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
        let prims = paint(&s, &Camera::default(), BOARD, &Highlight::default(), &|_| false);
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
        let prims = paint(&s, &Camera::default(), BOARD, &hi, &|id| id == "leaf");
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
    fn a_disc_grows_with_degree() {
        let s = scene();
        let z = 1.0;
        assert!(node_radius_px(&s.nodes[0], z) > node_radius_px(&s.nodes[3], z));
    }
}
