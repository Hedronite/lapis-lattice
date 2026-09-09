//! Lapis palette: blue ground, cream ink, regent grey chrome, gold accents.
//!
//! Named palettes are switchable at runtime (`Space z t`); the active one lives
//! in a process-wide slot so the draw code keeps calling `theme::chrome()` etc.
//! `lapis` is the default and its constants are the brand: BLUE `#1F2D68`,
//! REGENT `#809DAF`. A `[theme]` table in config can pick a name or override
//! individual colors with `#RRGGBB` values.

use std::sync::RwLock;

use ratatui::style::{Color, Modifier, Style};

pub const BLUE: Color = Color::Rgb(0x1F, 0x2D, 0x68);
pub const BLUE_DEEP: Color = Color::Rgb(0x16, 0x20, 0x4C);
pub const BLUE_SOFT: Color = Color::Rgb(0x2A, 0x3C, 0x84);
pub const REGENT: Color = Color::Rgb(0x80, 0x9D, 0xAF);
pub const CREAM: Color = Color::Rgb(0xF3, 0xE9, 0xD2);
pub const GOLD: Color = Color::Rgb(0xD9, 0xB8, 0x5C);
pub const COPPER: Color = Color::Rgb(0xC8, 0x7F, 0x4A);
pub const MUTED: Color = Color::Rgb(0x5C, 0x6E, 0x9C);
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
    blue: BLUE,
    blue_deep: BLUE_DEEP,
    blue_soft: BLUE_SOFT,
    regent: REGENT,
    cream: CREAM,
    gold: GOLD,
    copper: COPPER,
    muted: MUTED,
    ok: OK,
    warn: WARN,
};

/// Light ground, ink text; same gold/copper accents.
pub const PARCHMENT: Palette = Palette {
    name: "parchment",
    blue: Color::Rgb(0xF3, 0xE9, 0xD2),
    blue_deep: Color::Rgb(0xE6, 0xDA, 0xBE),
    blue_soft: Color::Rgb(0xE0, 0xD2, 0xB0),
    regent: Color::Rgb(0x4E, 0x62, 0x86),
    cream: Color::Rgb(0x1F, 0x2D, 0x68),
    gold: Color::Rgb(0x9A, 0x74, 0x1A),
    copper: Color::Rgb(0xA3, 0x5B, 0x2A),
    muted: Color::Rgb(0x8C, 0x8A, 0x7E),
    ok: Color::Rgb(0x3E, 0x7D, 0x3A),
    warn: Color::Rgb(0xB0, 0x5E, 0x1E),
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
    muted: Color::Rgb(0x5A, 0x60, 0x70),
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
    Style::default().fg(current().regent)
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
    Style::default().fg(current().regent).add_modifier(Modifier::UNDERLINED)
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
    fn lapis_palette_constants() {
        assert_eq!(BLUE, Color::Rgb(0x1F, 0x2D, 0x68));
        assert_eq!(REGENT, Color::Rgb(0x80, 0x9D, 0xAF));
        assert_eq!(CREAM, Color::Rgb(0xF3, 0xE9, 0xD2));
        assert_eq!(GOLD, Color::Rgb(0xD9, 0xB8, 0x5C));
        // the default palette IS the brand
        assert_eq!(LAPIS.blue, BLUE);
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
        assert_eq!(p.blue, BLUE, "bad value ignored");
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
