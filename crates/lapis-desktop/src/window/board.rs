//! The board's pixels: a lapis glow and a quiet starfield behind hairline edges and
//! discs. Every colour is read from the live theme or rotated from its accent, so a
//! theme swap restyles the graph with no restart.
use super::*;
use crate::graph_style::{FOLDER_HUES, STAR_PARALLAX, STARS, star};
use crate::strokes;

/// What one frame of the board needs, captured so the canvas can paint it.
pub(super) struct Board {
    pub prims: Vec<Prim>,
    /// Scene index of the hovered or selected node. Its links are drawn in gold.
    pub focus: Option<usize>,
    /// Folder colour slot for each scene node, if its folder is coloured.
    pub folder: Vec<Option<usize>>,
    pub pan: [f32; 2],
    pub theme: gpui_omarchy::Theme,
    pub show_boxes: bool,
    pub origin: Rc<Cell<(f32, f32)>>,
    pub board_px: Rc<Cell<(f32, f32)>>,
    pub draw_ms: Rc<Cell<f32>>,
}

/// A folder's colour: the accent's saturation and lightness at another hue.
pub(super) fn folder_hsla(theme: &gpui_omarchy::Theme, slot: usize) -> Hsla {
    let mut c = theme.accent;
    c.h = (c.h + FOLDER_HUES[slot % FOLDER_HUES.len()]).fract();
    // Soft and a little light: the folders are a quiet map, not a palette swatch.
    c.s = (c.s * 0.6).clamp(0.25, 0.5);
    c.l = c.l.clamp(0.62, 0.72);
    c
}

/// Edge buckets: (solid | dashed) × (lit | dimmed | hot). Each is one path.
const BUCKETS: usize = 6;

/// Up to this many drawn links, resting links get full ink; past it, ink falls
/// with the square root of the count, down to a quarter.
const EDGE_INK_FULL: f32 = 1_500.0;

/// Up to this many lit links around a focused note keep their dashes and full
/// gold; past it they are drawn solid, and their ink falls with the count.
const HOT_DASHED_MAX: usize = 40;

