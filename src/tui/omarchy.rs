//! Omarchy-native theming (C6a).
//!
//! Lapis is an Omarchy client, not a window with an optional Linux skin. When
//! `~/.local/state/omarchy/current/` exists we take the active theme's colours
//! and map them onto our ten palette roles; the brand palette is the fallback
//! for machines without Omarchy (macOS, vanilla Linux, CI).
//!
//! Three rules from SPEC-V02 §Theme that are easy to get wrong:
//!
//! * Read **exactly** `colors.toml`. A live theme directory also holds
//!   `colors.toml.bak-*`, and a glob would happily load a stale backup.
//! * Ignore `hyprland_*`. Those are `rgb()` / `rgba(RRGGBBAA)` function values
//!   belonging to the compositor, not `#RRGGBB`, and they are not ours to draw.
//! * After mapping, **no two roles may share a colour**. Themes reuse values
//!   (in `hedron`, `accent` and `orange` are both `#a96a38`), and two roles
//!   collapsing into one makes chrome indistinguishable from dim text. On a
//!   collision the higher-priority role keeps the colour and the lower falls
//!   back to brand — per role, never the whole palette.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ratatui::style::Color;

use super::theme::{LAPIS, Palette, parse_hex};

/// Where Omarchy publishes the active theme.
pub const STATE_REL: &str = ".local/state/omarchy/current";

/// Roles in the order they keep a contested colour. Earlier wins.
/// Mirrors SPEC-V02 §Theme so the two cannot drift silently.
const PRIORITY: [&str; 10] =
    ["cream", "blue", "gold", "warn", "ok", "copper", "blue_deep", "blue_soft", "regent", "muted"];

