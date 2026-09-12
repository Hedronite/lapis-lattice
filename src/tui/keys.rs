//! Key, overlay, leader, and mouse dispatch.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui_textarea::Input;

use crate::templates;
use crate::write;
use lapis_lattice::Mode as SearchMode;

use super::app::{App, Focus, Overlay, PromptKind, Split};
use super::leader::{self, Cmd};
use super::palette::{Item, Palette};
use super::preview;
use super::tags_view::Pick;
use super::tasks_view::View;
use super::theme;
use super::vim::{self, Action};

pub(crate) fn key_input(k: KeyEvent) -> Input {
    Input::from(k)
}

impl App {
    pub(crate) fn run(&mut self, cmd: Cmd, term: &mut DefaultTerminal) {
        match cmd {
            Cmd::FindNote => self.overlay = Some(Overlay::Palette(Palette::new(""))),
            Cmd::SearchText => self.overlay = Some(Overlay::Palette(Palette::new(""))),
            Cmd::Commands => self.overlay = Some(Overlay::Palette(Palette::new(">"))),
            Cmd::ToggleSidebar => {
                self.show_sidebar = !self.show_sidebar;
                if !self.show_sidebar && self.focus == Focus::Sidebar {
                    self.focus = if self.tabs.is_empty() { Focus::Sidebar } else { Focus::Editor };
                }
            }
            Cmd::Outline => {
                if let Some(t) = self.tab() {
                    let o = preview::outline(&t.body());
                    self.overlay = Some(Overlay::Outline(0, o));
                }
            }
            Cmd::TogglePreview | Cmd::ToggleSplit => {
                self.split = if self.split == Split::Both { Split::EditorOnly } else { Split::Both };
            }
            Cmd::EditorOnly => self.split = Split::EditorOnly,
            Cmd::PreviewOnly => {
                self.split = Split::PreviewOnly;
                if self.focus == Focus::Editor {
                    self.focus = Focus::Preview;
                }
            }
            Cmd::ToggleWrap => self.wrap = !self.wrap,
            Cmd::ToggleLineNumbers => {
                self.line_numbers = !self.line_numbers;
                let ln = self.line_numbers;
                for t in &mut self.tabs {
                    if ln {
                        t.text.set_line_number_style(theme::dim());
                    } else {
                        t.text.remove_line_number();
                    }
                }
            }
            Cmd::Kanban => self.open_tasks(View::Kanban),
            Cmd::TaskList => self.open_tasks(View::List),
            Cmd::Calendar => self.open_tasks(View::Calendar),
            Cmd::NotesView => {
                self.tasks = None;
                self.focus = if self.tabs.is_empty() { Focus::Sidebar } else { Focus::Editor };
            }
            Cmd::Daily => self.periodic(write::Period::Daily),
            Cmd::Weekly => self.periodic(write::Period::Weekly),
            Cmd::Monthly => self.periodic(write::Period::Monthly),
            Cmd::NewNote => {
                self.overlay =
                    Some(Overlay::Prompt("New note title".into(), String::new(), PromptKind::NewNote));
            }
            Cmd::NewFromTemplate => {
                let list = templates::list(&self.root());
                self.overlay = Some(Overlay::Templates(0, list));
            }
            Cmd::QuickCapture => {
                self.overlay =
                    Some(Overlay::Prompt("Quick capture".into(), String::new(), PromptKind::QuickCapture));
            }
            Cmd::ExternalEditor => self.external_editor(term),
            Cmd::TrashNote => self.trash_current(),
            Cmd::RestorePicker => {
                let list = write::trash_list(&self.root(), &self.trash_bucket);
                if list.is_empty() {
                    self.set_status("trash is empty");
                } else {
                    self.overlay = Some(Overlay::Restore(0, list));
                }
            }
            Cmd::CopyPath => {
                if let Some(t) = self.tab() {
                    let rel = t.rel.clone();
                    self.copy_system(rel);
                }
            }
            Cmd::CopySelection => self.copy_selection(false),
            Cmd::CutSelection => self.copy_selection(true),
            Cmd::PasteClipboard => self.paste_system(),
            Cmd::ToggleMouse => {
                self.mouse_capture = !self.mouse_capture;
                let result = if self.mouse_capture {
                    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
                } else {
                    crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
                };
                self.set_status(match result {
                    Ok(()) if self.mouse_capture => "mouse capture on: drag to select text".into(),
                    Ok(()) => "mouse capture off: terminal selection; Space z m restores capture".into(),
                    Err(e) => format!("mouse capture: {e}"),
                });
            }
            Cmd::Neighbors => {
                self.show_neighbors = !self.show_neighbors;
                if self.show_neighbors {
                    self.fetch_neighbors();
                }
            }
            Cmd::Hal => self.show_hal = !self.show_hal,
            Cmd::Tags => self.open_tags(),
            Cmd::Theme => self.next_theme(),
            Cmd::Buffers => {
                if !self.tabs.is_empty() {
                    self.overlay = Some(Overlay::Buffers(self.active));
                }
            }
            Cmd::TabNext => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                    self.after_open();
                }
            }
            Cmd::TabPrev => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                    self.after_open();
                }
            }
            Cmd::TabClose => self.close_tab(false),
            Cmd::Save => self.save(),
            Cmd::SaveCopy => self.save_copy(),
            Cmd::Help => self.overlay = Some(Overlay::Help(0)),
            Cmd::Quit => self.request_quit(),
            Cmd::Refresh => {
                let root = self.root();
                let dirs: Vec<String> = self.tree.children.keys().cloned().collect();
                for d in dirs {
                    self.tree.reload(&root, &d);
                }
                if self.tasks.is_some() {
                    self.scan_tasks();
                }
                self.poll_health();
                self.set_status("refreshed");
            }
        }
    }

    pub(crate) fn key(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
            return;
        }
        if k.code == KeyCode::F(6) {
            self.run(Cmd::ToggleMouse, term);
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        if self.overlay.is_some() {
            self.key_overlay(k, term);
            return;
        }
        // Editor prompt lines (/ and :) swallow everything.
        let in_prompt = self.focus == Focus::Editor && self.tab().is_some_and(|t| t.vim.prompt.is_some());
        if !in_prompt {
            if ctrl && matches!(k.code, KeyCode::Char('c' | 'C')) {
                self.copy_selection(false);
                return;
            }
            if ctrl && matches!(k.code, KeyCode::Char('v' | 'V')) {
                self.paste_system();
                return;
            }
            if ctrl && matches!(k.code, KeyCode::Char('x' | 'X')) {
                self.copy_selection(true);
                return;
            }
            match (ctrl, alt, k.code) {
                (true, _, KeyCode::Char('q')) => {
                    self.request_quit();
                    return;
                }
                (true, _, KeyCode::Char('p')) => {
                    self.overlay = Some(Overlay::Palette(Palette::new("")));
                    return;
                }
                (true, _, KeyCode::Char('w')) => {
                    self.overlay = Some(Overlay::Pane);
                    return;
                }
                (_, true, KeyCode::Char(c)) if c.is_ascii_digit() => {
                    let n = (c as u8 - b'1') as usize;
                    if n < self.tabs.len() {
                        self.active = n;
                        self.after_open();
                    }
                    return;
                }
                (false, false, KeyCode::Tab) if self.focus != Focus::Editor || self.editor_is_normal() => {
                    self.cycle_focus(false);
                    return;
                }
                (false, false, KeyCode::BackTab) => {
                    self.cycle_focus(true);
                    return;
                }
                _ => {}
            }
        }
        match self.focus {
            Focus::Sidebar => self.key_sidebar(k, term),
            Focus::Editor => self.key_editor(k, term),
            Focus::Preview => self.key_preview(k, term),
            Focus::Tasks => self.key_tasks(k, term),
        }
    }

    pub(crate) fn editor_is_normal(&self) -> bool {
        self.tab().is_none_or(|t| t.vim.mode == vim::Mode::Normal)
    }

    pub(crate) fn cycle_focus(&mut self, back: bool) {
        let mut order = vec![];
        if self.show_sidebar {
            order.push(Focus::Sidebar);
        }
        if self.tasks.is_some() {
            order.push(Focus::Tasks);
        } else if !self.tabs.is_empty() {
            if self.split != Split::PreviewOnly {
                order.push(Focus::Editor);
            }
            if self.split != Split::EditorOnly {
                order.push(Focus::Preview);
            }
        }
        if order.is_empty() {
            return;
        }
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        self.focus = order[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    pub(crate) fn leader_step(&mut self, keys: Vec<char>, term: &mut DefaultTerminal) {
        match leader::step(&keys) {
            leader::Step::Pending => self.overlay = Some(Overlay::Leader(keys)),
            leader::Step::Run(cmd) => {
                self.overlay = None;
                self.run(cmd, term);
            }
            leader::Step::Unknown => {
                self.overlay = None;
                self.set_status(format!("unknown chord: Space {}", keys.iter().collect::<String>()));
            }
        }
    }

    pub(crate) fn key_overlay(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        let Some(overlay) = self.overlay.take() else { return };
        match overlay {
            Overlay::Leader(mut keys) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Char(c) => {
                    keys.push(c);
                    self.leader_step(keys, term);
                }
                _ => self.overlay = Some(Overlay::Leader(keys)),
            },
            Overlay::Pane => match k.code {
                KeyCode::Char('h') | KeyCode::Left => self.cycle_focus(true),
                KeyCode::Char('l') | KeyCode::Right => self.cycle_focus(false),
                KeyCode::Char('<') => self.sidebar_pct = self.sidebar_pct.saturating_sub(4).max(12),
                KeyCode::Char('>') => self.sidebar_pct = (self.sidebar_pct + 4).min(60),
                KeyCode::Char('-') => self.preview_pct = self.preview_pct.saturating_sub(10).max(20),
                KeyCode::Char('+') | KeyCode::Char('=') => self.preview_pct = (self.preview_pct + 10).min(80),
                KeyCode::Char('v') => self.split = Split::Both,
                KeyCode::Char('o') => self.split = Split::EditorOnly,
                KeyCode::Char('q') | KeyCode::Char('c') => self.close_tab(false),
                _ => {}
            },
            Overlay::Palette(mut p) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => match p.selected().cloned() {
                    Some(Item::Note { path, .. }) => {
                        self.open_note(&path);
                        self.tasks = None;
                        self.focus = Focus::Editor;
                    }
                    Some(Item::Command { cmd, .. }) => self.run(cmd, term),
                    None => self.overlay = Some(Overlay::Palette(p)),
                },
                KeyCode::Down => {
                    p.down();
                    self.overlay = Some(Overlay::Palette(p));
                }
                KeyCode::Up => {
                    p.up();
                    self.overlay = Some(Overlay::Palette(p));
                }
                KeyCode::Char('n') | KeyCode::Char('j') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    p.down();
                    self.overlay = Some(Overlay::Palette(p));
                }
                KeyCode::Char('p') | KeyCode::Char('k') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    p.up();
                    self.overlay = Some(Overlay::Palette(p));
                }
                KeyCode::Backspace => {
                    p.input.pop();
                    if p.is_commands() {
                        p.refresh_commands();
                    } else if p.input.is_empty() {
                        p.items.clear();
                    }
                    self.overlay = Some(Overlay::Palette(p));
                    self.lattice_search(SearchMode::Bm25);
                }
                KeyCode::Char(c) => {
                    p.input.push(c);
                    if p.is_commands() {
                        p.refresh_commands();
                    }
                    self.overlay = Some(Overlay::Palette(p));
                    self.lattice_search(SearchMode::Bm25);
                }
                _ => self.overlay = Some(Overlay::Palette(p)),
            },
            Overlay::Help(scroll) => match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {}
                KeyCode::Char('j') | KeyCode::Down => self.overlay = Some(Overlay::Help(scroll + 1)),
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Help(scroll.saturating_sub(1)))
                }
                KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.overlay = Some(Overlay::Help(scroll + 10));
                }
                KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.overlay = Some(Overlay::Help(scroll.saturating_sub(10)));
                }
                _ => self.overlay = Some(Overlay::Help(scroll)),
            },
            Overlay::Outline(sel, items) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some((line, _, _)) = items.get(sel)
                        && let Some(t) = self.tab_mut()
                    {
                        t.text.move_cursor(ratatui_textarea::CursorMove::Jump(*line as u16, 0));
                        t.reader.scroll = *line;
                    }
                    self.focus = Focus::Editor;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let n = items.len();
                    self.overlay = Some(Overlay::Outline((sel + 1).min(n.saturating_sub(1)), items));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Outline(sel.saturating_sub(1), items))
                }
                _ => self.overlay = Some(Overlay::Outline(sel, items)),
            },
            Overlay::Buffers(sel) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    self.active = sel.min(self.tabs.len().saturating_sub(1));
                    self.focus = Focus::Editor;
                    self.after_open();
                }
                KeyCode::Char('x') | KeyCode::Char('d') => {
                    self.active = sel;
                    self.close_tab(false);
                    if !self.tabs.is_empty() {
                        self.overlay = Some(Overlay::Buffers(self.active));
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.overlay = Some(Overlay::Buffers((sel + 1).min(self.tabs.len().saturating_sub(1))));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Buffers(sel.saturating_sub(1)))
                }
                _ => self.overlay = Some(Overlay::Buffers(sel)),
            },
            Overlay::Restore(sel, items) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(p) = items.get(sel).cloned() {
                        self.restore(&p);
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let n = items.len();
                    self.overlay = Some(Overlay::Restore((sel + 1).min(n.saturating_sub(1)), items));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Restore(sel.saturating_sub(1), items))
                }
                _ => self.overlay = Some(Overlay::Restore(sel, items)),
            },
            Overlay::Tags(mut b) => match k.code {
                KeyCode::Esc => {
                    if b.back() {
                        self.overlay = Some(Overlay::Tags(b));
                    }
                }
                KeyCode::Enter => match b.enter() {
                    Pick::Note(path) => {
                        self.open_note(&path);
                        self.tasks = None;
                        self.focus = Focus::Editor;
                    }
                    Pick::Tag(_) | Pick::Nothing => self.overlay = Some(Overlay::Tags(b)),
                },
                KeyCode::Down => {
                    b.down();
                    self.overlay = Some(Overlay::Tags(b));
                }
                KeyCode::Up => {
                    b.up();
                    self.overlay = Some(Overlay::Tags(b));
                }
                KeyCode::Char('j') if b.open.is_some() || k.modifiers.contains(KeyModifiers::CONTROL) => {
                    b.down();
                    self.overlay = Some(Overlay::Tags(b));
                }
                KeyCode::Char('k') if b.open.is_some() || k.modifiers.contains(KeyModifiers::CONTROL) => {
                    b.up();
                    self.overlay = Some(Overlay::Tags(b));
                }
                KeyCode::Backspace => {
                    b.backspace();
                    self.overlay = Some(Overlay::Tags(b));
                }
                KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    b.type_char(c);
                    self.overlay = Some(Overlay::Tags(b));
                }
                _ => self.overlay = Some(Overlay::Tags(b)),
            },
            Overlay::Templates(sel, items) => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if items.get(sel).is_some() {
                        self.overlay = Some(Overlay::Prompt(
                            "New note title".into(),
                            String::new(),
                            PromptKind::NewFromTemplate(sel),
                        ));
                        // keep the template list alive through the prompt
                        self.pending_templates = Some(items);
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let n = items.len();
                    self.overlay = Some(Overlay::Templates((sel + 1).min(n.saturating_sub(1)), items));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.overlay = Some(Overlay::Templates(sel.saturating_sub(1), items))
                }
                _ => self.overlay = Some(Overlay::Templates(sel, items)),
            },
            Overlay::Prompt(title, mut input, kind) => match k.code {
                KeyCode::Esc => {
                    self.pending_templates = None;
                }
                KeyCode::Enter => {
                    let text = input.trim().to_string();
                    if text.is_empty() {
                        self.overlay = Some(Overlay::Prompt(title, input, kind));
                        return;
                    }
                    match kind {
                        PromptKind::NewNote => self.create_note(&text, None),
                        PromptKind::NewFromTemplate(i) => {
                            let id =
                                self.pending_templates.take().and_then(|l| l.get(i).map(|t| t.id.clone()));
                            self.create_note(&text, id);
                        }
                        PromptKind::QuickCapture => self.capture(&text),
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.overlay = Some(Overlay::Prompt(title, input, kind));
                }
                KeyCode::Char(c) => {
                    input.push(c);
                    self.overlay = Some(Overlay::Prompt(title, input, kind));
                }
                _ => self.overlay = Some(Overlay::Prompt(title, input, kind)),
            },
        }
    }

    pub(crate) fn key_sidebar(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        let rows = self.tree.visible();
        let root = self.root();
        if self.pending_g {
            self.pending_g = false;
            if let KeyCode::Char('g') = k.code {
                self.sel = 0;
            }
            return;
        }
        match k.code {
            KeyCode::Char(' ') => self.overlay = Some(Overlay::Leader(vec![])),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Char('/') => self.overlay = Some(Overlay::Palette(Palette::new(""))),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.sel = rows.len().saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.sel = (self.sel + 10).min(rows.len().saturating_sub(1));
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.sel = self.sel.saturating_sub(10)
            }
            KeyCode::Char('x') => self.trash_current(),
            KeyCode::Char('n') => self.run(Cmd::NewNote, term),
            KeyCode::Char('r') => {
                if let Some((_, e)) = rows.get(self.sel) {
                    let d = if e.is_dir {
                        e.rel.clone()
                    } else {
                        Path::new(&e.rel)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default()
                    };
                    self.tree.reload(&root, &d);
                }
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char('o') | KeyCode::Right => {
                if let Some((_, e)) = rows.get(self.sel).cloned() {
                    if e.is_dir {
                        if !self.tree.expanded.remove(&e.rel) {
                            self.tree.load(&root, &e.rel);
                            self.tree.expanded.insert(e.rel.clone());
                            self.watch_dir(&root.join(&e.rel));
                        }
                    } else {
                        self.open_note(&e.rel);
                        self.tasks = None;
                        if matches!(k.code, KeyCode::Enter) {
                            self.focus = Focus::Editor;
                        }
                    }
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some((_, e)) = rows.get(self.sel).cloned() {
                    if e.is_dir && self.tree.expanded.remove(&e.rel) {
                        return;
                    }
                    if let Some(parent) = Path::new(&e.rel).parent().map(|p| p.to_string_lossy().to_string())
                        && !parent.is_empty()
                        && let Some(i) = rows.iter().position(|(_, x)| x.rel == parent)
                    {
                        self.sel = i;
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn key_editor(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        let accepts_register =
            self.tab().is_some_and(|t| t.vim.mode != vim::Mode::Insert && t.vim.prompt.is_none());
        if accepts_register && !k.modifiers.contains(KeyModifiers::CONTROL) {
            if self.clipboard_register == 1 {
                self.clipboard_register = if matches!(k.code, KeyCode::Char('+' | '*')) { 2 } else { 0 };
                return;
            }
            if k.code == KeyCode::Char('"') {
                self.clipboard_register = 1;
                return;
            }
            if self.clipboard_register == 2 && k.code == KeyCode::Char('p') {
                self.clipboard_register = 0;
                self.paste_system();
                return;
            }
        }
        if k.code == KeyCode::Esc {
            self.clipboard_register = 0;
        }
        let Some(t) = self.tabs.get_mut(self.active) else {
            self.focus = Focus::Sidebar;
            return;
        };
        if t.readonly && t.vim.prompt.is_none() {
            // Read-only buffers: navigation only.
            if let KeyCode::Char(
                'i' | 'a' | 'o' | 'O' | 'I' | 'A' | 'x' | 'd' | 'D' | 'c' | 'C' | 'p' | 'r' | 'R' | 'J' | 'u',
            ) = k.code
            {
                self.set_status("read-only buffer");
                return;
            }
        }
        if t.vim.prompt.is_none() {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            let undo = k.code == KeyCode::Char('u')
                && ((t.vim.mode != vim::Mode::Insert && !ctrl) || (t.vim.mode == vim::Mode::Insert && ctrl));
            let redo = k.code == KeyCode::Char('r') && ctrl;
            if undo || redo {
                t.undo_edit(redo);
                return;
            }
        }
        let before_cursor = t.text.cursor();
        let was_yank = (t.vim.mode == vim::Mode::Visual && k.code == KeyCode::Char('y'))
            || t.vim.mode == vim::Mode::Operator('y');
        let action = t.vim.input(key_input(k), &mut t.text);
        t.record_edit((before_cursor.0, before_cursor.1));
        let copied = if was_yank && t.vim.mode == vim::Mode::Normal && self.clipboard_register == 2 {
            Some(t.text.yank_text())
        } else {
            None
        };
        if let Some(payload) = copied {
            self.clipboard_register = 0;
            self.copy_system(payload);
        }
        match action {
            Action::None => {}
            Action::Save => self.save(),
            Action::CloseTab { force } => self.close_tab(force),
            Action::SaveAndClose => {
                self.save();
                self.close_tab(false);
            }
            Action::Blur => self.focus = if self.show_sidebar { Focus::Sidebar } else { Focus::Preview },
            Action::ToggleCheckbox => self.toggle_checkbox_at_cursor(),
            Action::Status(s) => self.set_status(s),
            Action::NextTab => self.run(Cmd::TabNext, term),
            Action::PrevTab => self.run(Cmd::TabPrev, term),
            Action::Leader => self.overlay = Some(Overlay::Leader(vec![])),
            Action::Help => self.overlay = Some(Overlay::Help(0)),
        }
    }

    pub(crate) fn key_preview(&mut self, k: KeyEvent, _term: &mut DefaultTerminal) {
        let page = self.regions.preview.height.saturating_sub(2).max(1);
        let Some(t) = self.tabs.get_mut(self.active) else {
            self.focus = Focus::Sidebar;
            return;
        };
        if k.code == KeyCode::Char('y') {
            self.copy_selection(false);
            return;
        }
        if t.reader.selecting() && !k.modifiers.contains(KeyModifiers::CONTROL) {
            let movement = match k.code {
                KeyCode::Char('h') | KeyCode::Left => Some((-1, 0)),
                KeyCode::Char('l') | KeyCode::Right => Some((1, 0)),
                KeyCode::Char('j') | KeyCode::Down => Some((0, 1)),
                KeyCode::Char('k') | KeyCode::Up => Some((0, -1)),
                _ => None,
            };
            if let Some((x, y)) = movement {
                t.reader.move_selection(x, y, page as usize);
                return;
            }
        }
        match k.code {
            KeyCode::Char(' ') => self.overlay = Some(Overlay::Leader(vec![])),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
            KeyCode::Esc if t.reader.selecting() => t.reader.clear_selection(),
            KeyCode::Esc => self.focus = if self.show_sidebar { Focus::Sidebar } else { Focus::Editor },
            KeyCode::Char('v') => t.reader.begin_selection(),
            KeyCode::Char('j') | KeyCode::Down => t.reader.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => t.reader.scroll_by(-1),
            KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                t.reader.scroll_by((page / 2) as isize)
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                t.reader.scroll_by(-((page / 2) as isize))
            }
            KeyCode::PageDown => t.reader.scroll_by(page as isize),
            KeyCode::PageUp => t.reader.scroll_by(-(page as isize)),
            KeyCode::Char('G') => t.reader.end(),
            KeyCode::Char('g') => t.reader.scroll = 0,
            KeyCode::Char('q') => self.request_quit(),
            _ => {}
        }
    }

    pub(crate) fn key_tasks(&mut self, k: KeyEvent, _term: &mut DefaultTerminal) {
        let Some(tv) = self.tasks.as_mut() else {
            self.focus = Focus::Sidebar;
            return;
        };
        match k.code {
            KeyCode::Char(' ') => self.overlay = Some(Overlay::Leader(vec![])),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
            KeyCode::Esc | KeyCode::Char('q') => {
                self.tasks = None;
                self.focus = if self.tabs.is_empty() { Focus::Sidebar } else { Focus::Editor };
            }
            KeyCode::Char('j') | KeyCode::Down => tv.down(),
            KeyCode::Char('k') | KeyCode::Up => tv.up(),
            KeyCode::Char('h') | KeyCode::Left => tv.left(),
            KeyCode::Char('l') | KeyCode::Right => tv.right(),
            KeyCode::Char('H') => tv.prev_month(),
            KeyCode::Char('L') => tv.next_month(),
            KeyCode::Char('1') => tv.view = View::List,
            KeyCode::Char('2') => tv.view = View::Kanban,
            KeyCode::Char('3') => tv.view = View::Calendar,
            KeyCode::Char('r') => self.scan_tasks(),
            KeyCode::Char('f') => {
                tv.full = !tv.full;
                tv.sel = 0;
            }
            KeyCode::Char('u') => {
                tv.unscope();
                self.scan_tasks();
            }
            KeyCode::Enter if tv.is_summary() => {
                let scoped = tv.scope_to_selected();
                if scoped {
                    self.scan_tasks();
                }
            }
            KeyCode::Char('x') => {
                if let Some(id) = tv.current().map(|t| t.id.clone()) {
                    self.toggle_task(&id);
                }
            }
            KeyCode::Enter => {
                let cur = tv.current().map(|t| {
                    (
                        t.source_path.clone(),
                        t.line_number.unwrap_or(0),
                        t.raw_text.clone().unwrap_or_default(),
                    )
                });
                if let Some((rel, line, raw)) = cur {
                    self.open_note(&rel);
                    if let Some(tab) = self.tab_mut() {
                        // body lines exclude the frontmatter; find the raw line, fall back to its number
                        let idx = tab.text.lines().iter().position(|l| *l == raw).unwrap_or(line);
                        tab.text.move_cursor(ratatui_textarea::CursorMove::Jump(idx as u16, 0));
                    }
                    self.tasks = None;
                    self.focus = Focus::Editor;
                }
            }
            _ => {}
        }
    }
}
