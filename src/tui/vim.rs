//! Vim-like editing over `ratatui-textarea`, modeled on the crate's `vim`
//! example: NORMAL / INSERT / VISUAL / OPERATOR / REPLACE, hjkl, w b e 0 ^ $,
//! gg G, i a I A o O, x, dd yy cc, d/y/c + motion, p, u, Ctrl+r, J, `/` `n`
//! `N`, `:` ex line (`w`, `q`, `wq`, `x`, `q!`). Anything the buffer cannot
//! express is reported back to the app as an [`Action`].

use std::fmt;

use ratatui::style::{Modifier, Style};
use ratatui_textarea::{CursorMove, Input, Key, Scrolling, TextArea};

use super::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    /// `d`, `y`, `c` awaiting a motion.
    Operator(char),
    /// `r` (once) or `R` (overtype).
    Replace(bool),
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mode::Normal => write!(f, "NORMAL"),
            Mode::Insert => write!(f, "INSERT"),
            Mode::Visual => write!(f, "VISUAL"),
            Mode::Operator(c) => write!(f, "OPERATOR {c}"),
            Mode::Replace(_) => write!(f, "REPLACE"),
        }
    }
}

/// What the app must do after a key the editor could not fully handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Save,
    CloseTab {
        force: bool,
    },
    SaveAndClose,
    /// Leave editor focus (Esc in NORMAL).
    Blur,
    /// Toggle the checkbox on the cursor line (Ctrl+L).
    ToggleCheckbox,
    Status(String),
    /// `gt` / `gT`
    NextTab,
    PrevTab,
    /// Space in NORMAL: hand over to the leader.
    Leader,
    /// `?` in NORMAL
    Help,
}

/// Text-entry prompt shown on the status row (`/pattern` or `:cmd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub kind: char,
    pub text: String,
}

pub struct Vim {
    pub mode: Mode,
    pending: Option<char>,
    pub prompt: Option<Prompt>,
    count: usize,
}

impl Default for Vim {
    fn default() -> Self {
        Self::new()
    }
}

impl Vim {
    pub fn new() -> Self {
        Self { mode: Mode::Normal, pending: None, prompt: None, count: 0 }
    }

    pub fn cursor_style(&self) -> Style {
        theme::mode(&self.mode.to_string()).add_modifier(Modifier::REVERSED)
    }

    pub fn mode_label(&self) -> String {
        match self.mode {
            Mode::Operator(_) => "OPERATOR".into(),
            m => m.to_string(),
        }
    }

    fn is_before_line_end(ta: &TextArea<'_>) -> bool {
        let c = ta.cursor();
        c.1 < ta.lines()[c.0].chars().count().saturating_sub(1)
    }

    fn count(&mut self) -> usize {
        let n = self.count.max(1);
        self.count = 0;
        n
    }

    /// Feed one key. Returns the action for the app.
    pub fn input(&mut self, input: Input, ta: &mut TextArea<'_>) -> Action {
        if input.key == Key::Null {
            return Action::None;
        }
        // ---- prompt line (`/` or `:`)
        if let Some(p) = self.prompt.as_mut() {
            match input.key {
                Key::Esc => {
                    self.prompt = None;
                }
                Key::Enter => {
                    let p = self.prompt.take().unwrap();
                    return self.run_prompt(p, ta);
                }
                Key::Backspace => {
                    if p.text.pop().is_none() {
                        self.prompt = None;
                    }
                }
                Key::Char(c) if !input.ctrl => p.text.push(c),
                _ => {}
            }
            return Action::None;
        }
        match self.mode {
            Mode::Insert => self.insert(input, ta),
            Mode::Replace(once) => self.replace(input, once, ta),
            Mode::Normal | Mode::Visual | Mode::Operator(_) => self.normal(input, ta),
        }
    }

    fn insert(&mut self, input: Input, ta: &mut TextArea<'_>) -> Action {
        match input {
            Input { key: Key::Esc, .. } => {
                self.mode = Mode::Normal;
                if ta.cursor().1 > 0 {
                    ta.move_cursor(CursorMove::Back);
                }
                Action::None
            }
            Input { key: Key::Char('l'), ctrl: true, .. } => Action::ToggleCheckbox,
            Input { key: Key::Char('s'), ctrl: true, .. } => Action::Save,
            _ => {
                ta.input(input);
                Action::None
            }
        }
    }

