//! `lapis tui`: Ratatui shell. Sidebar = the real tree (lazy, walk-rule
//! skips), editor = ratatui-textarea, palette = lattice `/search`, HAL
//! inspector, hop-1 neighbors, daily note, trash, `$EDITOR`.
//!
//! Keys (also on the status bar):
//!   Tab focus sidebar/editor · j/k · Enter open/expand · h collapse
//!   Ctrl+P or / search palette · Ctrl+S save · Ctrl+Q quit · Esc back
//!   Space y HAL inspector · Space g neighbors · Space d daily
//!   Space t trash selected · Space l e open in $EDITOR
//!
//! Async lattice calls run on the tokio runtime and post messages to the UI
//! thread; the UI loop itself is `block_in_place` so it never starves them.
//! The watcher watches directories non-recursively (root, top-level dirs,
//! the open note's dir): a few dozen descriptors, never the media tree.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use notify::{RecursiveMode, Watcher};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::TextArea;

use crate::error::{LapisError, Result};
use crate::lattice::{Client, Hit, Mode, Neighbor, SearchParams};
use crate::ops::Ctx;
use crate::{hal, notes, overlay, tasks, write};

// ------------------------------------------------------------------ tree

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub rel: String,
    pub name: String,
    pub is_dir: bool,
}

/// Lazily loaded tree. `children` holds listed dirs; `expanded` which are open.
#[derive(Default)]
pub struct Tree {
    pub children: HashMap<String, Vec<Entry>>,
    pub expanded: HashSet<String>,
}

pub fn list_dir(root: &Path, rel: &str) -> Vec<Entry> {
    let dir = if rel.is_empty() { root.to_path_buf() } else { root.join(rel) };
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(ft) = e.file_type() else { continue };
        let is_dir = ft.is_dir();
        if tasks::excluded(&child, is_dir, &name) {
            continue;
        }
        if !is_dir
            && !matches!(notes::kind_of(&name), notes::Kind::Markdown | notes::Kind::Pdf | notes::Kind::Html)
        {
            continue;
        }
        out.push(Entry { rel: child, name, is_dir });
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

impl Tree {
    pub fn load(&mut self, root: &Path, rel: &str) {
        if !self.children.contains_key(rel) {
            let kids = list_dir(root, rel);
            self.children.insert(rel.to_string(), kids);
        }
    }
    pub fn reload(&mut self, root: &Path, rel: &str) {
        self.children.remove(rel);
        self.load(root, rel);
    }
    /// Flattened visible rows: (depth, entry).
    pub fn visible(&self) -> Vec<(usize, Entry)> {
        let mut out = Vec::new();
        self.walk("", 0, &mut out);
        out
    }
    fn walk(&self, rel: &str, depth: usize, out: &mut Vec<(usize, Entry)>) {
        if let Some(kids) = self.children.get(rel) {
            for k in kids {
                out.push((depth, k.clone()));
                if k.is_dir && self.expanded.contains(&k.rel) {
                    self.walk(&k.rel, depth + 1, out);
                }
            }
        }
    }
}

// ------------------------------------------------------------------- app

enum Msg {
    Search(u64, std::result::Result<Vec<Hit>, String>),
    Neighbors(String, std::result::Result<Vec<Neighbor>, String>),
    Reindexed(String, std::result::Result<Option<u64>, String>),
    Fs(PathBuf),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Sidebar,
    Editor,
    Palette,
}

struct Open {
    rel: String,
    text: TextArea<'static>,
    hal: serde_json::Map<String, serde_json::Value>,
    hal_valid: bool,
    dirty: bool,
}

struct Palette {
    input: String,
    results: Vec<Hit>,
    sel: usize,
    seq: u64,
    pending: bool,
}

struct App {
    ctx: Ctx,
    client: Option<Client>,
    tree: Tree,
    sel: usize,
    focus: Focus,
    open: Option<Open>,
    palette: Option<Palette>,
    show_hal: bool,
    show_neighbors: bool,
    neighbors: Vec<Neighbor>,
    neighbors_for: String,
    /// `Space` was pressed; the next one or two keys form a chord.
    chord_armed: bool,
    chord: Vec<char>,
    status: String,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    trash_bucket: String,
    quit: bool,
}

/// Chord table: a `Space` prefix followed by one or two keys.
pub fn chord_action(keys: &[char]) -> Option<&'static str> {
    match keys {
        ['y'] => Some("hal"),
        ['g'] => Some("neighbors"),
        ['d'] => Some("daily"),
        ['t'] => Some("trash"),
        ['l', 'e'] => Some("editor"),
        _ => None,
    }
}

