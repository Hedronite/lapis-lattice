//! The Lapis palette: night navy, cream ink and a scarce gold, from the brand poster.
//!
//! An Omarchy desktop keeps its own theme. Everywhere else (macOS, a Linux box
//! without Omarchy) the window starts in Lapis rather than Tokyo Night.
use gpui_kit::base::ThemeAppearance;
use gpui_kit::{App, Hsla, rgb};
use gpui_omarchy::Theme;

/// Serif for the wordmark and headings. Unknown families fall back to the UI font.
pub(crate) const SERIF: &str = if cfg!(target_os = "macos") { "Georgia" } else { "serif" };

pub(crate) fn lapis() -> Theme {
    Theme {
        name: "Lapis".into(),
        appearance: ThemeAppearance::Dark,
        background: rgb(0x0b1122).into(),
        surface: rgb(0x111a31).into(),
        inset: rgb(0x080d1a).into(),
        foreground: rgb(0xd9d4c5).into(),
        secondary: rgb(0x8592a8).into(),
        bright: rgb(0xf4eedd).into(),
        accent: rgb(0xc9a45c).into(),
        on_accent: rgb(0x0b1122).into(),
        selection: rgb(0x1f2d68).into(),
        border: rgb(0x1d2742).into(),
        danger: rgb(0xe0817a).into(),
        warning: rgb(0xd9a54a).into(),
        success: rgb(0x8fb573).into(),
        font: ".SystemUIFont".into(),
    }
}

/// Lapis blue, the brand core, for selected rows and links in the field.
pub(crate) fn lapis_blue() -> Hsla {
    rgb(0x1f2d68).into()
}

fn omarchy_present() -> bool {
    std::env::var_os("HOME").is_some_and(|home| {
        let home = std::path::Path::new(&home);
        home.join(".local/state/omarchy/current").exists() || home.join(".config/omarchy/current").exists()
    })
}

/// After `gpui_omarchy::init`: keep a real Omarchy theme, otherwise apply Lapis.
/// `LAPIS_THEME=omarchy` keeps the Omarchy fallback; `LAPIS_THEME=lapis` forces Lapis.
pub(crate) fn init(cx: &mut App) {
    let choice = std::env::var("LAPIS_THEME").unwrap_or_default();
    let lapis_wanted = match choice.as_str() {
        "lapis" => true,
        "omarchy" => false,
        _ => !omarchy_present(),
    };
    if lapis_wanted {
        lapis().apply(cx);
    }
}

/// Source view: the editor sits on the page itself, with no box around it.
pub(crate) fn source_style(theme: &Theme) -> gpui_kit::base::input::InputEditorStyle {
    let mut style = theme.input_style();
    style.background = theme.background;
    style.border = theme.background;
    style.selection = theme.accent.opacity(0.22);
    style.caret = theme.accent;
    style.editor_active_line = Some(theme.foreground.opacity(0.03));
    style
}

/// Monospace for code. `monospace` is not a family name on macOS.
pub(crate) const MONO: &str = if cfg!(target_os = "macos") { "Menlo" } else { "monospace" };
