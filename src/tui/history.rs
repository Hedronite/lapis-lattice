//! Byte-range undo transactions, bounded by retained edit bytes. A paste or
//! selection replacement is one transaction, independent of widget operations.

use std::collections::VecDeque;

type Cursor = (usize, usize);
const BUDGET: usize = 32 * 1024 * 1024;

struct Edit {
    at: usize,
    removed: String,
    inserted: String,
    before: Cursor,
    after: Cursor,
}

#[derive(Default)]
pub(crate) struct History {
    edits: VecDeque<Edit>,
    position: usize,
    bytes: usize,
}

impl History {
    pub(crate) fn record(&mut self, before: &str, after: &str, from: Cursor, to: Cursor) {
        if before == after {
            return;
        }
        while self.edits.len() > self.position {
            let edit = self.edits.pop_back().unwrap();
            self.bytes -= edit.removed.len() + edit.inserted.len();
        }
        let mut prefix = before.bytes().zip(after.bytes()).take_while(|(a, b)| a == b).count();
        while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = before[prefix..]
            .bytes()
            .rev()
            .zip(after[prefix..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(before.len() - suffix) || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        let edit = Edit {
            at: prefix,
            removed: before[prefix..before.len() - suffix].into(),
            inserted: after[prefix..after.len() - suffix].into(),
            before: from,
            after: to,
        };
        self.bytes += edit.removed.len() + edit.inserted.len();
        self.edits.push_back(edit);
        // Keep the newest transaction even if it exceeds the budget: a large
        // paste must remain undoable. Subsequent edits evict it if necessary.
        while self.edits.len() > 1 && (self.bytes > BUDGET || self.edits.len() > 4096) {
            let edit = self.edits.pop_front().unwrap();
            self.bytes -= edit.removed.len() + edit.inserted.len();
        }
        self.position = self.edits.len();
    }

    pub(crate) fn apply(&mut self, current: &str, redo: bool) -> Option<(String, Cursor)> {
        let index = if redo { self.position } else { self.position.checked_sub(1)? };
        let edit = self.edits.get(index)?;
        let (expected, replacement, cursor) = if redo {
            (&edit.removed, &edit.inserted, edit.after)
        } else {
            (&edit.inserted, &edit.removed, edit.before)
        };
        let end = edit.at.checked_add(expected.len())?;
        // Fail closed if a caller forgot to record an external buffer change.
        if current.get(edit.at..end)? != expected {
            return None;
        }
        let mut result = current.to_string();
        result.replace_range(edit.at..end, replacement);
        self.position = if redo { index + 1 } else { index };
        Some((result, cursor))
    }
}

impl super::app::Tab {
    pub(crate) fn record_edit(&mut self, before: Cursor) {
        let body = self.body();
        if body == self.preview_for {
            return;
        }
        let cursor = self.text.cursor();
        self.history.record(&self.preview_for, &body, before, (cursor.0, cursor.1));
        self.update_dirty(&body);
        self.refresh_preview();
    }

    fn update_dirty(&mut self, body: &str) {
        self.dirty = self
            .saved_source
            .as_ref()
            .is_none_or(|source| crate::hal::raw_parts(source).1.replace("\r\n", "\n") != body);
    }

    pub(crate) fn undo_edit(&mut self, redo: bool) -> bool {
        let Some((body, cursor)) = self.history.apply(&self.body(), redo) else {
            return false;
        };
        let mut text = ratatui_textarea::TextArea::from(body.split('\n'));
        text.set_max_histories(0);
        text.set_cursor_line_style(ratatui::style::Style::default());
        text.set_selection_style(super::theme::selected());
        if self.text.line_number_style().is_some() {
            text.set_line_number_style(super::theme::dim());
        }
        text.set_wrap_mode(self.text.wrap_mode());
        text.move_cursor(ratatui_textarea::CursorMove::Jump(cursor.0 as u16, cursor.1 as u16));
        self.text = text;
        self.vim.clear_pending();
        self.vim.mode = super::vim::Mode::Normal;
        self.update_dirty(&body);
        self.refresh_preview();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_selection_replacement_is_atomic_and_redo_branches() {
        let mut h = History::default();
        let before = "α first\n漢字 last\n";
        let after = "α :q!\n\t🙂\n last\n";
        h.record(before, after, (0, 2), (2, 0));
        assert_eq!(h.apply(after, false), Some((before.into(), (0, 2))));
        assert_eq!(h.apply(before, true), Some((after.into(), (2, 0))));
        h.apply(after, false).unwrap();
        h.record(before, "new", (0, 0), (0, 3));
        assert!(h.apply("new", true).is_none());
        assert_eq!(h.apply("new", false).unwrap().0, before);
    }

    #[test]
    fn one_mebibyte_paste_has_one_undo_and_redo() {
        let mut h = History::default();
        let after = format!("{}anchor", "line\t漢字\n".repeat(100_000));
        h.record("anchor", &after, (0, 0), (100_000, 0));
        assert!(h.bytes < after.len());
        assert_eq!(h.apply(&after, false).unwrap().0, "anchor");
        assert!(h.apply("anchor", false).is_none());
        assert_eq!(h.apply("anchor", true).unwrap().0, after);
    }
}