impl Board {
    pub(super) fn paint(self, bounds: Bounds<Pixels>, window: &mut Window) {
        let t_draw = Instant::now();
        let theme = &self.theme;
        self.origin.set((f32::from(bounds.origin.x), f32::from(bounds.origin.y)));
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        self.board_px.set((w, h));
        let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));

        // A deep lapis glow behind the middle of the board, as stacked soft discs.
        // Many faint layers rather than a few strong ones, so no band edge shows.
        let reach = w.max(h) * 0.62;
        for k in 0..16 {
            let r = reach * (1.0 - k as f32 * 0.055);
            window.paint_quad(dot(at(w / 2.0, h / 2.0), r, with_alpha(theme.selection, 0.045)));
        }
        // Stars drift a little with a pan, so the void has depth without moving on its own.
        for i in 0..STARS {
            let [sx, sy, b] = star(i);
            let x = (sx * w + self.pan[0] * STAR_PARALLAX).rem_euclid(w.max(1.0));
            let y = (sy * h + self.pan[1] * STAR_PARALLAX).rem_euclid(h.max(1.0));
            window.paint_quad(dot(at(x, y), 0.5 + b * 0.6, with_alpha(theme.foreground, 0.05 + b * 0.22)));
        }

        // A hub's gold fan: hundreds of converging dashes shimmer, so past a handful
        // the lit links are drawn solid and thinner in ink, like the resting ones.
        let lit = if self.focus.is_some() {
            self.prims
                .iter()
                .filter(|p| matches!(p, Prim::Stroke { alpha, .. } if *alpha >= FULL_ALPHA))
                .count()
        } else {
            0
        };
        let fan = lit > HOT_DASHED_MAX;
        let [on, off] = crate::view::DASH_PX;
        let (mut used, mut dashed_used) = (0usize, 0usize);
        let mut batch: [Vec<[f32; 4]>; BUCKETS] = Default::default();
        for p in &self.prims {
            let Prim::Stroke { x0, y0, x1, y1, dashed, alpha, .. } = p else { continue };
            // Only what is on the board is drawn: a link that runs off it is cut at
            // the edge, and one that never crosses it costs nothing.
            let Some(seg) = strokes::clip([*x0, *y0, *x1, *y1], [w, h], 2.0) else { continue };
            if used >= strokes::SEGMENT_BUDGET {
                break;
            }
            let tone = if *alpha < FULL_ALPHA {
                1
            } else if self.focus.is_some() {
                2
            } else {
                0
            };
            let cost = strokes::dash_count((seg[2] - seg[0]).hypot(seg[3] - seg[1]), on, off);
            let dashed = *dashed && !(fan && tone == 2) && dashed_used + cost <= strokes::DASH_BUDGET;
            let slot = tone * 2 + usize::from(dashed);
            if dashed {
                dashes(seg, on, off, &mut batch[slot]);
                (used, dashed_used) = (used + cost, dashed_used + cost);
            } else {
                batch[slot].push(seg);
                used += 1;
            }
        }
        // Thousands of hairlines at full ink wash a big vault's core white, so the
        // resting links thin out as their count grows; a focus still lights its own.
        let strokes = self.prims.iter().filter(|p| matches!(p, Prim::Stroke { .. })).count();
        let ink = 0.16 * (EDGE_INK_FULL / strokes.max(1) as f32).sqrt().clamp(0.25, 1.0);
        for (slot, segs) in batch.iter().enumerate() {
            let (colour, width) = match slot / 2 {
                0 => (with_alpha(theme.foreground, ink), crate::view::STROKE_PX),
                1 => (with_alpha(theme.border, DIM_ALPHA * 2.5), crate::view::STROKE_PX),
                _ => {
                    let hot = 0.8 * (HOT_DASHED_MAX as f32 / lit.max(1) as f32).sqrt().clamp(0.35, 1.0);
                    (with_alpha(theme.accent, hot), crate::view::STROKE_PX * if fan { 1.0 } else { 1.4 })
                }
            };
            if let Some(path) = segments_path(segs, width, bounds.origin) {
                window.paint_path(path, colour);
            }
        }

        if self.show_boxes {
            // The rectangle each label reserved, so a screenshot shows whether the
            // drawn text stays inside its claim.
            for p in &self.prims {
                if let Prim::Label { x, y, text, .. } = p {
                    let b = crate::view::label_box(text, [*x, *y]);
                    let colour = with_alpha(theme.accent, 0.45);
                    window.paint_quad(outline(b[0], b[1], b[2] - b[0], b[3] - b[1], bounds.origin, colour));
                }
            }
        }

        for p in &self.prims {
            let Prim::Disc { x, y, r, seed, dangling, pinned, alpha, group, node } = p else { continue };
            // At rest discs are slightly translucent, so overlapping clusters show their
            // structure; a focused neighbourhood paints solid.
            let (x, y, r) = (*x, *y, *r);
            let a = if self.focus.is_some() { *alpha } else { *alpha * 0.85 };
            // A group wins over everything: the operator asked for that colour by query.
            let colour = match (group, self.folder.get(*node).copied().flatten()) {
                (Some(g), _) => group_hsla(*g, theme),
                (None, _) if *dangling => with_alpha(theme.secondary, 0.6),
                (None, _) if *seed => theme.accent,
                (None, Some(slot)) => folder_hsla(theme, slot),
                // Notes outside the coloured folders recede, so the map reads first.
                (None, None) => with_alpha(theme.foreground, 0.55),
            };
            let focused = self.focus == Some(*node);
            if focused {
                window.paint_quad(dot(at(x, y), r * 3.2, with_alpha(theme.accent, 0.10)));
                window.paint_quad(dot(at(x, y), r * 2.0, with_alpha(theme.accent, 0.16)));
            } else if r >= 8.0 || *seed {
                // Hubs glow faintly in their own colour, so the busy notes read first.
                window.paint_quad(dot(at(x, y), r * 2.2, with_alpha(colour, 0.06 * a)));
            }
            if *pinned {
                // A ring around a pinned disc, so a held node is legible without a tooltip.
                window.paint_quad(dot(at(x, y), r + 3.0, with_alpha(theme.accent, a * 0.35)));
            }
            if *dangling {
                // An unresolved link is a hollow ring: there is no note inside it.
                window.paint_quad(ring(at(x, y), r, 1.0, with_alpha(colour, a * 0.75)));
            } else {
                window.paint_quad(dot(at(x, y), r, with_alpha(colour, a)));
            }
            if focused {
                window.paint_quad(ring(at(x, y), r + 3.0, 1.5, theme.accent));
            }
        }
        self.draw_ms.set(t_draw.elapsed().as_secs_f32() * 1000.0);
    }
}

/// A hollow circle `width` pixels thick.
fn ring(at: Point<Pixels>, r: f32, width: f32, colour: Hsla) -> PaintQuad {
    let mut q = dot(at, r, colour);
    q.background = gpui_kit::transparent_black().into();
    q.border_widths = Edges::all(px(width));
    q.border_color = colour;
    q
}
