//! Pure decisions behind the graph's look: which folders get a colour, where the
//! background stars sit. The window turns these into theme colours and pixels.
use crate::scene::Scene;

/// Folders coloured on the board. The rest share the default node colour.
pub const FOLDER_COLOURS: usize = 6;

/// Hue offsets, in turns from the theme's accent, for the coloured folders. From the
/// Lapis gold these land on sky blue, teal, lavender, sage, rose and sand: the cool
/// side of the wheel first, so the accent stays the warm note on the board.
pub const FOLDER_HUES: [f32; FOLDER_COLOURS] = [0.47, 0.37, 0.61, 0.19, 0.84, 0.03];

/// The top-level folder a note lives in, or `None` for a note at the vault root
/// and for an unresolved link, which lives nowhere.
pub fn folder_of(id: &str) -> Option<&str> {
    if id.starts_with("dangling:") {
        return None;
    }
    id.split_once('/').map(|(top, _)| top).filter(|top| !top.is_empty())
}

/// The folders with the most notes in `scene`, busiest first (ties by name), at
/// most [`FOLDER_COLOURS`] of them. Their order is their colour.
pub fn top_folders(scene: &Scene) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for n in &scene.nodes {
        if let Some(folder) = folder_of(&n.id) {
            *counts.entry(folder).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().map(|(f, c)| (f.to_string(), c)).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(FOLDER_COLOURS);
    ranked
}

/// Background stars on the board: how many, and how far they drift with a pan.
pub const STARS: usize = 220;
pub const STAR_PARALLAX: f32 = 0.06;

/// Star `i`: x and y in `0..1` of the board, and a brightness in `0..1`. A fixed
/// hash, so the sky is the same every frame and every launch.
pub fn star(i: usize) -> [f32; 3] {
    let mut h = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    let mut next = || {
        h ^= h >> 33;
        h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        h ^= h >> 29;
        (h >> 40) as f32 / (1u64 << 24) as f32
    };
    let (x, y, b) = (next(), next(), next());
    [x, y, b * b]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Node;

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            label: id.into(),
            depth: 0,
            x: 0.,
            y: 0.,
            degree: 0,
            dangling: false,
            is_seed: false,
        }
    }

    #[test]
    fn busiest_folders_lead_and_root_notes_and_dangling_links_have_none() {
        let mut scene = Scene::default();
        for id in
            ["notes/a.md", "notes/b.md", "projects/c.md", "top.md", "dangling:ghost", "a/x.md", "b/y.md"]
        {
            scene.nodes.push(node(id));
        }
        let top = top_folders(&scene);
        assert_eq!(top[0], ("notes".to_string(), 2));
        assert_eq!(top.iter().map(|(f, _)| f.as_str()).collect::<Vec<_>>(), ["notes", "a", "b", "projects"]);
        assert_eq!(folder_of("top.md"), None);
        assert_eq!(folder_of("dangling:notes/x"), None);
        assert_eq!(folder_of("notes/deep/x.md"), Some("notes"));
    }

    #[test]
    fn at_most_six_folders_are_coloured() {
        let mut scene = Scene::default();
        for f in 0..10 {
            scene.nodes.push(node(&format!("f{f}/n.md")));
        }
        assert_eq!(top_folders(&scene).len(), FOLDER_COLOURS);
    }

    #[test]
    fn stars_are_stable_and_inside_the_board() {
        assert_eq!(star(7), star(7));
        assert_ne!(star(7), star(8));
        for i in 0..STARS {
            let [x, y, b] = star(i);
            assert!((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y) && (0.0..=1.0).contains(&b));
        }
    }
}
