//! Lapis palette: blue ground, cream ink, regent grey chrome, gold accents.

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

pub fn base() -> Style {
    Style::default().bg(BLUE).fg(CREAM)
}
pub fn chrome() -> Style {
    Style::default().fg(REGENT)
}
pub fn focused() -> Style {
    Style::default().fg(GOLD)
}
pub fn dim() -> Style {
    Style::default().fg(MUTED)
}
pub fn accent() -> Style {
    Style::default().fg(GOLD).add_modifier(Modifier::BOLD)
}
pub fn selected() -> Style {
    Style::default().bg(BLUE_SOFT).fg(CREAM).add_modifier(Modifier::BOLD)
}
pub fn statusline() -> Style {
    Style::default().bg(BLUE_DEEP).fg(CREAM)
}
pub fn overlay() -> Style {
    Style::default().bg(BLUE_DEEP).fg(CREAM)
}
pub fn code() -> Style {
    Style::default().bg(BLUE_DEEP).fg(REGENT)
}
pub fn heading(level: u8) -> Style {
    match level {
        1 => Style::default().fg(GOLD).add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default().fg(GOLD).add_modifier(Modifier::BOLD),
        3 => Style::default().fg(COPPER).add_modifier(Modifier::BOLD),
        _ => Style::default().fg(REGENT).add_modifier(Modifier::BOLD),
    }
}
pub fn link() -> Style {
    Style::default().fg(REGENT).add_modifier(Modifier::UNDERLINED)
}
pub fn mode(mode: &str) -> Style {
    let bg = match mode {
        "INSERT" => OK,
        "VISUAL" => GOLD,
        "REPLACE" | "OPERATOR" => WARN,
        _ => REGENT,
    };
    Style::default().bg(bg).fg(BLUE_DEEP).add_modifier(Modifier::BOLD)
}