    fn replace(&mut self, input: Input, once: bool, ta: &mut TextArea<'_>) -> Action {
        match input.key {
            Key::Esc => self.mode = Mode::Normal,
            Key::Char(c) => {
                if Self::is_before_line_end(ta) || !ta.lines()[ta.cursor().0].is_empty() {
                    ta.delete_next_char();
                }
                ta.insert_char(c);
                if once {
                    ta.move_cursor(CursorMove::Back);
                    self.mode = Mode::Normal;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn run_prompt(&mut self, p: Prompt, ta: &mut TextArea<'_>) -> Action {
        match p.kind {
            '/' => {
                if p.text.is_empty() {
                    let _ = ta.set_search_pattern("");
                    return Action::None;
                }
                match ta.set_search_pattern(&p.text) {
                    Ok(()) => {
                        if !ta.search_forward(false) {
                            return Action::Status(format!("pattern not found: {}", p.text));
                        }
                        Action::None
                    }
                    Err(e) => Action::Status(format!("bad pattern: {e}")),
                }
            }
            ':' => match p.text.trim() {
                "w" => Action::Save,
                "q" => Action::CloseTab { force: false },
                "q!" => Action::CloseTab { force: true },
                "wq" | "x" => Action::SaveAndClose,
                "" => Action::None,
                other => {
                    if let Ok(n) = other.parse::<u16>() {
                        ta.move_cursor(CursorMove::Jump(n.saturating_sub(1), 0));
                        Action::None
                    } else {
                        Action::Status(format!("unknown ex command: {other}"))
                    }
                }
            },
            _ => Action::None,
        }
    }

    fn select_line(ta: &mut TextArea<'_>) {
        ta.move_cursor(CursorMove::Head);
        ta.start_selection();
        let before = ta.cursor();
        ta.move_cursor(CursorMove::Down);
        if before == ta.cursor() {
            ta.move_cursor(CursorMove::End);
        }
    }

    fn apply_operator(&mut self, op: char, ta: &mut TextArea<'_>) {
        match op {
            'y' => {
                ta.copy();
                self.mode = Mode::Normal;
            }
            'd' => {
                ta.cut();
                self.mode = Mode::Normal;
            }
            'c' => {
                ta.cut();
                self.mode = Mode::Insert;
            }
            _ => self.mode = Mode::Normal,
        }
    }

    fn normal(&mut self, input: Input, ta: &mut TextArea<'_>) -> Action {
        // Two-key sequences: gg, gt, gT.
        if let Some(pending) = self.pending.take() {
            match (pending, input.key) {
                ('g', Key::Char('g')) => {
                    ta.move_cursor(CursorMove::Top);
                    ta.move_cursor(CursorMove::Head);
                }
                ('g', Key::Char('t')) => return Action::NextTab,
                ('g', Key::Char('T')) => return Action::PrevTab,
                _ => {}
            }
            if let Mode::Operator(op) = self.mode
                && pending == 'g'
            {
                self.apply_operator(op, ta);
            }
            return Action::None;
        }
        if let Key::Char(d) = input.key
            && d.is_ascii_digit()
            && !(d == '0' && self.count == 0)
            && !input.ctrl
        {
            self.count = self.count * 10 + (d as u8 - b'0') as usize;
            return Action::None;
        }
        let n = self.count();
        let action = match input {
            Input { key: Key::Char('h') | Key::Left, .. } | Input { key: Key::Backspace, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::Back);
                }
                Action::None
            }
            Input { key: Key::Char('j') | Key::Down, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::Down);
                }
                Action::None
            }
            Input { key: Key::Char('k') | Key::Up, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::Up);
                }
                Action::None
            }
            Input { key: Key::Char('l') | Key::Right, ctrl: false, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::Forward);
                }
                Action::None
            }
            Input { key: Key::Char('w'), ctrl: false, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::WordForward);
                }
                Action::None
            }
            Input { key: Key::Char('e'), ctrl: false, .. } => {
                ta.move_cursor(CursorMove::WordEnd);
                if matches!(self.mode, Mode::Operator(_)) {
                    ta.move_cursor(CursorMove::Forward);
                }
                Action::None
            }
            Input { key: Key::Char('b'), ctrl: false, .. } => {
                for _ in 0..n {
                    ta.move_cursor(CursorMove::WordBack);
                }
                Action::None
            }
            Input { key: Key::Char('0') | Key::Home, .. } => {
                ta.move_cursor(CursorMove::Head);
                Action::None
            }
            Input { key: Key::Char('^'), .. } => {
                ta.move_cursor(CursorMove::Head);
                Action::None
            }
            Input { key: Key::Char('$') | Key::End, .. } => {
                ta.move_cursor(CursorMove::End);
                if self.mode == Mode::Normal && ta.cursor().1 > 0 {
                    ta.move_cursor(CursorMove::Back);
                }
                Action::None
            }
            Input { key: Key::Char('G'), .. } => {
                ta.move_cursor(CursorMove::Bottom);
                ta.move_cursor(CursorMove::Head);
                Action::None
            }
            Input { key: Key::Char('{'), .. } => {
                ta.move_cursor(CursorMove::ParagraphBack);
                Action::None
            }
            Input { key: Key::Char('}'), .. } => {
                ta.move_cursor(CursorMove::ParagraphForward);
                Action::None
            }
            Input { key: Key::Char('g'), ctrl: false, .. } => {
                self.pending = Some('g');
                return Action::None;
            }
            Input { key: Key::Char('D'), .. } => {
                ta.delete_line_by_end();
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('C'), .. } => {
                ta.delete_line_by_end();
                ta.cancel_selection();
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('p'), ctrl: false, .. } => {
                let y = ta.yank_text();
                for _ in 0..n {
                    if y.ends_with('\n') {
                        // linewise register: paste below the current line
                        let row = ta.cursor().0;
                        ta.move_cursor(CursorMove::End);
                        ta.insert_newline();
                        ta.insert_str(y.trim_end_matches('\n'));
                        ta.move_cursor(CursorMove::Jump(row as u16 + 1, 0));
                    } else {
                        ta.paste();
                    }
                }
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('u'), ctrl: false, .. } => {
                ta.undo();
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('r'), ctrl: true, .. } => {
                ta.redo();
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('x'), ctrl: false, .. } if self.mode == Mode::Normal => {
                for _ in 0..n {
                    if Self::is_before_line_end(ta) || !ta.lines()[ta.cursor().0].is_empty() {
                        ta.delete_next_char();
                    }
                }
                return Action::None;
            }
            Input { key: Key::Char('i'), ctrl: false, .. } if self.mode == Mode::Normal => {
                ta.cancel_selection();
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('a'), ctrl: false, .. } if self.mode == Mode::Normal => {
                ta.cancel_selection();
                if Self::is_before_line_end(ta) || !ta.lines()[ta.cursor().0].is_empty() {
                    ta.move_cursor(CursorMove::Forward);
                }
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('A'), .. } if self.mode == Mode::Normal => {
                ta.cancel_selection();
                ta.move_cursor(CursorMove::End);
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('I'), .. } if self.mode == Mode::Normal => {
                ta.cancel_selection();
                ta.move_cursor(CursorMove::Head);
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('o'), ctrl: false, .. } if self.mode == Mode::Normal => {
                ta.move_cursor(CursorMove::End);
                ta.insert_newline();
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('O'), .. } if self.mode == Mode::Normal => {
                ta.move_cursor(CursorMove::Head);
                ta.insert_newline();
                ta.move_cursor(CursorMove::Up);
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char('J'), ctrl: false, .. } if self.mode == Mode::Normal => {
                let row = ta.cursor().0;
                if row + 1 < ta.lines().len() {
                    ta.move_cursor(CursorMove::End);
                    ta.delete_next_char();
                    ta.insert_char(' ');
                }
                return Action::None;
            }
            Input { key: Key::Char('r'), ctrl: false, .. } if self.mode == Mode::Normal => {
                self.mode = Mode::Replace(true);
                return Action::None;
            }
            Input { key: Key::Char('R'), ctrl: false, .. } if self.mode == Mode::Normal => {
                self.mode = Mode::Replace(false);
                return Action::None;
            }
            Input { key: Key::Char('v'), ctrl: false, .. } if self.mode == Mode::Normal => {
                ta.start_selection();
                self.mode = Mode::Visual;
                return Action::None;
            }
            Input { key: Key::Char('V'), ctrl: false, .. } if self.mode == Mode::Normal => {
                Self::select_line(ta);
                self.mode = Mode::Visual;
                return Action::None;
            }
            Input { key: Key::Esc, .. } => {
                if self.mode == Mode::Normal {
                    return Action::Blur;
                }
                ta.cancel_selection();
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('/'), .. } => {
                self.prompt = Some(Prompt { kind: '/', text: String::new() });
                return Action::None;
            }
            Input { key: Key::Char(':'), .. } => {
                self.prompt = Some(Prompt { kind: ':', text: String::new() });
                return Action::None;
            }
            Input { key: Key::Char('n'), ctrl: false, .. } => {
                if !ta.search_forward(false) {
                    return Action::Status("no more matches".into());
                }
                return Action::None;
            }
            Input { key: Key::Char('N'), ctrl: false, .. } => {
                if !ta.search_back(false) {
                    return Action::Status("no more matches".into());
                }
                return Action::None;
            }
            Input { key: Key::Char(' '), .. } if self.mode == Mode::Normal => return Action::Leader,
            Input { key: Key::Char('?'), .. } if self.mode == Mode::Normal => return Action::Help,
            Input { key: Key::Char('s'), ctrl: true, .. } => return Action::Save,
            Input { key: Key::Char('l'), ctrl: true, .. } => return Action::ToggleCheckbox,
            Input { key: Key::Char('e'), ctrl: true, .. } => {
                ta.scroll((1, 0));
                Action::None
            }
            Input { key: Key::Char('y'), ctrl: true, .. } => {
                ta.scroll((-1, 0));
                Action::None
            }
            Input { key: Key::Char('d'), ctrl: true, .. } => {
                ta.scroll(Scrolling::HalfPageDown);
                Action::None
            }
            Input { key: Key::Char('u'), ctrl: true, .. } => {
                ta.scroll(Scrolling::HalfPageUp);
                Action::None
            }
            Input { key: Key::Char('f'), ctrl: true, .. } | Input { key: Key::PageDown, .. } => {
                ta.scroll(Scrolling::PageDown);
                Action::None
            }
            Input { key: Key::Char('b'), ctrl: true, .. } | Input { key: Key::PageUp, .. } => {
                ta.scroll(Scrolling::PageUp);
                Action::None
            }
            Input { key: Key::Char(c @ ('y' | 'd' | 'c')), ctrl: false, .. } if self.mode == Mode::Normal => {
                ta.start_selection();
                self.mode = Mode::Operator(c);
                return Action::None;
            }
            Input { key: Key::Char('y'), ctrl: false, .. } if self.mode == Mode::Visual => {
                ta.move_cursor(CursorMove::Forward);
                let start = ta.selection_range().map(|(s, _)| s);
                ta.copy();
                if let Some((r, c)) = start {
                    ta.move_cursor(CursorMove::Jump(r as u16, c as u16));
                }
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('d'), ctrl: false, .. } if self.mode == Mode::Visual => {
                ta.move_cursor(CursorMove::Forward);
                ta.cut();
                self.mode = Mode::Normal;
                return Action::None;
            }
            Input { key: Key::Char('c'), ctrl: false, .. } if self.mode == Mode::Visual => {
                ta.move_cursor(CursorMove::Forward);
                ta.cut();
                self.mode = Mode::Insert;
                return Action::None;
            }
            Input { key: Key::Char(op), ctrl: false, .. } if self.mode == Mode::Operator(op) => {
                // dd / yy / cc: whole line(s)
                let row = ta.cursor().0;
                Self::select_line(ta);
                for _ in 1..n {
                    ta.move_cursor(CursorMove::Down);
                }
                if op == 'y' {
                    self.apply_operator(op, ta);
                    ta.move_cursor(CursorMove::Jump(row as u16, 0));
                    return Action::None;
                }
                Action::None
            }
            _ => return Action::None,
        };
        // A motion happened. If an operator is pending, apply it.
        if let Mode::Operator(op) = self.mode {
            self.apply_operator(op, ta);
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ta(text: &str) -> TextArea<'static> {
        TextArea::from(text.lines().map(str::to_string).collect::<Vec<_>>())
    }
    fn key(c: char) -> Input {
        Input { key: Key::Char(c), ctrl: false, alt: false, shift: false }
    }
    fn ctrl(c: char) -> Input {
        Input { key: Key::Char(c), ctrl: true, alt: false, shift: false }
    }
    fn esc() -> Input {
        Input { key: Key::Esc, ctrl: false, alt: false, shift: false }
    }
    fn enter() -> Input {
        Input { key: Key::Enter, ctrl: false, alt: false, shift: false }
    }
    fn feed(v: &mut Vim, t: &mut TextArea<'static>, keys: &str) -> Vec<Action> {
        keys.chars().map(|c| v.input(key(c), t)).collect()
    }

    #[test]
    fn insert_and_escape() {
        let mut v = Vim::new();
        let mut t = ta("abc");
        feed(&mut v, &mut t, "ixy");
        assert_eq!(v.mode, Mode::Insert);
        assert_eq!(t.lines()[0], "xyabc");
        v.input(esc(), &mut t);
        assert_eq!(v.mode, Mode::Normal);
        feed(&mut v, &mut t, "A!");
        v.input(esc(), &mut t);
        assert_eq!(t.lines()[0], "xyabc!");
    }

    #[test]
    fn dd_yy_p_and_undo() {
        let mut v = Vim::new();
        let mut t = ta("one\ntwo\nthree");
        feed(&mut v, &mut t, "jdd");
        assert_eq!(t.lines(), ["one", "three"]);
        assert_eq!(v.mode, Mode::Normal);
        feed(&mut v, &mut t, "u");
        assert_eq!(t.lines(), ["one", "two", "three"]);
        feed(&mut v, &mut t, "ggyyp");
        assert_eq!(t.lines(), ["one", "one", "two", "three"]);
        assert_eq!((t.cursor().0, t.cursor().1), (1, 0));
        feed(&mut v, &mut t, "Gp");
        assert_eq!(t.lines(), ["one", "one", "two", "three", "one"]);
    }

    #[test]
    fn motions_counts_and_x() {
        let mut v = Vim::new();
        let mut t = ta("hello world\nline2");
        feed(&mut v, &mut t, "3l");
        assert_eq!((t.cursor().0, t.cursor().1), (0, 3));
        feed(&mut v, &mut t, "0wx");
        assert_eq!(t.lines()[0], "hello orld");
        feed(&mut v, &mut t, "$");
        assert_eq!(t.cursor().1, "hello orld".len() - 1);
        feed(&mut v, &mut t, "jgg");
        assert_eq!((t.cursor().0, t.cursor().1), (0, 0));
        feed(&mut v, &mut t, "G");
        assert_eq!(t.cursor().0, 1);
    }

    #[test]
    fn dw_and_cw() {
        let mut v = Vim::new();
        let mut t = ta("alpha beta gamma");
        feed(&mut v, &mut t, "dw");
        assert_eq!(t.lines()[0], "beta gamma");
        assert_eq!(v.mode, Mode::Normal);
        feed(&mut v, &mut t, "cw");
        assert_eq!(v.mode, Mode::Insert);
        feed(&mut v, &mut t, "X");
        assert_eq!(t.lines()[0], "Xgamma");
    }

    #[test]
    fn search_prompt_and_ex_commands() {
        let mut v = Vim::new();
        let mut t = ta("aaa\nbbb needle\nccc");
        v.input(key('/'), &mut t);
        assert!(v.prompt.is_some());
        feed(&mut v, &mut t, "needle");
        let a = v.input(enter(), &mut t);
        assert_eq!(a, Action::None);
        assert_eq!(t.cursor().0, 1);
        v.input(key(':'), &mut t);
        feed(&mut v, &mut t, "wq");
        assert_eq!(v.input(enter(), &mut t), Action::SaveAndClose);
        v.input(key(':'), &mut t);
        feed(&mut v, &mut t, "q!");
        assert_eq!(v.input(enter(), &mut t), Action::CloseTab { force: true });
        assert_eq!(v.input(ctrl('s'), &mut t), Action::Save);
        assert_eq!(v.input(esc(), &mut t), Action::Blur);
        assert_eq!(v.input(key(' '), &mut t), Action::Leader);
        assert_eq!(v.input(key('?'), &mut t), Action::Help);
    }

    #[test]
    fn visual_yank_and_replace() {
        let mut v = Vim::new();
        let mut t = ta("abcd");
        feed(&mut v, &mut t, "vly");
        assert_eq!(v.mode, Mode::Normal);
        assert_eq!(t.yank_text(), "ab");
        feed(&mut v, &mut t, "rZ");
        assert_eq!(t.lines()[0], "Zbcd");
        assert_eq!(v.mode, Mode::Normal);
    }
}
