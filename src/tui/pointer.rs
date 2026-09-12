//! Pointer selection uses the editor's own rendered coordinate map.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Margin, Position, Rect};
use ratatui_textarea::{CursorMove, DataCursor, TextArea};

use super::app::{App, Focus};
use super::mouse::{self, Action as MouseAction, Target};
use super::vim::Mode;

#[derive(Default)]
pub(crate) struct Pointer {
    drag: Option<Drag>,
    last_click: Option<(Instant, u16, u16)>,
    last_motion: Option<(MouseEvent, Instant)>,
}

#[derive(Clone, Copy)]
enum Drag {
    Editor,
    Preview,
    SidebarDivider,
    PreviewDivider,
}

fn inner(text: &TextArea<'_>, area: Rect) -> Rect {
    text.block().map_or(area, |b| b.inner(area))
}

fn location(text: &TextArea<'_>, area: Rect, x: u16, y: u16) -> CursorMove {
    let area = inner(text, area);
    let (top, left) = text.scroll_offset();
    let row = y.clamp(area.y, area.bottom().saturating_sub(1).max(area.y)) - area.y;
    let col = x.clamp(area.x, area.right().saturating_sub(1).max(area.x)) - area.x;
    // Horizontal scrolling clips the gutter too, so add the offset before
    // subtracting the gutter. This also maps either half of a wide glyph.
    let col = (usize::from(col) + usize::from(left)).saturating_sub(text.line_number_width() as usize);
    let DataCursor(row, col) = text.screen_to_data(usize::from(top) + usize::from(row), col);
    CursorMove::Jump(row.min(u16::MAX as usize) as u16, col.min(u16::MAX as usize) as u16)
}

fn select_word(text: &mut TextArea<'_>) {
    let DataCursor(row, col) = text.cursor();
    let chars: Vec<char> = text.lines()[row].chars().collect();
    if chars.is_empty() {
        return;
    }
    let col = col.min(chars.len() - 1);
    let class = |c: char| {
        if c.is_whitespace() {
            0
        } else if c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        }
    };
    let group = class(chars[col]);
    let mut start = col;
    let mut end = col + 1;
    while start > 0 && class(chars[start - 1]) == group {
        start -= 1;
    }
    while end < chars.len() && class(chars[end]) == group {
        end += 1;
    }
    text.cancel_selection();
    text.move_cursor(CursorMove::Jump(row as u16, start as u16));
    text.start_selection();
    text.move_cursor(CursorMove::Jump(row as u16, end as u16));
}

impl App {
    pub(crate) fn pointer_tick(&mut self) -> bool {
        let mut changed = false;
        if let Some((event, at)) = self.pointer.last_motion
            && matches!(self.pointer.drag, Some(Drag::Preview))
            && at.elapsed() >= Duration::from_millis(50)
            && !self.regions.preview.inner(Margin::new(1, 1)).contains(Position::new(event.column, event.row))
        {
            self.drag_preview(event);
            changed = true;
        }
        if let Some((event, at)) = self.pointer.last_motion
            && matches!(self.pointer.drag, Some(Drag::Editor))
            && at.elapsed() >= Duration::from_millis(50)
            && let Some(t) = self.tab()
        {
            let area = inner(&t.text, self.regions.editor);
            if !area.contains(Position::new(event.column, event.row)) {
                self.drag_editor(event);
                changed = true;
            }
        }
        changed
    }

    fn drag_preview(&mut self, m: MouseEvent) {
        let area = self.regions.preview.inner(Margin::new(1, 1));
        if let Some(t) = self.tab_mut() {
            t.reader.drag(area, m.column, m.row);
        }
        self.pointer.last_motion = Some((m, Instant::now()));
    }

    fn drag_editor(&mut self, m: MouseEvent) {
        let area = self.regions.editor;
        if let Some(t) = self.tab_mut() {
            let content = inner(&t.text, area);
            if m.row < content.y {
                t.text.scroll((-1, 0));
            }
            if m.row >= content.bottom() {
                t.text.scroll((1, 0));
            }
            if m.column < content.x {
                t.text.scroll((0, -1));
            }
            if m.column >= content.right() {
                t.text.scroll((0, 1));
            }
            t.text.move_cursor(location(&t.text, area, m.column, m.row));
            if t.vim.mode != Mode::Insert {
                t.vim.mode = Mode::Visual;
            }
        }
        self.pointer.last_motion = Some((m, Instant::now()));
    }

