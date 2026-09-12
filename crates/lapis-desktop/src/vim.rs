//! Source-based Vim commands. The native widget owns text, IME and undo history.
use crate::motion as m;
use std::ops::Range;

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::VisualLine => "VISUAL LINE",
        }
    }
}
#[derive(Default)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
    pub generation: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Pass,
    Select(Range<usize>),
    Edit { range: Range<usize>, text: String, cursor: usize },
    Undo,
    Redo,
    Save,
    Close(bool),
    SaveClose,
    Status(String),
}
#[derive(Default)]
pub struct Vim {
    pub mode: Mode,
    pub prompt: Option<(char, String)>,
    count: usize,
    operator: Option<(char, usize)>,
    pending: Option<char>,
    anchor: usize,
    head: usize,
    selection: Range<usize>,
    search: String,
}
impl Vim {
    pub fn label(&self) -> String {
        if let Some((kind, text)) = &self.prompt {
            format!("{kind}{text}")
        } else if let Some((op, n)) = self.operator {
            format!("{n}{op}{}", if self.count > 0 { self.count.to_string() } else { String::new() })
        } else if let Some(pending) = self.pending {
            format!("{} {pending}", self.mode.label())
        } else if self.count > 0 {
            format!("{} {}", self.mode.label(), self.count)
        } else {
            self.mode.label().into()
        }
    }
    fn visual(&self) -> bool {
        matches!(self.mode, Mode::Visual | Mode::VisualLine)
    }
    pub fn reset(&mut self) {
        self.mode = Mode::Normal;
        self.prompt = None;
        self.count = 0;
        self.operator = None;
        self.pending = None;
    }
    fn edit(range: Range<usize>, text: String) -> Effect {
        let cursor = range.start;
        Effect::Edit { range, text, cursor }
    }
    fn selected(&mut self, s: &str) -> Range<usize> {
        let (a, b) = (self.anchor.min(self.head), self.anchor.max(self.head));
        self.selection = if self.mode == Mode::VisualLine {
            m::start(s, a)..m::next(s, m::end(s, b))
        } else {
            a..m::next(s, b)
        };
        self.selection.clone()
    }
    fn operate(
        &mut self,
        op: char,
        range: Range<usize>,
        linewise: bool,
        s: &str,
        register: &mut Register,
    ) -> Effect {
        if range.is_empty() {
            self.reset();
            if op == 'c' {
                self.mode = Mode::Insert;
            }
            return Effect::None;
        }
        register.generation = register.generation.wrapping_add(1);
        register.text = s[range.clone()].into();
        register.linewise = linewise;
        if linewise && !register.text.ends_with('\n') {
            register.text.push('\n');
        }
        self.reset();
        if op == 'y' {
            Effect::Select(range.start..range.start)
        } else {
            if op == 'c' {
                self.mode = Mode::Insert;
            }
            // Keep the line separator for cc so inserted text remains on its own line.
            let range = if op == 'c' && linewise && s[range.clone()].ends_with('\n') {
                range.start..range.end - 1
            } else {
                range
            };
            Self::edit(range, String::new())
        }
    }
    pub fn key(
        &mut self,
        key: &str,
        control: bool,
        s: &str,
        cursor: usize,
        selection: Range<usize>,
        register: &mut Register,
    ) -> Effect {
        let cursor = m::clip(s, cursor);
        let selection = m::clip(s, selection.start)..m::clip(s, selection.end);
        self.anchor = m::clip(s, self.anchor);
        self.head = m::clip(s, self.head);
        if key == "escape" {
            let p = if self.mode == Mode::Insert && cursor > m::start(s, cursor) {
                m::previous(s, cursor)
            } else {
                cursor
            };
            self.reset();
            return Effect::Select(p..p);
        }
        if let Some((kind, text)) = self.prompt.as_mut() {
            match key {
                "enter" => {
                    let (kind, text) = self.prompt.take().unwrap();
                    return if kind == '/' {
                        self.search = text;
                        self.find(s, cursor, false)
                    } else {
                        match text.trim() {
                            "w" => Effect::Save,
                            "q" => Effect::Close(false),
                            "q!" => Effect::Close(true),
                            "wq" | "x" => Effect::SaveClose,
                            other => Effect::Status(format!("Unknown command: {other}")),
                        }
                    };
                }
                "backspace" => {
                    text.pop();
                }
                _ if !control && key.chars().count() == 1 => text.push_str(key),
                "space" => text.push(' '),
                _ => return Effect::Status(format!("{kind} prompt accepts one line")),
            }
            return Effect::None;
        }
        if self.mode == Mode::Insert {
            return Effect::Pass;
        }
        if control {
            return match key {
                "r" => Effect::Redo,
                _ => Effect::Pass,
            };
        }
        if self.visual() && selection != self.selection {
            // A native mouse gesture supersedes the old keyboard selection.
            self.anchor = selection.start;
            self.head = if selection.is_empty() { cursor } else { m::previous(s, selection.end) };
        }
        let p = if self.visual() { self.head.min(s.len()) } else { cursor };
        if self.pending == Some('r') {
            self.pending = None;
            if key.chars().count() == 1 {
                if p == m::end(s, p) {
                    return Effect::None;
                }
                let range = p..m::next(s, p);
                return Self::edit(range, key.into());
            }
            return Effect::Status("Replace cancelled: expected a character".into());
        }
        // Counts before and after an operator multiply, with a bounded repetition limit.
        if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() && (key != "0" || self.count > 0) {
            self.count = (self.count.saturating_mul(10) + usize::from(key.as_bytes()[0] - b'0')).min(10_000);
            return Effect::None;
        }
        let counted = self.count > 0;
        let n = std::mem::take(&mut self.count).max(1);
        if key == ":" || key == "/" {
            self.operator = None;
            self.prompt = Some((key.chars().next().unwrap(), String::new()));
            return Effect::None;
        }
        if key == "g" {
            if self.pending == Some('g') {
                self.pending = None;
                return self.motion(s, p, m::vertical(s, 0, true, n - 1), false, true, register);
            }
            self.pending = Some('g');
            self.count = n;
            return Effect::None;
        }
        self.pending = None;
        if matches!(key, "d" | "c" | "y") {
            let op = key.chars().next().unwrap();
            if self.visual() {
                let r = self.selected(s);
                return self.operate(op, r, self.mode == Mode::VisualLine, s, register);
            }
            if !selection.is_empty() {
                return self.operate(op, selection, false, s, register);
            }
            if let Some((old, count)) = self.operator.take()
                && old == op
            {
                return self.operate(op, m::line_range(s, p, (count * n).min(10_000)), true, s, register);
            }
            self.operator = Some((op, n));
            return Effect::None;
        }
        let repetitions = self.operator.map_or(n, |(_, count)| (count * n).min(10_000));
        let target = match key {
            "h" | "left" => {
                Some((0..repetitions).fold(p, |p, _| if p > m::start(s, p) { m::previous(s, p) } else { p }))
            }
            "l" | "right" => Some((0..repetitions).fold(p, |p, _| m::next(s, p).min(m::end(s, p)))),
            "j" | "down" => Some(m::vertical(s, p, true, repetitions)),
            "k" | "up" => Some(m::vertical(s, p, false, repetitions)),
            "0" | "home" => Some(m::start(s, p)),
            "^" => Some(m::first(s, p)),
            "$" | "end" => Some(m::end(s, p)),
            "G" => Some(if counted { m::vertical(s, 0, true, n - 1) } else { m::start(s, s.len()) }),
            "w" | "W"
                if self.operator.is_some_and(|(op, _)| op == 'c')
                    && !s[p..].starts_with(char::is_whitespace) =>
            {
                let q = m::word_end_here(s, p, key == "W");
                Some((1..repetitions).fold(q, |q, _| m::word(s, q, if key == "W" { 'E' } else { 'e' })))
            }
            "w" | "W" | "b" | "B" | "e" | "E" => {
                Some((0..repetitions).fold(p, |p, _| m::word(s, p, key.chars().next().unwrap())))
            }
            _ => None,
        };
        if let Some(target) = target {
            return self.motion(
                s,
                p,
                target,
                matches!(key, "e" | "E")
                    || (matches!(key, "w" | "W")
                        && self.operator.is_some_and(|(op, _)| op == 'c')
                        && !s[p..].starts_with(char::is_whitespace)),
                matches!(key, "j" | "k" | "G"),
                register,
            );
        }
        self.operator = None;
        match key {
            "i" | "a" | "I" | "A" => {
                self.mode = Mode::Insert;
                let q = match key {
                    "a" => m::next(s, p).min(m::end(s, p)),
                    "I" => m::first(s, p),
                    "A" => m::end(s, p),
                    _ => p,
                };
                Effect::Select(q..q)
            }
            "o" | "O" => {
                self.mode = Mode::Insert;
                let q = if key == "o" { m::end(s, p) } else { m::start(s, p) };
                let line = &s[m::start(s, p)..m::end(s, p)];
                let indent = &line[..line.find(|c: char| !c.is_whitespace()).unwrap_or(line.len())];
                Effect::Edit {
                    range: q..q,
                    text: if key == "o" { format!("\n{indent}") } else { format!("{indent}\n") },
                    cursor: q + indent.len() + usize::from(key == "o"),
                }
            }
            "v" | "V" => {
                if self.visual() {
                    self.reset();
                    Effect::Select(p..p)
                } else {
                    self.mode = if key == "V" { Mode::VisualLine } else { Mode::Visual };
                    self.anchor = p;
                    self.head = p;
                    Effect::Select(self.selected(s))
                }
            }
            "x" | "delete" => {
                let r = if self.visual() {
                    self.selected(s)
                } else if !selection.is_empty() {
                    selection
                } else {
                    p..(0..n).fold(p, |p, _| m::next(s, p).min(m::end(s, p)))
                };
                self.operate('d', r, false, s, register)
            }
            "D" | "C" => {
                self.operate(if key == "D" { 'd' } else { 'c' }, p..m::end(s, p), false, s, register)
            }
            "Y" => self.operate('y', m::line_range(s, p, n), true, s, register),
            "u" => Effect::Undo,
            "r" => {
                self.pending = Some('r');
                Effect::None
            }
            "p" | "P" => {
                if register.text.is_empty() {
                    return Effect::Status("Register is empty; use system paste for external text".into());
                }
                if register.text.len().saturating_mul(n) > 32 * 1024 * 1024 {
                    return Effect::Status("Repeated paste exceeds 32 MiB".into());
                }
                let mut text = register.text.repeat(n);
                let range = if self.visual() {
                    self.selected(s)
                } else if !selection.is_empty() {
                    selection
                } else {
                    let q = if register.linewise {
                        if key == "P" { m::start(s, p) } else { m::next(s, m::end(s, p)) }
                    } else if key == "P" {
                        p
                    } else {
                        m::next(s, p).min(m::end(s, p))
                    };
                    if register.linewise && key == "p" && q == s.len() && !s.ends_with('\n') {
                        text.insert(0, '\n');
                    }
                    q..q
                };
                self.reset();
                Self::edit(range, text)
            }
            "n" => self.find(s, p, false),
            "N" => self.find(s, p, true),
            "J" => {
                let e = m::end(s, p);
                if e == s.len() {
                    Effect::None
                } else {
                    let q = m::first(s, e + 1);
                    Self::edit(e..q, " ".into())
                }
            }
            _ => Effect::Status(format!("Unmapped normal-mode key: {key}")),
        }
    }
    fn motion(
        &mut self,
        s: &str,
        p: usize,
        q: usize,
        inclusive: bool,
        linewise: bool,
        register: &mut Register,
    ) -> Effect {
        if let Some((op, _)) = self.operator.take() {
            let (a, b) = (p.min(q), p.max(q));
            let r = if linewise {
                m::start(s, a)..m::next(s, m::end(s, b))
            } else {
                a..if inclusive { m::next(s, b) } else { b }
            };
            self.operate(op, r, linewise, s, register)
        } else if self.visual() {
            let q = m::normal_cursor(s, q);
            self.head = q;
            Effect::Select(self.selected(s))
        } else {
            let q = m::normal_cursor(s, q);
            Effect::Select(q..q)
        }
    }
    fn find(&mut self, s: &str, p: usize, back: bool) -> Effect {
        if self.search.is_empty() {
            return Effect::Status("No previous search".into());
        }
        let found = if back {
            s[..p].rfind(&self.search).or_else(|| s[p..].rfind(&self.search).map(|i| p + i))
        } else {
            let next = m::next(s, p);
            s[next..].find(&self.search).map(|i| next + i).or_else(|| s[..next].find(&self.search))
        };
        found.map_or_else(|| Effect::Status(format!("Not found: {}", self.search)), |p| Effect::Select(p..p))
    }
}

