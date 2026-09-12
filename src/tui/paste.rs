//! Literal terminal paste. A payload never travels through the Vim key map.

use super::app::{App, Focus, Overlay};
use super::vim::{Mode, Vim};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use lapis_lattice::Mode as SearchMode;
use ratatui_textarea::TextArea;

/// Key events that arrive together in one read and are all plain text are a paste
/// from a terminal without bracketed paste. Below this many, they are typing.
pub(crate) const BURST: usize = 16;

/// The text a key event contributes to a raw paste, if it is plain text.
fn text_of(event: &Event) -> Option<char> {
    let Event::Key(k) = event else { return None };
    if k.kind != KeyEventKind::Press || k.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SUPER) {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    match k.code {
        KeyCode::Char(c) if !ctrl => Some(c),
        // Raw LF and CR reach a raw-mode terminal as Ctrl+J and Enter. Keep them
        // distinct here so [`insert`] folds a CR LF pair into one line break.
        KeyCode::Char('j') if ctrl => Some('\n'),
        KeyCode::Char('m') if ctrl => Some('\r'),
        KeyCode::Enter => Some('\r'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// Turn runs of at least [`BURST`] plain-text key events into one [`Event::Paste`],
/// so an unbracketed paste is inserted literally instead of being run as keys.
pub(crate) fn coalesce(events: Vec<Event>) -> Vec<Event> {
    let mut out = Vec::with_capacity(events.len());
    let mut run: Vec<Event> = Vec::new();
    let flush = |run: &mut Vec<Event>, out: &mut Vec<Event>| {
        if run.len() >= BURST {
            out.push(Event::Paste(run.iter().filter_map(text_of).collect()));
        } else {
            out.append(run);
        }
        run.clear();
    };
    for event in events {
        if text_of(&event).is_some() {
            run.push(event);
        } else {
            flush(&mut run, &mut out);
            out.push(event);
        }
    }
    flush(&mut run, &mut out);
    out
}

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
        if self.too_small {
            self.set_status("enlarge the terminal before pasting");
            return;
        }
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
        let cursor = t.text.cursor();
        if insert(&mut t.vim, &mut t.text, &payload) {
            t.record_edit((cursor.0, cursor.1));
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
    fn a_burst_of_plain_keys_is_one_paste_and_short_runs_stay_keys() {
        use crossterm::event::KeyEvent;
        let key = |code: KeyCode, m: KeyModifiers| Event::Key(KeyEvent::new(code, m));
        let mut burst: Vec<Event> =
            "first line".chars().map(|c| key(KeyCode::Char(c), KeyModifiers::NONE)).collect();
        burst.push(key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        burst.extend("    indented".chars().map(|c| key(KeyCode::Char(c), KeyModifiers::NONE)));
        burst.push(key(KeyCode::Enter, KeyModifiers::NONE));
        burst.push(key(KeyCode::Char('j'), KeyModifiers::CONTROL));
        let coalesced = coalesce(burst);
        assert_eq!(coalesced.len(), 1);
        assert!(matches!(&coalesced[0], Event::Paste(t) if t == "first line\n    indented\r\n"));
        let mut vim = Vim::new();
        let mut text = TextArea::from(["anchor"]);
        assert!(insert(&mut vim, &mut text, "first line\n    indented\r\n"));
        assert_eq!(text.lines().join("\n"), "first line\n    indented\nanchor");
        // Typing: a short run stays individual keys, and a control key splits runs.
        let mut typed: Vec<Event> =
            "iunsaved".chars().map(|c| key(KeyCode::Char(c), KeyModifiers::NONE)).collect();
        typed.push(key(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(coalesce(typed).len(), 9);
        let mut long: Vec<Event> =
            "x".repeat(BURST).chars().map(|c| key(KeyCode::Char(c), KeyModifiers::NONE)).collect();
        long.push(key(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let out = coalesce(long);
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Event::Paste(t) if t.len() == BURST));
    }

    #[test]
    fn prompt_paste_does_not_submit_a_command() {
        assert_eq!(single_line("one\r\n:q!\n\tend"), "one :q!  end");
    }
}
