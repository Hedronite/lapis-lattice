//! Lapis palette: quiet navy ground, cream ink, restrained brand accents.
//!
//! Named palettes are switchable at runtime (`Space z t`); the active one lives
//! in a process-wide slot so the draw code keeps calling `theme::chrome()` etc.
//! `lapis` is the default and its constants are the brand: BLUE `#1F2D68`,
//! REGENT `#809DAF`. A `[theme]` table in config can pick a name or override
//! individual colors with `#RRGGBB` values.

use std::sync::RwLock;

use ratatui::style::{Color, Modifier, Style};

pub const BLUE: Color = Color::Rgb(0x1F, 0x2D, 0x68);
pub const REGENT: Color = Color::Rgb(0x80, 0x9D, 0xAF);
pub const CREAM: Color = Color::Rgb(0xF3, 0xE9, 0xD2);
pub const GOLD: Color = Color::Rgb(0xD9, 0xB8, 0x5C);
pub const COPPER: Color = Color::Rgb(0xC8, 0x7F, 0x4A);
pub const OK: Color = Color::Rgb(0x8F, 0xC9, 0x8A);
pub const WARN: Color = Color::Rgb(0xE8, 0x9C, 0x5A);

/// The ten roles every palette fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub blue: Color,
    pub blue_deep: Color,
    pub blue_soft: Color,
    pub regent: Color,
    pub cream: Color,
    pub gold: Color,
    pub copper: Color,
    pub muted: Color,
    pub ok: Color,
    pub warn: Color,
}

pub const LAPIS: Palette = Palette {
    name: "lapis",
    blue: Color::Rgb(0x12, 0x17, 0x22),
    blue_deep: Color::Rgb(0x0D, 0x11, 0x1A),
    blue_soft: Color::Rgb(0x24, 0x34, 0x52),
    regent: REGENT,
    cream: CREAM,
    gold: GOLD,
    copper: COPPER,
    muted: Color::Rgb(0x83, 0x93, 0xA8),
    ok: OK,
    warn: WARN,
};

/// Light ground, ink text; same gold/copper accents.
pub const PARCHMENT: Palette = Palette {
    name: "parchment",
    blue: Color::Rgb(0xF3, 0xE9, 0xD2),
    blue_deep: Color::Rgb(0xE6, 0xDA, 0xBE),
    blue_soft: Color::Rgb(0xE0, 0xD2, 0xB0),
    regent: Color::Rgb(0x44, 0x57, 0x79),
    cream: BLUE,
    gold: Color::Rgb(0x82, 0x61, 0x11),
    copper: Color::Rgb(0x91, 0x4D, 0x22),
    muted: Color::Rgb(0x62, 0x66, 0x61),
    ok: Color::Rgb(0x32, 0x6D, 0x30),
    warn: Color::Rgb(0x8F, 0x45, 0x13),
};

/// Near-black ground for dim rooms; regent and gold kept.
pub const OBSIDIAN: Palette = Palette {
    name: "obsidian",
    blue: Color::Rgb(0x10, 0x12, 0x18),
    blue_deep: Color::Rgb(0x0A, 0x0B, 0x10),
    blue_soft: Color::Rgb(0x1E, 0x22, 0x30),
    regent: REGENT,
    cream: Color::Rgb(0xE8, 0xE4, 0xDA),
    gold: GOLD,
    copper: COPPER,
    muted: Color::Rgb(0x87, 0x90, 0xA4),
    ok: OK,
    warn: WARN,
};

/// Every built-in, in switch order.
pub const PALETTES: [Palette; 3] = [LAPIS, PARCHMENT, OBSIDIAN];

pub fn named(name: &str) -> Option<Palette> {
    PALETTES.iter().copied().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// `#RRGGBB` (or `RRGGBB`) → color.
pub fn parse_hex(s: &str) -> Option<Color> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

impl Palette {
    /// Apply `key = "#RRGGBB"` overrides; unknown keys and bad values are ignored.
    pub fn with_overrides<'a>(mut self, pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        for (k, v) in pairs {
            let Some(c) = parse_hex(v) else { continue };
            match k {
                "blue" => self.blue = c,
                "blue_deep" => self.blue_deep = c,
                "blue_soft" => self.blue_soft = c,
                "regent" => self.regent = c,
                "cream" => self.cream = c,
                "gold" => self.gold = c,
                "copper" => self.copper = c,
                "muted" => self.muted = c,
                "ok" => self.ok = c,
                "warn" => self.warn = c,
                _ => {}
            }
        }
        if self != named(self.name).unwrap_or(LAPIS) {
            self.name = "custom";
        }
        self
    }

    /// The palette after this one in [`PALETTES`] (custom → lapis).
    pub fn next(&self) -> Palette {
        let i = PALETTES.iter().position(|p| p == self).map(|i| (i + 1) % PALETTES.len()).unwrap_or(0);
        PALETTES[i]
    }
}

static CURRENT: RwLock<Palette> = RwLock::new(LAPIS);