/// Clipboard payloads remain one literal native edit; never replay them as keys.
pub fn normalize_paste(text: &str) -> Result<String, &'static str> {
    if text.len() > 32 * 1024 * 1024 {
        return Err("Paste exceeds 32 MiB; no text was inserted");
    }
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Editor {
        vim: Vim,
        register: Register,
        text: String,
        cursor: usize,
        selected: Range<usize>,
    }
    impl Editor {
        fn new(s: &str) -> Self {
            Self {
                vim: Vim::default(),
                register: Register::default(),
                text: s.into(),
                cursor: 0,
                selected: 0..0,
            }
        }
        fn key(&mut self, key: &str) -> Effect {
            let effect =
                self.vim.key(key, false, &self.text, self.cursor, self.selected.clone(), &mut self.register);
            match &effect {
                Effect::Select(r) => {
                    self.selected = r.clone();
                    self.cursor = r.end;
                }
                Effect::Edit { range, text, cursor } => {
                    self.text.replace_range(range.clone(), text);
                    self.cursor = *cursor;
                    self.selected = *cursor..*cursor;
                }
                _ => {}
            }
            assert!(self.text.is_char_boundary(self.cursor));
            effect
        }
        fn keys(&mut self, keys: &[&str]) {
            for key in keys {
                self.key(key);
            }
        }
    }
    #[test]
    fn motions_keep_combining_sequences_and_emoji_intact() {
        let mut e = Editor::new("e\u{301} 👩‍💻 漢字\nnext");
        e.key("l");
        assert_eq!(e.cursor, "e\u{301}".len());
        e.keys(&["l", "x"]);
        assert_eq!(e.text, "e\u{301}  漢字\nnext");
        assert_eq!(e.register.text, "👩‍💻");
        e.key("p");
        assert_eq!(e.text, "e\u{301}  👩‍💻漢字\nnext");
    }
    #[test]
    fn operators_multiply_counts_and_share_a_register() {
        let mut e = Editor::new("one two three four five six seven\n");
        e.keys(&["2", "d", "3", "w"]);
        assert_eq!(e.text, "seven\n");
        assert_eq!(e.register.text, "one two three four five six ");
        e.key("P");
        assert_eq!(e.text, "one two three four five six seven\n");
    }
    #[test]
    fn line_yank_and_paste_preserve_missing_final_newline() {
        let mut e = Editor::new("first\nlast");
        e.keys(&["y", "y", "G", "p"]);
        assert_eq!(e.text, "first\nlast\nfirst\n");
    }
    #[test]
    fn change_line_preserves_following_line_and_enters_insert() {
        let mut e = Editor::new("  first\nsecond\n");
        e.keys(&["c", "c"]);
        assert_eq!(e.text, "\nsecond\n");
        assert_eq!(e.vim.mode, Mode::Insert);
        assert_eq!(e.key("x"), Effect::Pass);
    }
    #[test]
    fn change_word_does_not_consume_trailing_space() {
        let mut e = Editor::new("first second");
        e.keys(&["c", "w"]);
        assert_eq!(e.text, " second");
        assert_eq!(e.vim.mode, Mode::Insert);
    }
    #[test]
    fn visual_motion_can_reverse_and_delete_exact_graphemes() {
        let mut e = Editor::new("a漢🙂z");
        e.keys(&["l", "l", "v", "h", "d"]);
        assert_eq!(e.text, "az");
        assert_eq!(e.register.text, "漢🙂");
        assert_eq!(e.cursor, 1);
    }
    #[test]
    fn native_mouse_selection_is_an_operator_target() {
        let mut e = Editor::new("prefix selected suffix");
        e.selected = 7..15;
        e.cursor = 15;
        e.key("d");
        assert_eq!(e.text, "prefix  suffix");
        assert_eq!(e.register.text, "selected");
    }
    #[test]
    fn gg_and_replace_pending_state_are_separate() {
        let mut e = Editor::new("first\nsecond");
        e.keys(&["G", "g", "g"]);
        assert_eq!(e.cursor, 0);
        e.keys(&["r", "漢"]);
        assert_eq!(e.text, "漢irst\nsecond");
    }
    #[test]
    fn prompts_do_not_edit_the_document_and_search_wraps() {
        let mut e = Editor::new("needle\nhay needle");
        e.keys(&["/", "n", "e", "e", "d", "l", "e", "enter"]);
        assert_eq!(e.cursor, 11);
        e.key("n");
        assert_eq!(e.cursor, 0);
        e.keys(&[":", "w", "q"]);
        assert_eq!(e.key("enter"), Effect::SaveClose);
        assert_eq!(e.text, "needle\nhay needle");
        e.keys(&[":", "q", "!"]);
        assert_eq!(e.key("enter"), Effect::Close(true));
    }
    #[test]
    fn literal_paste_preserves_line_structure_and_command_text() {
        let text = "# Test\r\n\r\n  item\t漢🙂\r\n:q!\r\n";
        assert_eq!(normalize_paste(text).unwrap(), "# Test\n\n  item\t漢🙂\n:q!\n");
        let payload = "x".repeat(1024 * 1024);
        assert_eq!(normalize_paste(&payload).unwrap(), payload);
    }
    #[test]
    fn open_line_retains_existing_indentation() {
        let mut e = Editor::new("  first\nsecond");
        e.key("o");
        assert_eq!(e.text, "  first\n  \nsecond");
        assert_eq!(e.cursor, 10);
    }
    #[test]
    fn change_single_letter_and_whitespace_are_local() {
        let mut e = Editor::new("a word");
        e.keys(&["c", "w"]);
        assert_eq!(e.text, " word");
        let mut e = Editor::new("  word");
        e.keys(&["c", "w"]);
        assert_eq!(e.text, "word");
    }
    #[test]
    fn linewise_paste_before_a_note_never_joins_lines() {
        let mut e = Editor::new("first\nlast");
        e.keys(&["G", "y", "y", "g", "g", "P"]);
        assert_eq!(e.text, "last\nfirst\nlast");
    }

    #[test]
    fn counted_line_jumps_and_linewise_gg_delete() {
        let mut e = Editor::new("one\ntwo\nthree\nfour");
        e.keys(&["3", "g", "g"]);
        assert_eq!(e.cursor, 8);
        e.keys(&["d", "g", "g"]);
        assert_eq!(e.text, "four");
        let mut e = Editor::new("one\ntwo\nthree\nfour");
        e.keys(&["2", "G"]);
        assert_eq!(e.cursor, 4);
    }
    #[test]
    fn normal_end_stays_on_text_and_delete_reaches_end() {
        let mut e = Editor::new("a漢🙂\nnext");
        e.key("$");
        assert_eq!(e.cursor, 4);
        e.key("x");
        assert_eq!(e.text, "a漢\nnext");
        e.keys(&["0", "d", "$"]);
        assert_eq!(e.text, "\nnext");
        let mut e = Editor::new("");
        e.keys(&["r", "x"]);
        assert_eq!(e.text, "");
    }
    #[test]
    fn empty_delete_does_not_destroy_the_register() {
        let mut e = Editor::new("");
        e.register.text = "keep".into();
        e.key("x");
        assert_eq!(e.register.text, "keep");
        assert_eq!(e.register.generation, 0);
        e.keys(&["c", "c"]);
        assert_eq!(e.vim.mode, Mode::Insert);
    }

    #[test]
    fn stale_visual_offsets_after_native_undo_are_clamped() {
        let mut e = Editor::new("a long prior note");
        e.keys(&["v", "$"]);
        e.text = "短".into();
        e.cursor = 20;
        e.selected = 10..20;
        e.keys(&["h", "d"]);
        assert_eq!(e.text, "");
    }
}