/// Palette role → the `colors.toml` key it reads, with a second choice used
/// only when the first is absent.
const MAP: [(&str, &str, Option<&str>); 10] = [
    ("blue", "background", None),
    ("blue_deep", "dark_background", Some("darker_background")),
    ("blue_soft", "selection", Some("lighter_background")),
    ("regent", "light_foreground", Some("foreground")),
    ("cream", "foreground", Some("bright_foreground")),
    ("gold", "accent", Some("yellow")),
    ("copper", "brown", Some("orange")),
    ("muted", "muted", Some("dark_foreground")),
    ("ok", "green", None),
    ("warn", "yellow", Some("red")),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// Contents of `theme.name`, for the status line and `Space z t`.
    pub name: String,
    /// `dark` or `light` when the theme declares it.
    pub mode: Option<String>,
    pub palette: Palette,
    /// Roles that fell back to brand, and why. Surfaced so a half-styled
    /// terminal is explainable rather than mysterious.
    pub fallbacks: Vec<(String, &'static str)>,
}

/// `~/.local/state/omarchy/current`, if a home directory is known.
pub fn state_root() -> Option<PathBuf> {
    std::env::home_dir().map(|h| h.join(STATE_REL))
}

/// Read `<root>/theme.name` and `<root>/theme/colors.toml`. `None` when this is
/// not an Omarchy box, which is the normal case on macOS and in CI.
pub fn load(root: &Path) -> Option<Loaded> {
    let colors = root.join("theme").join("colors.toml");
    if !colors.is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(&colors).ok()?;
    let name = std::fs::read_to_string(root.join("theme.name"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "omarchy".to_string());
    Some(from_toml(&name, &raw))
}

/// Parse and map. Separate from the filesystem so it is testable on any host.
pub fn from_toml(name: &str, raw: &str) -> Loaded {
    let table: toml::Table = raw.parse().unwrap_or_default();
    let mode = table.get("mode").and_then(|v| v.as_str()).map(str::to_string);

    // String scalars only, and never the compositor's own keys.
    let keys: BTreeMap<&str, &str> = table
        .iter()
        .filter(|(k, _)| !k.starts_with("hyprland_"))
        .filter_map(|(k, v)| v.as_str().map(|s| (k.as_str(), s)))
        .collect();

    let brand = LAPIS;
    let mut chosen: BTreeMap<&str, Color> = BTreeMap::new();
    let mut fallbacks: Vec<(String, &'static str)> = Vec::new();
    let mut taken: Vec<Color> = Vec::new();

    for role in PRIORITY {
        let (_, first, second) = MAP.iter().find(|(r, _, _)| *r == role).expect("role in MAP");
        let picked = keys
            .get(first)
            .and_then(|v| parse_hex(v))
            .or_else(|| second.and_then(|k| keys.get(k)).and_then(|v| parse_hex(v)));
        match picked {
            None => {
                chosen.insert(role, brand_role(&brand, role));
                fallbacks.push((role.to_string(), "no usable key in colors.toml"));
            }
            Some(c) if taken.contains(&c) => {
                // Another, higher-priority role already owns this colour.
                chosen.insert(role, brand_role(&brand, role));
                fallbacks.push((role.to_string(), "collided with a higher-priority role"));
            }
            Some(c) => {
                taken.push(c);
                chosen.insert(role, c);
            }
        }
    }

    let get = |r: &str| *chosen.get(r).unwrap_or(&brand_role(&brand, r));
    Loaded {
        name: name.to_string(),
        mode,
        palette: Palette {
            name: "omarchy",
            blue: get("blue"),
            blue_deep: get("blue_deep"),
            blue_soft: get("blue_soft"),
            regent: get("regent"),
            cream: get("cream"),
            gold: get("gold"),
            copper: get("copper"),
            muted: get("muted"),
            ok: get("ok"),
            warn: get("warn"),
        },
        fallbacks,
    }
}

fn brand_role(p: &Palette, role: &str) -> Color {
    match role {
        "blue" => p.blue,
        "blue_deep" => p.blue_deep,
        "blue_soft" => p.blue_soft,
        "regent" => p.regent,
        "cream" => p.cream,
        "gold" => p.gold,
        "copper" => p.copper,
        "muted" => p.muted,
        "ok" => p.ok,
        _ => p.warn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/omarchy/current")
    }

    #[test]
    fn loads_the_live_hedron_fixture() {
        let l = load(&fixture()).expect("fixture present");
        assert_eq!(l.name, "hedron");
        assert_eq!(l.mode.as_deref(), Some("dark"));
        let p = l.palette;
        assert_eq!(p.blue, parse_hex("#0a0e1e").unwrap(), "ground is background");
        assert_eq!(p.blue_deep, parse_hex("#080b18").unwrap());
        assert_eq!(p.blue_soft, parse_hex("#1c2450").unwrap(), "selection, not lighter_background");
        assert_eq!(p.cream, parse_hex("#eef1ff").unwrap(), "ink is foreground");
        assert_eq!(p.regent, parse_hex("#b7bfe8").unwrap(), "chrome is light_foreground");
        assert_eq!(p.muted, parse_hex("#8b93b8").unwrap());
        assert_eq!(p.gold, parse_hex("#a96a38").unwrap(), "accent");
        assert_eq!(p.copper, parse_hex("#75452a").unwrap(), "brown, not orange");
        assert_eq!(p.ok, parse_hex("#5a9e8a").unwrap());
        assert_eq!(p.warn, parse_hex("#e6bf6a").unwrap());
        assert!(l.fallbacks.is_empty(), "hedron fills every role: {:?}", l.fallbacks);

        // regent and muted are the pair that collapsed under the first draft of
        // the mapping table; they must stay distinct.
        assert_ne!(p.regent, p.muted, "chrome must not equal dim text");
    }

    /// The compositor's own colours are not `#RRGGBB` and are not ours to draw.
    /// Note `#a96a38` legitimately reaches `gold` through `accent`; what must
    /// not happen is a role *sourcing* from an `hyprland_*` key. So this gives
    /// a role no other source and checks it falls back rather than reaching for
    /// the compositor's value.
    #[test]
    fn hyprland_keys_are_never_a_source() {
        let raw = std::fs::read_to_string(fixture().join("theme/colors.toml")).unwrap();
        assert!(raw.contains("hyprland_active_border") && raw.contains("rgba("), "fixture keeps the trap");

        let only_hyprland = r##"
            background = "#101010"
            hyprland_active_border = "rgba(00ff00ee)"
            hyprland_inactive_border = "rgb(00ff00)"
        "##;
        let l = from_toml("hypr", only_hyprland);
        assert_eq!(l.palette.ok, LAPIS.ok, "green is absent, so ok falls back to brand");
        for c in [l.palette.gold, l.palette.ok, l.palette.warn, l.palette.copper, l.palette.regent] {
            assert_ne!(c, Color::Rgb(0x00, 0xff, 0x00), "no role sourced an hyprland value");
        }
        assert_eq!(l.palette.blue, parse_hex("#101010").unwrap(), "real keys still work");
    }

    /// Two roles resolving to one colour is the failure this rule exists for.
    #[test]
    fn collision_falls_back_per_role_by_priority() {
        // `muted` and `light_foreground` share a value, so regent and muted both
        // want #888888. `regent` outranks `muted`, so muted falls back to brand.
        let raw = r##"
            background = "#101010"
            dark_background = "#050505"
            selection = "#202020"
            foreground = "#ffffff"
            light_foreground = "#888888"
            muted = "#888888"
            accent = "#ffcc00"
            brown = "#884400"
            green = "#00ff00"
            yellow = "#ffff00"
        "##;
        let l = from_toml("collide", raw);
        assert_eq!(l.palette.regent, parse_hex("#888888").unwrap(), "higher priority keeps it");
        assert_eq!(l.palette.muted, LAPIS.muted, "lower priority falls back to brand");
        assert_ne!(l.palette.regent, l.palette.muted);
        assert!(l.fallbacks.iter().any(|(r, why)| r == "muted" && why.contains("collided")));
        // and only that role fell back
        assert_eq!(l.palette.blue, parse_hex("#101010").unwrap());
    }

    /// A missing key costs one role, never the whole palette.
    #[test]
    fn missing_role_falls_back_alone() {
        let raw = r##"
            background = "#101010"
            foreground = "#ffffff"
        "##;
        let l = from_toml("sparse", raw);
        assert_eq!(l.palette.blue, parse_hex("#101010").unwrap(), "present keys still win");
        assert_eq!(l.palette.cream, parse_hex("#ffffff").unwrap());
        assert_eq!(l.palette.ok, LAPIS.ok, "absent key falls back");
        assert!(l.fallbacks.iter().any(|(r, _)| r == "ok"));
        assert!(!l.fallbacks.iter().any(|(r, _)| r == "blue"));
    }

    #[test]
    fn absent_omarchy_tree_is_none() {
        assert!(load(Path::new("/nonexistent/omarchy/current")).is_none());
        // A directory with no colors.toml is also not an Omarchy theme.
        assert!(load(&PathBuf::from(env!("CARGO_MANIFEST_DIR"))).is_none());
    }
}