/// Make `p` the active palette for every style function.
pub fn install(p: Palette) {
    if let Ok(mut w) = CURRENT.write() {
        *w = p;
    }
}

pub fn current() -> Palette {
    CURRENT.read().map(|p| *p).unwrap_or(LAPIS)
}

pub fn base() -> Style {
    let p = current();
    Style::default().bg(p.blue).fg(p.cream)
}
pub fn chrome() -> Style {
    Style::default().fg(current().muted)
}
pub fn focused() -> Style {
    Style::default().fg(current().gold)
}
pub fn dim() -> Style {
    Style::default().fg(current().muted)
}
pub fn accent() -> Style {
    Style::default().fg(current().gold).add_modifier(Modifier::BOLD)
}
pub fn selected() -> Style {
    let p = current();
    Style::default().bg(p.blue_soft).fg(p.cream).add_modifier(Modifier::BOLD)
}
pub fn statusline() -> Style {
    let p = current();
    Style::default().bg(p.blue_deep).fg(p.cream)
}
pub fn overlay() -> Style {
    let p = current();
    Style::default().bg(p.blue_deep).fg(p.cream)
}
pub fn code() -> Style {
    let p = current();
    Style::default().bg(p.blue_deep).fg(p.regent)
}
pub fn heading(level: u8) -> Style {
    let p = current();
    match level {
        1 => Style::default().fg(p.gold).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default().fg(p.gold).add_modifier(Modifier::BOLD),
        3 => Style::default().fg(p.copper).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(p.regent).add_modifier(Modifier::BOLD),
    }
}
pub fn link() -> Style {
    Style::default().fg(current().muted).add_modifier(Modifier::UNDERLINED)
}
pub fn mode(mode: &str) -> Style {
    let p = current();
    let bg = match mode {
        "INSERT" => p.ok,
        "VISUAL" => p.gold,
        "REPLACE" | "OPERATOR" => p.warn,
        _ => p.regent,
    };
    Style::default().bg(bg).fg(p.blue_deep).add_modifier(Modifier::BOLD)
}
/// Role colors of the active palette, for call sites that used the constants directly.
pub fn gold() -> Color {
    current().gold
}
pub fn cream() -> Color {
    current().cream
}
pub fn warn() -> Color {
    current().warn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_reading_roles_have_legible_contrast() {
        let luminance = |color: Color| {
            let Color::Rgb(r, g, b) = color else { panic!("built-in colors must be explicit RGB") };
            [r, g, b]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(v, weight)| {
                    let v = f64::from(v) / 255.0;
                    (if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }) * weight
                })
                .sum::<f64>()
        };
        for p in PALETTES {
            let pairs = [
                (p.cream, p.blue),
                (p.regent, p.blue),
                (p.muted, p.blue),
                (p.gold, p.blue),
                (p.copper, p.blue),
                (p.ok, p.blue),
                (p.warn, p.blue),
                (p.cream, p.blue_soft),
                (p.regent, p.blue_deep),
            ];
            for (ink, ground) in pairs {
                let (a, b) = (luminance(ink), luminance(ground));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(ratio >= 4.5, "{} {ink:?} on {ground:?}: {ratio:.2}", p.name);
            }
        }
    }

    #[test]
    fn lapis_palette_constants() {
        assert_eq!(BLUE, Color::Rgb(0x1F, 0x2D, 0x68));
        assert_eq!(REGENT, Color::Rgb(0x80, 0x9D, 0xAF));
        assert_eq!(CREAM, Color::Rgb(0xF3, 0xE9, 0xD2));
        assert_eq!(GOLD, Color::Rgb(0xD9, 0xB8, 0x5C));
        // Canonical brand pigment stays intact; working surfaces are quieter.
        assert_ne!(LAPIS.blue, BLUE);
        assert_eq!(LAPIS.regent, REGENT);
        assert_eq!(named("lapis"), Some(LAPIS));
        assert_eq!(named("LAPIS "), Some(LAPIS));
        assert_eq!(named("neon"), None);
        assert_eq!(LAPIS.next().name, "parchment");
        assert_eq!(OBSIDIAN.next().name, "lapis");
    }

    /// N21: hex parsing and `[theme.custom]` overrides; defaults survive untouched.
    #[test]
    fn hex_and_overrides() {
        assert_eq!(parse_hex("#1F2D68"), Some(BLUE));
        assert_eq!(parse_hex("809daf"), Some(REGENT));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
        let p = LAPIS.with_overrides([("gold", "#FFFFFF"), ("nope", "#000000"), ("blue", "bad")]);
        assert_eq!(p.gold, Color::Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.blue, LAPIS.blue, "bad value ignored");
        assert_eq!(p.name, "custom");
        assert_eq!(
            LAPIS.with_overrides([("cream", "#F3E9D2")]).name,
            "lapis",
            "no-op override keeps the name"
        );
        // constants are untouched by overrides or installs
        assert_eq!(BLUE, Color::Rgb(0x1F, 0x2D, 0x68));
        assert_eq!(REGENT, Color::Rgb(0x80, 0x9D, 0xAF));
    }
}
