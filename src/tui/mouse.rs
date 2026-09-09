//! Mouse hit-testing, separated from the app so it can be driven by a
//! [`MouseEvent`] plus the last frame's [`Regions`] with no terminal.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

/// Screen regions from the last draw.
#[derive(Default, Clone, Copy, Debug)]
pub struct Regions {
    pub sidebar: Rect,
    pub tabs: Rect,
    pub editor: Rect,
    pub preview: Rect,
    /// Neighbors pane (only set while it is shown).
    pub bottom: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Row index inside the sidebar list (0 = first visible row, before scroll offset).
    Sidebar {
        row: usize,
    },
    /// Column offset from the left edge of the tab bar.
    Tabs {
        x: u16,
    },
    Editor,
    Preview,
    /// Row index inside the neighbors pane.
    Bottom {
        row: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Click(Target),
    Scroll { target: Target, down: bool },
}

pub fn inside(a: Rect, x: u16, y: u16) -> bool {
    x >= a.x && x < a.x + a.width && y >= a.y && y < a.y + a.height
}

/// Which pane a point falls in. Bordered panes (sidebar, neighbors) report the
/// list row under the cursor, offset by their top border.
pub fn hit(r: &Regions, x: u16, y: u16) -> Option<Target> {
    if inside(r.sidebar, x, y) {
        Some(Target::Sidebar { row: y.saturating_sub(r.sidebar.y + 1) as usize })
    } else if inside(r.tabs, x, y) {
        Some(Target::Tabs { x: x - r.tabs.x })
    } else if inside(r.editor, x, y) {
        Some(Target::Editor)
    } else if inside(r.preview, x, y) {
        Some(Target::Preview)
    } else if inside(r.bottom, x, y) {
        Some(Target::Bottom { row: y.saturating_sub(r.bottom.y + 1) as usize })
    } else {
        None
    }
}

/// Left click → `Click`, wheel → `Scroll`; anything else is ignored.
pub fn classify(r: &Regions, m: &MouseEvent) -> Option<Action> {
    let target = hit(r, m.column, m.row)?;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => Some(Action::Click(target)),
        MouseEventKind::ScrollDown => Some(Action::Scroll { target, down: true }),
        MouseEventKind::ScrollUp => Some(Action::Scroll { target, down: false }),
        _ => None,
    }
}

/// Tab under column offset `x`, given each tab's label width. Cells are laid
/// out as ` label ` plus a separator, i.e. `label + 3` columns each.
pub fn tab_at(label_widths: &[usize], x: u16) -> Option<usize> {
    let mut left = 0u16;
    for (i, w) in label_widths.iter().enumerate() {
        let w = *w as u16 + 3;
        if x >= left && x < left + w {
            return Some(i);
        }
        left += w;
    }
    None
}

/// Wheel step, clamped to `[0, max - 1]`.
pub fn scroll(cur: usize, max: usize, down: bool, step: usize) -> usize {
    if down { (cur + step).min(max.saturating_sub(1)) } else { cur.saturating_sub(step) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn regions() -> Regions {
        Regions {
            sidebar: Rect::new(0, 0, 20, 30),
            tabs: Rect::new(20, 0, 60, 1),
            editor: Rect::new(20, 1, 30, 20),
            preview: Rect::new(50, 1, 30, 20),
            bottom: Rect::new(20, 21, 60, 9),
        }
    }

    fn ev(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn click_targets_by_region() {
        let r = regions();
        let down = MouseEventKind::Down(MouseButton::Left);
        // sidebar row 3 (y=4 minus the top border)
        assert_eq!(classify(&r, &ev(down, 5, 4)), Some(Action::Click(Target::Sidebar { row: 3 })));
        assert_eq!(classify(&r, &ev(down, 27, 0)), Some(Action::Click(Target::Tabs { x: 7 })));
        assert_eq!(classify(&r, &ev(down, 25, 10)), Some(Action::Click(Target::Editor)));
        assert_eq!(classify(&r, &ev(down, 60, 10)), Some(Action::Click(Target::Preview)));
        assert_eq!(classify(&r, &ev(down, 30, 23)), Some(Action::Click(Target::Bottom { row: 1 })));
        // right button and off-screen are ignored
        assert_eq!(classify(&r, &ev(MouseEventKind::Down(MouseButton::Right), 25, 10)), None);
        assert_eq!(classify(&r, &ev(down, 90, 40)), None);
    }

    #[test]
    fn wheel_scrolls_preview_and_sidebar() {
        let r = regions();
        assert_eq!(
            classify(&r, &ev(MouseEventKind::ScrollDown, 60, 5)),
            Some(Action::Scroll { target: Target::Preview, down: true })
        );
        assert_eq!(
            classify(&r, &ev(MouseEventKind::ScrollUp, 2, 5)),
            Some(Action::Scroll { target: Target::Sidebar { row: 4 }, down: false })
        );
        assert_eq!(scroll(0, 100, true, 3), 3);
        assert_eq!(scroll(98, 100, true, 3), 99);
        assert_eq!(scroll(2, 100, false, 3), 0);
        assert_eq!(scroll(0, 0, true, 3), 0);
    }

    #[test]
    fn tab_hit_by_label_width() {
        // " a.md " = 4+3 = 7 cols, then " notes.md " = 8+3 = 11 cols
        let w = [4usize, 8];
        assert_eq!(tab_at(&w, 0), Some(0));
        assert_eq!(tab_at(&w, 6), Some(0));
        assert_eq!(tab_at(&w, 7), Some(1));
        assert_eq!(tab_at(&w, 17), Some(1));
        assert_eq!(tab_at(&w, 18), None);
    }
}