/// True when more keys could still complete a chord.
pub fn chord_prefix(keys: &[char]) -> bool {
    matches!(keys, ['l'])
}

impl App {
    fn new(ctx: Ctx) -> Self {
        let (tx, rx) = channel();
        let client = ctx.client().ok();
        let trash_bucket = overlay::load(&ctx.vault.root)
            .map(|(o, _)| o.buckets.trash)
            .unwrap_or_else(|_| ".lapis/trash".into());
        let mut tree = Tree::default();
        tree.load(&ctx.vault.root, "");
        let mut app = App {
            ctx,
            client,
            tree,
            sel: 0,
            focus: Focus::Sidebar,
            open: None,
            palette: None,
            show_hal: false,
            show_neighbors: false,
            neighbors: vec![],
            neighbors_for: String::new(),
            chord_armed: false,
            chord: vec![],
            status:
                "Ctrl+P search · Enter open · Space y/g/d/t · Space l e $EDITOR · Ctrl+S save · Ctrl+Q quit"
                    .into(),
            tx,
            rx,
            watcher: None,
            watched: HashSet::new(),
            trash_bucket,
            quit: false,
        };
        app.start_watcher();
        app
    }

    fn root(&self) -> &Path {
        &self.ctx.vault.root
    }

    // ---- watcher: non-recursive, capped, failure is a status line
    fn start_watcher(&mut self) {
        let tx = self.tx.clone();
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                for p in ev.paths {
                    let _ = tx.send(Msg::Fs(p));
                }
            }
        }) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => {
                self.status = format!("watcher off: {e}");
                return;
            }
        }
        let root = self.root().to_path_buf();
        self.watch_dir(&root);
        let top: Vec<PathBuf> = self
            .tree
            .children
            .get("")
            .map(|k| k.iter().filter(|e| e.is_dir).map(|e| root.join(&e.rel)).collect())
            .unwrap_or_default();
        for d in top {
            self.watch_dir(&d);
        }
    }

    fn watch_dir(&mut self, dir: &Path) {
        const CAP: usize = 256;
        if self.watched.contains(dir) || self.watched.len() >= CAP {
            return;
        }
        if let Some(w) = self.watcher.as_mut() {
            match w.watch(dir, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(dir.to_path_buf());
                }
                Err(e) => self.status = format!("watch {} failed: {e}", dir.display()),
            }
        }
    }

    // ---- notes
    fn open_note(&mut self, rel: &str) {
        match notes::read(self.root(), rel) {
            Ok(n) => {
                let mut ta = TextArea::from(n.body.lines().map(str::to_string).collect::<Vec<_>>());
                ta.set_cursor_line_style(Style::default());
                ta.set_line_number_style(Style::default().fg(Color::DarkGray));
                self.open = Some(Open {
                    rel: n.path.clone(),
                    text: ta,
                    hal: n.hal,
                    hal_valid: n.hal_valid,
                    dirty: false,
                });
                self.status = format!("{}  ({} bytes)", n.path, n.size);
                if let Some(parent) = self.root().join(&n.path).parent().map(Path::to_path_buf) {
                    self.watch_dir(&parent);
                }
                if self.show_neighbors {
                    self.fetch_neighbors();
                }
            }
            Err(e) => self.status = format!("open failed: {e}"),
        }
    }

    /// Frontmatter + marker are kept verbatim; only the body is edited.
    fn save(&mut self) {
        if self.open.as_ref().is_some_and(|o| notes::kind_of(&o.rel) == notes::Kind::Pdf) {
            self.status = "PDF text is read-only".into();
            return;
        }
        let Some(o) = self.open.as_mut() else { return };
        let abs = self.ctx.vault.root.join(&o.rel);
        let current = std::fs::read_to_string(&abs).unwrap_or_default();
        let head_len = current.len() - hal::parse(&current).body.len();
        let head = &current[..head_len];
        let mut body = o.text.lines().join("\n");
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let next = write::set_frontmatter_key(&format!("{head}{body}"), "updated", &write::today());
        match std::fs::write(&abs, next) {
            Ok(()) => {
                o.dirty = false;
                let rel = o.rel.clone();
                self.status = format!("saved {rel}");
                self.kick(rel);
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    fn kick(&self, rel: String) {
        let Some(client) = self.client.clone() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = client.reindex(&rel).await.map(|r| r.chunks).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Reindexed(rel, r));
        });
    }

    fn fetch_neighbors(&mut self) {
        let Some(o) = &self.open else { return };
        let Some(client) = self.client.clone() else {
            self.status = "lattice client unavailable".into();
            return;
        };
        let rel = o.rel.clone();
        self.neighbors_for = rel.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r =
                client.neighbors(&rel, "both", true).await.map(|n| n.neighbors).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Neighbors(rel, r));
        });
    }

    fn search(&mut self) {
        let Some(p) = self.palette.as_mut() else { return };
        let q = p.input.trim().to_string();
        if q.is_empty() {
            p.results.clear();
            return;
        }
        let Some(client) = self.client.clone() else {
            self.status = "lattice client unavailable".into();
            return;
        };
        p.seq += 1;
        p.pending = true;
        let seq = p.seq;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let params = SearchParams {
                query: q,
                top_k: 20,
                domain: None,
                mode: Mode::Bm25,
                per_doc: true,
                mmr: false,
                include_archives: false,
            };
            let r = client.search(&params).await.map(|r| r.hits).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Search(seq, r));
        });
    }

    fn daily(&mut self) {
        match write::daily(self.root(), None, self.ctx.cfg.operator.name.clone()) {
            Ok(d) => {
                if d.created {
                    self.kick(d.path.clone());
                    self.tree.reload(&self.ctx.vault.root.clone(), "Daily");
                    self.tree.reload(&self.ctx.vault.root.clone(), "");
                }
                self.open_note(&d.path);
                self.focus = Focus::Editor;
            }
            Err(e) => self.status = format!("daily failed: {e}"),
        }
    }

    fn trash_selected(&mut self) {
        let rows = self.tree.visible();
        let Some((_, e)) = rows.get(self.sel) else { return };
        if e.is_dir {
            self.status = "trash works on notes, not folders".into();
            return;
        }
        let root = self.ctx.vault.root.clone();
        match write::trash(&root, &e.rel, &self.trash_bucket) {
            Ok(t) => {
                let parent =
                    Path::new(&t.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                self.tree.reload(&root, &parent);
                if self.open.as_ref().is_some_and(|o| o.rel == t.path) {
                    self.open = None;
                }
                self.sel = self.sel.min(self.tree.visible().len().saturating_sub(1));
                self.status = format!("trashed -> {}", t.trashed_to);
            }
            Err(e) => self.status = format!("trash failed: {e}"),
        }
    }

    fn external_editor(&mut self, term: &mut DefaultTerminal) {
        let Some(o) = &self.open else {
            self.status = "open a note first".into();
            return;
        };
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let abs = self.root().join(&o.rel);
        let rel = o.rel.clone();
        ratatui::restore();
        let status = std::process::Command::new(&editor).arg(&abs).status();
        *term = ratatui::init();
        match status {
            Ok(_) => {
                self.open_note(&rel);
                self.kick(rel);
            }
            Err(e) => self.status = format!("{editor}: {e}"),
        }
    }

    // ---- messages from tasks / watcher
    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Search(seq, r) => {
                    if let Some(p) = self.palette.as_mut()
                        && seq == p.seq
                    {
                        p.pending = false;
                        match r {
                            Ok(hits) => {
                                p.results = hits;
                                p.sel = 0;
                            }
                            Err(e) => self.status = format!("search: {e}"),
                        }
                    }
                }
                Msg::Neighbors(rel, r) => {
                    if rel == self.neighbors_for {
                        match r {
                            Ok(n) => self.neighbors = n,
                            Err(e) => self.status = format!("neighbors: {e}"),
                        }
                    }
                }
                Msg::Reindexed(rel, r) => {
                    self.status = match r {
                        Ok(Some(n)) => format!("{rel}: indexed ({n} chunks)"),
                        Ok(None) => format!("{rel}: index unchanged"),
                        Err(e) => format!("{rel}: reindex failed: {e}"),
                    }
                }
                Msg::Fs(path) => {
                    let root = self.ctx.vault.root.clone();
                    if let Ok(rel) = path.strip_prefix(&root) {
                        let rel = rel.to_string_lossy().to_string();
                        let parent = Path::new(&rel)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if self.tree.children.contains_key(&parent) {
                            self.tree.reload(&root, &parent);
                        }
                        if let Some(o) = &self.open
                            && o.rel == rel
                            && !o.dirty
                        {
                            self.open_note(&rel);
                        }
                    }
                }
            }
        }
    }

    // ---- keys
    fn key(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (ctrl, k.code) {
            (true, KeyCode::Char('q')) => {
                self.quit = true;
                return;
            }
            (true, KeyCode::Char('p')) => {
                self.palette =
                    Some(Palette { input: String::new(), results: vec![], sel: 0, seq: 0, pending: false });
                self.focus = Focus::Palette;
                return;
            }
            (true, KeyCode::Char('s')) => {
                self.save();
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Palette => self.key_palette(k),
            Focus::Editor => self.key_editor(k),
            Focus::Sidebar => self.key_sidebar(k, term),
        }
    }

    fn key_palette(&mut self, k: KeyEvent) {
        let Some(p) = self.palette.as_mut() else { return };
        match k.code {
            KeyCode::Esc => {
                self.palette = None;
                self.focus = Focus::Sidebar;
            }
            KeyCode::Enter => {
                if let Some(h) = p.results.get(p.sel).cloned() {
                    self.palette = None;
                    self.open_note(&h.path);
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Down => p.sel = (p.sel + 1).min(p.results.len().saturating_sub(1)),
            KeyCode::Up => p.sel = p.sel.saturating_sub(1),
            KeyCode::Backspace => {
                p.input.pop();
                self.search();
            }
            KeyCode::Char(c) => {
                p.input.push(c);
                self.search();
            }
            _ => {}
        }
    }

    fn key_editor(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc | KeyCode::Tab => self.focus = Focus::Sidebar,
            _ => {
                if let Some(o) = self.open.as_mut()
                    && o.text.input(k)
                {
                    o.dirty = true;
                }
            }
        }
    }

    fn key_sidebar(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        // Space-chords first.
        if self.chord_armed {
            if let KeyCode::Char(c) = k.code {
                self.chord.push(c);
                let keys = self.chord.clone();
                if let Some(action) = chord_action(&keys) {
                    self.chord_armed = false;
                    self.chord.clear();
                    match action {
                        "hal" => self.show_hal = !self.show_hal,
                        "neighbors" => {
                            self.show_neighbors = !self.show_neighbors;
                            if self.show_neighbors {
                                self.fetch_neighbors();
                            }
                        }
                        "daily" => self.daily(),
                        "trash" => self.trash_selected(),
                        "editor" => self.external_editor(term),
                        _ => {}
                    }
                } else if !chord_prefix(&keys) {
                    self.chord_armed = false;
                    self.chord.clear();
                    self.status = format!("unknown chord: Space {}", keys.iter().collect::<String>());
                }
            } else {
                self.chord_armed = false;
                self.chord.clear();
            }
            return;
        }
        let rows = self.tree.visible();
        match k.code {
            KeyCode::Char(' ') => {
                self.chord_armed = true;
                self.chord.clear();
                self.status = "Space: y HAL · g neighbors · d daily · t trash · l e $EDITOR".into();
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('/') => self.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL), term),
            KeyCode::Tab => {
                if self.open.is_some() {
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                if let Some((_, e)) = rows.get(self.sel).cloned() {
                    if e.is_dir {
                        let root = self.ctx.vault.root.clone();
                        if !self.tree.expanded.remove(&e.rel) {
                            self.tree.load(&root, &e.rel);
                            self.tree.expanded.insert(e.rel.clone());
                            self.watch_dir(&root.join(&e.rel));
                        }
                    } else {
                        self.open_note(&e.rel);
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

    // ---- draw
    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let [main, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let [side, right] = Layout::horizontal([Constraint::Percentage(28), Constraint::Fill(1)]).areas(main);
        self.draw_sidebar(f, side);
        let (editor_area, hal_area) = if self.show_hal {
            let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(35)]).areas(right);
            (a, Some(b))
        } else {
            (right, None)
        };
        let (editor_area, nb_area) = if self.show_neighbors {
            let [a, b] = Layout::vertical([Constraint::Fill(1), Constraint::Length(10)]).areas(editor_area);
            (a, Some(b))
        } else {
            (editor_area, None)
        };
        self.draw_editor(f, editor_area);
        if let Some(a) = hal_area {
            self.draw_hal(f, a);
        }
        if let Some(a) = nb_area {
            self.draw_neighbors(f, a);
        }
        let dirty = self.open.as_ref().is_some_and(|o| o.dirty);
        let s = Line::from(vec![
            Span::styled(if dirty { " ● " } else { "   " }, Style::default().fg(Color::Yellow)),
            Span::raw(self.status.clone()),
        ]);
        f.render_widget(
            Paragraph::new(s).style(Style::default().bg(Color::Rgb(31, 45, 104)).fg(Color::White)),
            status,
        );
        if self.palette.is_some() {
            self.draw_palette(f, area);
        }
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let rows = self.tree.visible();
        let items: Vec<ListItem> = rows
            .iter()
            .map(|(depth, e)| {
                let pad = "  ".repeat(*depth);
                let glyph = if e.is_dir {
                    if self.tree.expanded.contains(&e.rel) { "▾ " } else { "▸ " }
                } else {
                    "  "
                };
                let style = if e.is_dir { Style::default().fg(Color::Cyan) } else { Style::default() };
                ListItem::new(Line::from(Span::styled(format!("{pad}{glyph}{}", e.name), style)))
            })
            .collect();
        let focused = self.focus == Focus::Sidebar;
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " {} ",
                self.ctx.vault.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
            ))
            .border_style(if focused { Style::default().fg(Color::Yellow) } else { Style::default() });
        let mut state = ListState::default().with_selected(Some(self.sel.min(rows.len().saturating_sub(1))));
        f.render_stateful_widget(
            List::new(items).block(block).highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            area,
            &mut state,
        );
    }

    fn draw_editor(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Editor;
        let border = if focused { Style::default().fg(Color::Yellow) } else { Style::default() };
        match self.open.as_mut() {
            Some(o) => {
                let title = format!(" {}{} ", o.rel, if o.dirty { " *" } else { "" });
                o.text.set_block(Block::default().borders(Borders::ALL).title(title).border_style(border));
                f.render_widget(&o.text, area);
            }
            None => {
                let p = Paragraph::new("Enter on a note to open it · Ctrl+P to search the lattice")
                    .block(Block::default().borders(Borders::ALL).title(" Lapis ").border_style(border))
                    .wrap(Wrap { trim: true });
                f.render_widget(p, area);
            }
        }
    }

    fn draw_hal(&self, f: &mut Frame, area: Rect) {
        let text = match &self.open {
            Some(o) if o.hal_valid => write::to_yaml(&o.hal).unwrap_or_default(),
            Some(_) => "(frontmatter is not valid YAML)".into(),
            None => String::new(),
        };
        let p = Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title(" HAL "))
            .wrap(Wrap { trim: false });
        f.render_widget(p, area);
    }

    fn draw_neighbors(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.neighbors.is_empty() {
            vec![ListItem::new("(no hop-1 neighbors, or still loading)")]
        } else {
            self.neighbors
                .iter()
                .map(|n| {
                    let arrow = if n.direction == "in" { "<- " } else { "-> " };
                    ListItem::new(format!("{arrow}{}", n.path))
                })
                .collect()
        };
        f.render_widget(
            List::new(items).block(Block::default().borders(Borders::ALL).title(" hop-1 neighbors ")),
            area,
        );
    }

    fn draw_palette(&self, f: &mut Frame, area: Rect) {
        let Some(p) = &self.palette else { return };
        let w = area.width.saturating_mul(3) / 4;
        let h = (area.height * 2 / 3).max(8);
        let popup = Rect { x: (area.width - w) / 2, y: (area.height - h) / 2, width: w, height: h };
        f.render_widget(Clear, popup);
        let [input, list] = Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(popup);
        let title = if p.pending { " search (lattice…) " } else { " search (lattice) " };
        f.render_widget(
            Paragraph::new(format!("> {}", p.input))
                .block(Block::default().borders(Borders::ALL).title(title)),
            input,
        );
        let items: Vec<ListItem> = p
            .results
            .iter()
            .map(|h| {
                ListItem::new(Line::from(vec![
                    Span::styled(h.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  {}", h.path), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(p.sel));
        f.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL))
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
            list,
            &mut state,
        );
    }
}

pub async fn run(ctx: Ctx) -> Result<()> {
    tokio::task::block_in_place(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            ratatui::restore();
            default_hook(info);
        }));
        let mut term = ratatui::init();
        let mut app = App::new(ctx);
        let result = ui_loop(&mut app, &mut term);
        ratatui::restore();
        result
    })
}