    pub(crate) fn mouse(&mut self, m: MouseEvent) {
        if self.overlay.is_some() {
            self.pointer.drag = None;
            return;
        }
        if m.kind == MouseEventKind::Up(MouseButton::Left) {
            if matches!(self.pointer.drag, Some(Drag::Editor))
                && let Some(t) = self.tab_mut()
                && t.text.selection_range().is_some_and(|(a, b)| a == b)
            {
                t.text.cancel_selection();
                if t.vim.mode == Mode::Visual {
                    t.vim.mode = Mode::Normal;
                }
            }
            self.pointer.drag = None;
            self.pointer.last_motion = None;
            return;
        }
        if m.kind == MouseEventKind::Drag(MouseButton::Left) {
            match self.pointer.drag {
                Some(Drag::Editor) => self.drag_editor(m),
                Some(Drag::Preview) => self.drag_preview(m),
                Some(Drag::SidebarDivider) => {
                    let width = self.regions.sidebar.width + self.regions.tabs.width;
                    self.sidebar_pct =
                        ((u32::from(m.column) * 100 / u32::from(width.max(1))) as u16).clamp(12, 60);
                }
                Some(Drag::PreviewDivider) => {
                    let width = self.regions.editor.width + self.regions.preview.width;
                    let remaining = self.regions.preview.right().saturating_sub(m.column);
                    self.preview_pct =
                        ((u32::from(remaining) * 100 / u32::from(width.max(1))) as u16).clamp(20, 80);
                }
                None => {}
            }
            return;
        }
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            self.pointer.drag = None;
            if self.show_sidebar
                && self.regions.sidebar.width > 0
                && m.column == self.regions.sidebar.right().saturating_sub(1)
            {
                self.pointer.drag = Some(Drag::SidebarDivider);
                return;
            }
            if self.regions.preview.width > 0
                && self.regions.editor.width > 0
                && m.column == self.regions.editor.right().saturating_sub(1)
            {
                self.pointer.drag = Some(Drag::PreviewDivider);
                return;
            }
            if self.tasks.is_none() && mouse::inside(self.regions.editor, m.column, m.row) {
                self.focus = Focus::Editor;
                let area = self.regions.editor;
                let double = self.pointer.last_click.is_some_and(|(at, x, y)| {
                    at.elapsed() < Duration::from_millis(400) && (x, y) == (m.column, m.row)
                });
                self.pointer.last_click = Some((Instant::now(), m.column, m.row));
                if let Some(t) = self.tab_mut() {
                    if !inner(&t.text, area).contains(Position::new(m.column, m.row)) {
                        return;
                    }
                    t.vim.clear_pending();
                    t.vim.selection_exclusive = true;
                    let extend = m.modifiers.contains(KeyModifiers::SHIFT);
                    if !extend {
                        t.text.cancel_selection();
                    }
                    if extend && !t.text.is_selecting() {
                        t.text.start_selection();
                    }
                    t.text.move_cursor(location(&t.text, area, m.column, m.row));
                    if double {
                        select_word(&mut t.text);
                    } else if !extend {
                        t.text.start_selection();
                    }
                    if t.vim.mode != Mode::Insert {
                        t.vim.mode = if double || extend { Mode::Visual } else { Mode::Normal };
                    }
                    self.pointer.drag = Some(Drag::Editor);
                }
                return;
            }
            let area = self.regions.preview.inner(Margin::new(1, 1));
            if area.contains(Position::new(m.column, m.row)) {
                self.focus = Focus::Preview;
                let double = self.pointer.last_click.is_some_and(|(at, x, y)| {
                    at.elapsed() < Duration::from_millis(400) && (x, y) == (m.column, m.row)
                });
                self.pointer.last_click = Some((Instant::now(), m.column, m.row));
                if let Some(t) = self.tab_mut() {
                    t.reader.point(area, m.column, m.row, m.modifiers.contains(KeyModifiers::SHIFT));
                    if double {
                        t.reader.word();
                    }
                }
                self.pointer.drag = Some(Drag::Preview);
                return;
            }
        }
        self.mouse_navigation(m);
    }
}

