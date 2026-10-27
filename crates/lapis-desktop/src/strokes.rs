//! Hairlines a frame can afford. Every link is clipped to the board before it
//! is drawn, and unresolved links are dashed only while the frame has room.
//!
//! Each hairline segment becomes two triangles of GPU instance data, about
//! 720 bytes, and the renderer drops any frame past its 256 MiB instance cap.
//! Twenty thousand notes with thousands of long unresolved links once needed
//! 480 MiB a frame, so the board froze on whatever it last drew.

/// Most hairline segments, dashes included, one frame draws.
pub const SEGMENT_BUDGET: usize = 120_000;

/// Most dash segments one frame draws. Past it, an unresolved link is drawn
/// solid: at that density nobody can tell a dash from a line anyway.
pub const DASH_BUDGET: usize = 60_000;

// At about 720 bytes a segment, a full frame stays under half the renderer's cap.
const _: () = assert!(SEGMENT_BUDGET * 720 < 128 * 1024 * 1024 && DASH_BUDGET <= SEGMENT_BUDGET);

/// Instance bytes one batched small disc costs: four triangles, three vertices
/// each, at the same per-vertex size as a hairline's.
pub const DISC_BYTES: usize = 4 * 3 * 120;

// The whole frame's worst case: every segment the budget allows plus a
// batched disc for every node the snapshot can hold, under half the renderer's
// 256 MiB cap.
const _: () =
    assert!(SEGMENT_BUDGET * 720 + lapis_lattice::SNAPSHOT_NODE_CAP * DISC_BYTES < 128 * 1024 * 1024);

/// The part of segment `s` (`[x0, y0, x1, y1]`) inside the board grown by
/// `margin` on every side, or `None` when none of it is (Liang–Barsky).
pub fn clip(s: [f32; 4], board: [f32; 2], margin: f32) -> Option<[f32; 4]> {
    let (x0, y0, dx, dy) = (s[0], s[1], s[2] - s[0], s[3] - s[1]);
    let (lo_x, lo_y, hi_x, hi_y) = (-margin, -margin, board[0] + margin, board[1] + margin);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [(-dx, x0 - lo_x), (dx, hi_x - x0), (-dy, y0 - lo_y), (dy, hi_y - y0)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            t0 = t0.max(r);
        } else {
            t1 = t1.min(r);
        }
        if t0 > t1 {
            return None;
        }
    }
    Some([x0 + dx * t0, y0 + dy * t0, x0 + dx * t1, y0 + dy * t1])
}

/// How many dash runs a segment of `len` pixels costs.
pub fn dash_count(len: f32, on: f32, off: f32) -> usize {
    if !len.is_finite() || len <= 0.0 { 0 } else { (len / (on + off).max(0.5)).ceil() as usize }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: [f32; 2] = [100.0, 50.0];

    #[test]
    fn a_segment_inside_the_board_is_untouched() {
        assert_eq!(clip([10.0, 10.0, 90.0, 40.0], BOARD, 0.0), Some([10.0, 10.0, 90.0, 40.0]));
    }

    #[test]
    fn a_segment_crossing_the_board_is_cut_to_it() {
        let c = clip([-100.0, 25.0, 200.0, 25.0], BOARD, 0.0).unwrap();
        assert_eq!(c, [0.0, 25.0, 100.0, 25.0]);
        let m = clip([-100.0, 25.0, 200.0, 25.0], BOARD, 4.0).unwrap();
        assert_eq!(m, [-4.0, 25.0, 104.0, 25.0], "the margin keeps line ends under the board's edge");
    }

    #[test]
    fn a_segment_wholly_off_the_board_is_dropped() {
        assert_eq!(clip([-50.0, -50.0, -10.0, -5.0], BOARD, 0.0), None);
        assert_eq!(clip([0.0, 80.0, 100.0, 90.0], BOARD, 0.0), None, "below the board");
        assert_eq!(clip([120.0, 0.0, 120.0, 50.0], BOARD, 0.0), None, "vertical, to the right");
        assert_eq!(clip([-30.0, 60.0, 30.0, 120.0], BOARD, 0.0), None, "diagonal past a corner");
    }

    #[test]
    fn dash_counts_follow_length_and_survive_bad_input() {
        assert_eq!(dash_count(70.0, 4.0, 3.0), 10);
        assert_eq!(dash_count(0.0, 4.0, 3.0), 0);
        assert_eq!(dash_count(f32::INFINITY, 4.0, 3.0), 0);
        assert_eq!(dash_count(f32::NAN, 4.0, 3.0), 0);
    }
}
