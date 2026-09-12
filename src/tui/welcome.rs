//! Quiet, cell-native welcome view derived from the canonical Lapis mark.

use super::theme;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    text::{Line, Span},
    widgets::Paragraph,
};

pub(crate) fn draw(f: &mut Frame, area: Rect, index_missing: bool) {
    let spacious = area.width >= 66 && area.height >= 21;
    let mut lines = Vec::new();
    if spacious {
        let mark = include_str!("../../assets/tui/mark.txt");
        for (i, line) in mark.lines().enumerate() {
            let text = match i {
                3 => "L A P I S",
                5 => "write · link · recall",
                8 => "A connected workspace.",
                _ => "",
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{line:<34}"), theme::accent()),
                Span::styled(text, theme::chrome()),
            ]));
        }
        lines.push(Line::default());
    } else {
        lines.push(Line::from(Span::styled("L A P I S", theme::accent())).alignment(Alignment::Center));
        lines.push(
            Line::from(Span::styled("write · link · recall", theme::dim())).alignment(Alignment::Center),
        );
        lines.push(Line::default());
    }
    let mut keys = vec![
        ("Enter", "open selected note"),
        ("Ctrl+P", "find a note"),
        ("Space n n", "new note"),
        ("Space / ?", "actions / help"),
    ];
    if index_missing {
        // First run: files work now; search needs one explicit background build.
        keys.push(("Space i", "build search index (files work now)"));
    }
    for (key, action) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:>12}  "), theme::accent()),
            Span::styled(action, theme::chrome()),
        ]));
    }
    let width = if spacious { 64 } else { 38 }.min(area.width);
    let height = (lines.len() as u16).min(area.height);
    let centered =
        Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    f.render_widget(Paragraph::new(lines), centered);
}
