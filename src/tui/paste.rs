//! Literal terminal paste. A payload never travels through the Vim key map.

use super::app::{App, Focus, Overlay};
use super::vim::{Mode, Vim};
use lapis_lattice::Mode as SearchMode;
use ratatui_textarea::TextArea;

pub(crate) fn insert(vim: &mut Vim, text: &mut TextArea<'_>, payload: &str) -> bool {
    vim.clear_pending();
    let changed = text.insert_str(payload.replace("\r\n", "\n").replace('\r', "\n"));
    if vim.mode != Mode::Insert {
        vim.mode = Mode::Normal;
    }
    changed
}

fn single_line(payload: &str) -> String {
    payload.replace("\r\n", " ").replace(['\r', '\n', '\t'], " ")
}

impl App {
    pub(crate) fn paste(&mut self, payload: String) {
        if let Some(overlay) = self.overlay.as_mut() {
            match overlay {
                Overlay::Palette(p) => {
                    p.input.push_str(&single_line(&payload));
                    if p.is_commands() {
                        p.refresh_commands();
                    } else {
                        self.lattice_search(SearchMode::Bm25);
                    }
                }
                Overlay::Prompt(_, input, _) => input.push_str(&single_line(&payload)),
                _ => self.set_status("close this panel before pasting"),
            }
            return;
        }
        if self.focus != Focus::Editor || self.tasks.is_some() {
            self.set_status("focus an editable note to paste");
            return;
        }
        let Some(t) = self.tab_mut() else { return };
        if t.readonly {
            self.set_status("read-only buffer");
            return;
        }
        if let Some(prompt) = t.vim.prompt.as_mut() {
            prompt.text.push_str(&single_line(&payload));
            return;
        }
        if insert(&mut t.vim, &mut t.text, &payload) {
            t.dirty = true;
            t.refresh_preview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_is_literal_and_one_undo_in_normal_and_insert_modes() {
        for mode in [Mode::Normal, Mode::Insert] {
            let mut vim = Vim::new();
            vim.mode = mode;
            let mut text = TextArea::from(["anchor"]);
            let payload = "first\r\n\tindented\r\n\r\n:q! 漢字\r\n";
            assert!(insert(&mut vim, &mut text, payload));
            assert_eq!(text.lines().join("\n"), "first\n\tindented\n\n:q! 漢字\nanchor");
            assert_eq!(vim.mode, mode);
            assert!(text.undo());
            assert_eq!(text.lines(), &["anchor"]);
            assert!(text.redo());
            assert_eq!(text.lines()[3], ":q! 漢字");
        }
    }

    #[test]
    fn prompt_paste_does_not_submit_a_command() {
        assert_eq!(single_line("one\r\n:q!\n\tend"), "one :q!  end");
    }
}
