//! Easing on the board: the camera glides to a new framing, and a finished
//! layout slides into its final places instead of snapping there.
use crate::view::Camera;

/// How long a finished layout takes to slide from the last unfolding frame
/// into its separated, final positions.
pub const SETTLE_GLIDE_MS: f32 = 320.0;

/// Share of the remaining distance the camera covers each frame.
pub const CAMERA_GLIDE: f32 = 0.18;

/// Smoothstep: eases in and out over `0..=1`, clamped outside it.
pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Positions `t` of the way from `from` to `to`. The two are index-parallel; a
/// node `from` does not have starts at its destination.
pub fn blend(from: &[[f32; 2]], to: &[[f32; 2]], t: f32) -> Vec<[f32; 2]> {
    to.iter()
        .enumerate()
        .map(|(i, b)| match from.get(i) {
            Some(a) => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
            None => *b,
        })
        .collect()
}

/// Move `cam` a step `k` of the way toward `to`: zoom by ratio, so a glide
/// across a big change in scale looks even, and pan linearly. Returns false
/// once it has arrived, after snapping exactly onto `to`.
pub fn camera_toward(cam: &mut Camera, to: &Camera, k: f32) -> bool {
    let ratio = to.zoom / cam.zoom.max(f32::EPSILON);
    if (ratio - 1.0).abs() < 0.002 && (to.pan_x - cam.pan_x).abs() < 0.5 && (to.pan_y - cam.pan_y).abs() < 0.5
    {
        *cam = *to;
        return false;
    }
    cam.zoom *= ratio.powf(k);
    cam.pan_x += (to.pan_x - cam.pan_x) * k;
    cam.pan_y += (to.pan_y - cam.pan_y) * k;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_is_clamped_and_symmetric() {
        assert_eq!(smooth(-1.0), 0.0);
        assert_eq!(smooth(2.0), 1.0);
        assert!((smooth(0.5) - 0.5).abs() < 1e-6);
        assert!(smooth(0.1) < 0.1, "eases in");
        assert!(smooth(0.9) > 0.9, "eases out");
    }

    #[test]
    fn blend_runs_from_start_to_end_and_new_nodes_start_home() {
        let from = [[0.0, 0.0]];
        let to = [[10.0, -4.0], [3.0, 3.0]];
        assert_eq!(blend(&from, &to, 0.0), vec![[0.0, 0.0], [3.0, 3.0]]);
        assert_eq!(blend(&from, &to, 0.5), vec![[5.0, -2.0], [3.0, 3.0]]);
        assert_eq!(blend(&from, &to, 1.0), to.to_vec());
    }

    #[test]
    fn the_camera_arrives_and_stops() {
        let mut cam = Camera { zoom: 0.05, pan_x: 0.0, pan_y: 0.0 };
        let to = Camera { zoom: 0.8, pan_x: 120.0, pan_y: -40.0 };
        let mut frames = 0;
        while camera_toward(&mut cam, &to, CAMERA_GLIDE) {
            frames += 1;
            assert!(frames < 200, "a glide ends");
        }
        assert_eq!(cam, to, "it lands exactly");
        assert!(frames > 5, "and it glided rather than jumped");
        assert!(!camera_toward(&mut cam, &to, CAMERA_GLIDE), "an arrived camera stays put");
    }
}