fn ui_loop(app: &mut App, term: &mut DefaultTerminal) -> Result<()> {
    while !app.quit {
        app.drain();
        term.draw(|f| app.draw(f)).map_err(|e| LapisError::Internal(format!("draw: {e}")))?;
        if event::poll(Duration::from_millis(80)).map_err(|e| LapisError::Internal(e.to_string()))?
            && let Event::Key(k) = event::read().map_err(|e| LapisError::Internal(e.to_string()))?
        {
            app.key(k, term);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords() {
        assert_eq!(chord_action(&['y']), Some("hal"));
        assert_eq!(chord_action(&['l', 'e']), Some("editor"));
        assert_eq!(chord_action(&['l']), None);
        assert!(chord_prefix(&['l']));
        assert!(!chord_prefix(&['z']));
    }

    #[test]
    fn tree_lists_lazily_and_skips_excluded() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let v = std::env::temp_dir().join(format!("lapis-tui-{}-{n}", std::process::id()));
        for d in ["foundry/lapis", "agents", "Aeon/notes/media", ".obsidian", "_archives"] {
            std::fs::create_dir_all(v.join(d)).unwrap();
        }
        std::fs::write(v.join("foundry/lapis/SPEC.md"), "x").unwrap();
        std::fs::write(v.join("foundry/lapis/pic.png"), "x").unwrap();
        std::fs::write(v.join("Aeon/notes/media/a.md"), "x").unwrap();
        std::fs::write(v.join("README.md"), "x").unwrap();
        let mut t = Tree::default();
        t.load(&v, "");
        let names: Vec<String> = t.visible().iter().map(|(_, e)| e.name.clone()).collect();
        assert_eq!(names, ["Aeon", "agents", "foundry", "README.md"]);
        t.load(&v, "foundry");
        t.expanded.insert("foundry".into());
        t.load(&v, "foundry/lapis");
        t.expanded.insert("foundry/lapis".into());
        let rows = t.visible();
        let lapis: Vec<(usize, String)> =
            rows.iter().filter(|(d, _)| *d == 2).map(|(d, e)| (*d, e.name.clone())).collect();
        assert_eq!(lapis, [(2, "SPEC.md".to_string())], "png hidden, depth tracked");
        assert!(list_dir(&v, "Aeon/notes/media").is_empty(), "media never listed");
        let _ = std::fs::remove_dir_all(&v);
    }
}