impl App {
    pub(crate) fn mouse_navigation(&mut self, m: MouseEvent) {
        let Some(action) = mouse::classify(&self.regions, &m) else { return };
        match action {
            MouseAction::Click(target) => {
                if self.overlay.is_some() {
                    return;
                }
                match target {
                    Target::Sidebar { row } => {
                        self.focus = Focus::Sidebar;
                        let row = row + self.sidebar_scroll;
                        let rows = self.tree.visible();
                        if row < rows.len() {
                            let was = self.sel;
                            self.sel = row;
                            if was == row {
                                // second click opens / toggles
                                let (_, e) = rows[row].clone();
                                let root = self.root();
                                if e.is_dir {
                                    if !self.tree.expanded.remove(&e.rel) {
                                        self.tree.load(&root, &e.rel);
                                        self.tree.expanded.insert(e.rel);
                                    }
                                } else {
                                    self.open_note(&e.rel);
                                    self.tasks = None;
                                }
                            }
                        }
                    }
                    Target::Tabs { x } => {
                        if let Some(i) = self.tab_slots.iter().find(|s| s.cells.contains(&x)).map(|s| s.index)
                        {
                            self.active = i;
                            self.focus = Focus::Editor;
                            self.after_open();
                        }
                    }
                    Target::Editor => {
                        self.focus = if self.tasks.is_some() { Focus::Tasks } else { Focus::Editor };
                    }
                    Target::Preview => self.focus = Focus::Preview,
                    Target::Bottom { row } => {
                        if self.show_neighbors
                            && let Some(p) = self.neighbors.get(row).and_then(|n| n.path.clone())
                        {
                            self.open_note(&p);
                        }
                    }
                }
            }
            MouseAction::Scroll { target, down } => match target {
                Target::Sidebar { .. } => {
                    let n = self.tree.visible().len();
                    self.sel = mouse::scroll(self.sel, n, down, 3);
                }
                Target::Preview => {
                    if let Some(t) = self.tab_mut() {
                        t.reader.scroll_by(if down { 3 } else { -3 });
                    }
                }
                Target::Editor => {
                    if let Some(tv) = self.tasks.as_mut() {
                        if down { tv.down() } else { tv.up() }
                    } else if let Some(t) = self.tab_mut() {
                        t.text.scroll((if down { 3 } else { -3 }, 0));
                    }
                }
                Target::Tabs { .. } if !self.tabs.is_empty() => {
                    self.active = mouse::scroll(self.active, self.tabs.len(), down, 1);
                    self.after_open();
                }
                Target::Tabs { .. } | Target::Bottom { .. } => {}
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::style::Style;
    use ratatui::widgets::{Block, Borders, Widget};
    use ratatui_textarea::WrapMode;

    fn rendered(lines: &[&str], area: Rect, wrap: bool) -> TextArea<'static> {
        let mut text = TextArea::from(lines.iter().copied());
        text.set_block(Block::default().borders(Borders::ALL));
        text.set_line_number_style(Style::default());
        if wrap {
            text.set_wrap_mode(WrapMode::WordOrGlyph);
        }
        (&text).render(area, &mut Buffer::empty(area));
        text
    }

    #[test]
    fn hit_testing_matches_gutter_wide_characters_tabs_and_wrapping() {
        let area = Rect::new(20, 2, 24, 8);
        let mut text = rendered(&["a漢\tb", "second"], area, false);
        let x = area.x + 1 + text.line_number_width();
        for (dx, expected) in [(0, 0), (1, 1), (2, 1), (3, 2), (4, 3)] {
            text.move_cursor(location(&text, area, x + dx, area.y + 1));
            assert_eq!(text.cursor(), (0, expected));
        }
        let area = Rect::new(10, 3, 12, 7);
        let mut text = rendered(&["abcdefghijklmnop"], area, true);
        text.move_cursor(location(&text, area, area.x + 1 + text.line_number_width(), area.y + 2));
        assert_eq!(text.cursor(), (0, 7));
    }

    #[test]
    fn horizontal_scroll_is_applied_before_clipped_gutter() {
        let area = Rect::new(5, 4, 12, 6);
        let mut text = rendered(&["abcdefghijklmnopqrstuvwxyz"], area, false);
        text.move_cursor(CursorMove::End);
        (&text).render(area, &mut Buffer::empty(area));
        let (_, left) = text.scroll_offset();
        assert!(left > text.line_number_width());
        text.move_cursor(location(&text, area, area.x + 1, area.y + 1));
        assert_eq!(text.cursor(), (0, (left - text.line_number_width()) as usize));
    }

    #[test]
    fn mouse_word_yank_excludes_the_following_character() {
        let mut text = TextArea::from(["alpha beta!"]);
        text.move_cursor(CursorMove::Jump(0, 8));
        select_word(&mut text);
        let mut vim = super::super::vim::Vim::new();
        vim.mode = Mode::Visual;
        vim.selection_exclusive = true;
        vim.input(
            ratatui_textarea::Input {
                key: ratatui_textarea::Key::Char('y'),
                ctrl: false,
                alt: false,
                shift: false,
            },
            &mut text,
        );
        assert_eq!(text.yank_text(), "beta");
    }
}
