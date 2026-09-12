//! Selectable styled text. Drawing and hit testing share grapheme rows, so soft
//! wraps never become clipboard newlines and wide glyphs have one text position.

use std::ops::Range;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

struct Glyph {
    bytes: Range<usize>,
    style: Style,
    width: usize,
}

#[derive(Default)]
pub(crate) struct Reader {
    text: String,
    glyphs: Vec<Glyph>,
    rows: Vec<Range<usize>>,
    layout: Option<(u16, bool)>,
    pub(crate) scroll: usize,
    cursor: usize,
    anchor: Option<usize>,
}

impl Reader {
    pub(crate) fn replace(&mut self, lines: &[Line<'_>]) {
        self.text.clear();
        self.glyphs.clear();
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                self.push("\n", Style::default());
            }
            for glyph in line.styled_graphemes(Style::default()) {
                self.push(glyph.symbol, glyph.style);
            }
        }
        self.layout = None;
        self.cursor = self.cursor.min(self.glyphs.len());
        self.anchor = None;
    }

    fn push(&mut self, symbol: &str, style: Style) {
        let start = self.text.len();
        self.text.push_str(symbol);
        self.glyphs.push(Glyph {
            bytes: start..self.text.len(),
            style,
            width: if symbol == "\t" { 4 } else { Span::raw(symbol).width() },
        });
    }

    fn symbol(&self, i: usize) -> &str {
        &self.text[self.glyphs[i].bytes.clone()]
    }

    pub(crate) fn reflow(&mut self, width: u16, wrap: bool) {
        if self.layout == Some((width, wrap)) {
            return;
        }
        self.layout = Some((width, wrap));
        self.rows.clear();
        let limit = usize::from(width.max(1));
        let mut start = 0;
        let mut at = 0;
        let mut used = 0;
        let mut word_break = None;
        while at < self.glyphs.len() {
            if self.symbol(at) == "\n" {
                self.rows.push(start..at);
                at += 1;
                start = at;
                used = 0;
                word_break = None;
                continue;
            }
            let size = self.glyphs[at].width;
            if wrap && used + size > limit && at > start {
                let end = word_break.filter(|b| *b > start).unwrap_or(at);
                self.rows.push(start..end);
                start = end;
                at = end;
                used = 0;
                word_break = None;
                continue;
            }
            used += size;
            if self.symbol(at).chars().all(char::is_whitespace) {
                word_break = Some(at + 1);
            }
            at += 1;
        }
        self.rows.push(start..at);
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
    }

    pub(crate) fn scroll_by(&mut self, delta: isize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(self.rows.len().saturating_sub(1));
    }

    pub(crate) fn end(&mut self) {
        self.scroll = self.rows.len().saturating_sub(1);
    }

    fn hit(&self, row: usize, x: usize) -> usize {
        let Some(range) = self.rows.get(row) else {
            return self.glyphs.len();
        };
        let mut col = 0;
        for i in range.clone() {
            col += self.glyphs[i].width;
            if x < col {
                return i;
            }
        }
        range.end
    }

    pub(crate) fn point(&mut self, area: Rect, x: u16, y: u16, extend: bool) {
        if area.is_empty() {
            return;
        }
        if extend && self.anchor.is_none() {
            self.anchor = Some(self.cursor);
        }
        let row = usize::from(y.clamp(area.y, area.bottom() - 1) - area.y) + self.scroll;
        let col = usize::from(x.clamp(area.x, area.right()) - area.x);
        self.cursor = self.hit(row, col);
        if !extend {
            self.anchor = Some(self.cursor);
        }
    }

    pub(crate) fn drag(&mut self, area: Rect, x: u16, y: u16) {
        if y < area.y {
            self.scroll_by(-1);
        }
        if y >= area.bottom() {
            self.scroll_by(1);
        }
        self.point(area, x, y, true);
    }

    pub(crate) fn word(&mut self) {
        if self.cursor == self.glyphs.len() {
            return;
        }
        let class = |s: &str| {
            if s.chars().all(char::is_whitespace) {
                0
            } else if s.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                1
            } else {
                2
            }
        };
        let group = class(self.symbol(self.cursor));
        let mut start = self.cursor;
        while start > 0 && self.symbol(start - 1) != "\n" && class(self.symbol(start - 1)) == group {
            start -= 1;
        }
        while self.cursor < self.glyphs.len()
            && self.symbol(self.cursor) != "\n"
            && class(self.symbol(self.cursor)) == group
        {
            self.cursor += 1;
        }
        self.anchor = Some(start);
    }

    pub(crate) fn selected(&self) -> Option<String> {
        let anchor = self.anchor?;
        if anchor == self.cursor {
            return None;
        }
        let byte = |i: usize| self.glyphs.get(i).map_or(self.text.len(), |g| g.bytes.start);
        Some(self.text[byte(anchor.min(self.cursor))..byte(anchor.max(self.cursor))].into())
    }

    pub(crate) fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub(crate) fn begin_selection(&mut self) {
        self.cursor = self.rows.get(self.scroll).map_or(0, |r| r.start);
        self.anchor = Some(self.cursor);
    }

    pub(crate) fn selecting(&self) -> bool {
        self.anchor.is_some()
    }

    pub(crate) fn move_selection(&mut self, horizontal: isize, vertical: isize, height: usize) {
        if vertical == 0 {
            self.cursor = self.cursor.saturating_add_signed(horizontal).min(self.glyphs.len());
        } else {
            let row = self
                .rows
                .iter()
                .position(|r| r.contains(&self.cursor))
                .unwrap_or_else(|| self.rows.len().saturating_sub(1));
            let x: usize = self
                .rows
                .get(row)
                .map_or(0, |r| (r.start..self.cursor.min(r.end)).map(|i| self.glyphs[i].width).sum());
            self.cursor =
                self.hit(row.saturating_add_signed(vertical).min(self.rows.len().saturating_sub(1)), x);
        }
        let row = self
            .rows
            .iter()
            .position(|r| r.contains(&self.cursor))
            .unwrap_or_else(|| self.rows.len().saturating_sub(1));
        if row < self.scroll {
            self.scroll = row;
        }
        if row >= self.scroll + height {
            self.scroll = row.saturating_sub(height.saturating_sub(1));
        }
    }

    pub(crate) fn draw(&self, area: Rect, buffer: &mut Buffer, focused: bool, selection: Style) {
        let selected = self.anchor.map(|a| a.min(self.cursor)..a.max(self.cursor));
        for (y, row) in self.rows.iter().skip(self.scroll).take(area.height as usize).enumerate() {
            let spans: Vec<_> = row
                .clone()
                .map(|i| {
                    let mut style = self.glyphs[i].style;
                    if selected.as_ref().is_some_and(|r| r.contains(&i)) {
                        style = style.patch(selection);
                    } else if focused && self.anchor.is_some() && i == self.cursor {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    Span::styled(if self.symbol(i) == "\t" { "    " } else { self.symbol(i) }, style)
                })
                .collect();
            Line::from(spans).render(Rect::new(area.x, area.y + y as u16, area.width, 1), buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_selection_copies_logical_text_and_maps_wide_graphemes() {
        let mut reader = Reader::default();
        reader.replace(&[Line::from("ab 漢字 e\u{301}🙂 end"), Line::from("next")]);
        let area = Rect::new(10, 3, 6, 10);
        reader.reflow(area.width, true);
        reader.point(area, 10, 3, false);
        reader.point(area, 16, 12, true);
        assert_eq!(reader.selected().unwrap(), "ab 漢字 e\u{301}🙂 end\nnext");
        // The second half of 漢 maps to the same grapheme; a soft wrap
        // leaves neither a missing space nor an invented newline in a yank.
        assert_eq!(reader.hit(1, 0), reader.hit(1, 1));
        reader.point(area, 11, 4, false);
        reader.word();
        assert_eq!(reader.selected().unwrap(), "漢字");
    }

    #[test]
    fn resizing_keeps_selection_and_scroll_reaches_wrapped_tail() {
        let mut r = Reader::default();
        r.replace(&[Line::from("one two three four five")]);
        r.reflow(5, true);
        r.begin_selection();
        r.move_selection(3, 0, 2);
        assert_eq!(r.selected().unwrap(), "one");
        r.reflow(10, true);
        assert_eq!(r.selected().unwrap(), "one");
        r.end();
        assert!(r.scroll > 0);
        r.replace(&[Line::from("new")]);
        r.reflow(10, true);
        assert!(r.selected().is_none());
        assert_eq!(r.scroll, 0);
    }
}
