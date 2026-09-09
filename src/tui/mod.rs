//! `lapis tui`: the human surface. Sidebar tree | tabs | Vim editor | rendered
//! preview, leader + which-key, palette, help, mouse, tasks/kanban/calendar,
//! periodic notes, templates, neighbors, HAL inspector, trash + restore.
//!
//! Lattice work runs on the tokio runtime and posts [`Msg`]s; the UI loop is
//! `block_in_place`. The watcher is non-recursive and capped (no EMFILE).

mod hal_view;
mod help;
mod leader;
mod mouse;
mod neighbors_view;
mod palette;
mod preview;
mod tags_view;
mod tasks_view;
mod theme;
mod tree;
mod vim;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseEvent,
};
use notify::{RecursiveMode, Watcher};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use ratatui_textarea::{Input, TextArea};

use crate::error::{LapisError, Result};
use crate::lattice::{Client, Document, Hit, ListParams, Mode as SearchMode, Neighbor, SearchParams};
use crate::ops::Ctx;
use crate::tasks::Task;
use crate::{hal, notes, tasks, templates, write};
use leader::Cmd;
use mouse::{Action as MouseAction, Regions, Target};
use palette::{Item, Palette};
use tags_view::{Pick, TagsBrowser};
use tasks_view::{TasksView, View};
use tree::Tree;
use vim::{Action, Vim};

// ------------------------------------------------------------------ messages

enum Msg {
    Search(u64, std::result::Result<Vec<Hit>, String>),
    Neighbors(String, std::result::Result<Vec<Neighbor>, String>),
    Documents(std::result::Result<Vec<Document>, String>),
    Reindexed(String, std::result::Result<Option<u64>, String>),
    Tasks(Vec<Task>),
    Health(bool),
    Fs(PathBuf),
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Focus {
    Sidebar,
    Editor,
    Preview,
    Tasks,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Split {
    Both,
    EditorOnly,
    PreviewOnly,
}

/// Modal overlays; at most one at a time.
enum Overlay {
    Leader(Vec<char>),
    Palette(Palette),
    Help(usize),
    Outline(usize, Vec<(usize, u8, String)>),
    Buffers(usize),
    Restore(usize, Vec<String>),
    Templates(usize, Vec<templates::Template>),
    Tags(TagsBrowser),
    /// (title, input, purpose)
    Prompt(String, String, PromptKind),
    /// Ctrl+W pane prefix
    Pane,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptKind {
    NewNote,
    NewFromTemplate(usize),
    QuickCapture,
}

struct Tab {
    rel: String,
    text: TextArea<'static>,
    vim: Vim,
    hal: serde_json::Map<String, serde_json::Value>,
    hal_valid: bool,
    dirty: bool,
    preview_scroll: u16,
    preview: Vec<Line<'static>>,
    preview_for: String,
    readonly: bool,
}

impl Tab {
    fn body(&self) -> String {
        self.text.lines().join("\n")
    }
    fn refresh_preview(&mut self) {
        let body = self.body();
        if body != self.preview_for {
            self.preview = preview::render(&body);
            self.preview_for = body;
        }
    }
}

struct App {
    ctx: Ctx,
    client: Option<Client>,
    tree: Tree,
    sel: usize,
    sidebar_scroll: usize,
    show_sidebar: bool,
    sidebar_pct: u16,
    preview_pct: u16,
    split: Split,
    wrap: bool,
    line_numbers: bool,
    focus: Focus,
    tabs: Vec<Tab>,
    active: usize,
    overlay: Option<Overlay>,
    tasks: Option<TasksView>,
    show_hal: bool,
    show_neighbors: bool,
    neighbors: Vec<Neighbor>,
    neighbors_for: String,
    neighbors_sel: usize,
    status: String,
    status_at: Instant,
    lattice_ok: Option<bool>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    watcher: Option<notify::RecommendedWatcher>,
    watched: HashSet<PathBuf>,
    trash_bucket: String,
    regions: Regions,
    pending_g: bool,
    pending_templates: Option<Vec<templates::Template>>,
    quit: bool,
}

fn key_input(k: KeyEvent) -> Input {
    Input::from(k)
}

impl App {
    fn new(ctx: Ctx) -> Self {
        let (tx, rx) = channel();
        let client = ctx.client().ok();
        let trash_bucket = crate::ops::trash_bucket(&ctx);
        let mut tree = Tree::default();
        tree.load(&ctx.vault.root, "");
        let mut app = App {
            ctx,
            client,
            tree,
            sel: 0,
            sidebar_scroll: 0,
            show_sidebar: true,
            sidebar_pct: 24,
            preview_pct: 50,
            split: Split::Both,
            wrap: true,
            line_numbers: true,
            focus: Focus::Sidebar,
            tabs: vec![],
            active: 0,
            overlay: None,
            tasks: None,
            show_hal: false,
            show_neighbors: false,
            neighbors: vec![],
            neighbors_for: String::new(),
            neighbors_sel: 0,
            status: "Space leader · Ctrl+P palette · ? help".into(),
            status_at: Instant::now(),
            lattice_ok: None,
            tx,
            rx,
            watcher: None,
            watched: HashSet::new(),
            trash_bucket,
            regions: Regions::default(),
            pending_g: false,
            pending_templates: None,
            quit: false,
        };
        app.start_watcher();
        app.poll_health();
        app
    }

    fn root(&self) -> PathBuf {
        self.ctx.vault.root.clone()
    }

    fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Instant::now();
    }

    fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }
    fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    // ------------------------------------------------------------ background

    fn poll_health(&self) {
        let Some(client) = self.client.clone() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ok = client.health().await.is_ok();
            let _ = tx.send(Msg::Health(ok));
        });
    }

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
                self.set_status(format!("watcher off: {e}"));
                return;
            }
        }
        let root = self.root();
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
                Err(e) => self.set_status(format!("watch {} failed: {e}", dir.display())),
            }
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
        let Some(rel) = self.tab().map(|t| t.rel.clone()) else { return };
        let Some(client) = self.client.clone() else {
            self.set_status("lattice client unavailable");
            return;
        };
        self.neighbors_for = rel.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r =
                client.neighbors(&rel, "both", true).await.map(|n| n.neighbors).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Neighbors(rel, r));
        });
    }

    fn scan_tasks(&mut self) {
        let root = self.root();
        let tx = self.tx.clone();
        if let Some(t) = self.tasks.as_mut() {
            t.loading = true;
        }
        tokio::task::spawn_blocking(move || {
            let list = tasks::list(&root, &tasks::Filter::default()).unwrap_or_default();
            let _ = tx.send(Msg::Tasks(list));
        });
    }

    /// `Space #`: tags from the lattice document table plus inline task tags.
    fn open_tags(&mut self) {
        self.overlay = Some(Overlay::Tags(TagsBrowser::new()));
        let tx = self.tx.clone();
        match self.client.clone() {
            Some(client) => {
                tokio::spawn(async move {
                    let p = ListParams { limit: 1000, ..ListParams::default() };
                    let r = client.documents(&p).await.map_err(|e| e.to_string());
                    let _ = tx.send(Msg::Documents(r));
                });
            }
            None => self.set_status("lattice client unavailable; tags from tasks only"),
        }
        let root = self.root();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let list = tasks::list(&root, &tasks::Filter::default()).unwrap_or_default();
            let _ = tx.send(Msg::Tasks(list));
        });
    }

    fn lattice_search(&mut self, mode: SearchMode) {
        let Some(Overlay::Palette(p)) = self.overlay.as_mut() else { return };
        let q = p.query();
        if q.is_empty() || p.is_commands() {
            return;
        }
        if q == p.asked {
            return;
        }
        let Some(client) = self.client.clone() else {
            self.set_status("lattice client unavailable");
            return;
        };
        p.seq += 1;
        p.pending = true;
        p.asked = q.clone();
        let seq = p.seq;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let params = SearchParams {
                query: q,
                top_k: 25,
                domain: None,
                mode,
                per_doc: true,
                mmr: false,
                include_archives: false,
            };
            let r = client.search(&params).await.map(|r| r.hits).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Search(seq, r));
        });
    }

    // ------------------------------------------------------------------ tabs

    fn open_note(&mut self, rel: &str) {
        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            self.active = i;
            self.after_open();
            return;
        }
        match notes::read(&self.root(), rel) {
            Ok(n) => {
                let mut ta = TextArea::from(n.body.lines().map(str::to_string).collect::<Vec<_>>());
                ta.set_cursor_line_style(Style::default());
                ta.set_line_number_style(theme::dim());
                ta.set_selection_style(theme::selected());
                ta.set_search_style(Style::default().bg(theme::GOLD).fg(theme::BLUE_DEEP));
                let readonly = n.kind != notes::Kind::Markdown;
                let mut tab = Tab {
                    rel: n.path.clone(),
                    text: ta,
                    vim: Vim::new(),
                    hal: n.hal,
                    hal_valid: n.hal_valid,
                    dirty: false,
                    preview_scroll: 0,
                    preview: vec![],
                    preview_for: String::new(),
                    readonly,
                };
                tab.refresh_preview();
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.set_status(format!("{}  ({} bytes)", n.path, n.size));
                if let Some(parent) = self.root().join(&n.path).parent().map(Path::to_path_buf) {
                    self.watch_dir(&parent);
                }
                self.after_open();
            }
            Err(e) => self.set_status(format!("open failed: {e}")),
        }
    }

    fn after_open(&mut self) {
        if self.show_neighbors {
            self.fetch_neighbors();
        }
        if let Some(rel) = self.tab().map(|t| t.rel.clone()) {
            let root = self.root();
            if let Some(i) = self.tree.reveal(&root, &rel) {
                self.sel = i;
            }
        }
    }

    fn reload_tab(&mut self, rel: &str) {
        let Some(i) = self.tabs.iter().position(|t| t.rel == rel) else { return };
        if let Ok(n) = notes::read(&self.root(), rel) {
            let t = &mut self.tabs[i];
            let cursor = t.text.cursor();
            let mut ta = TextArea::from(n.body.lines().map(str::to_string).collect::<Vec<_>>());
            ta.set_cursor_line_style(Style::default());
            ta.set_line_number_style(theme::dim());
            ta.set_selection_style(theme::selected());
            ta.move_cursor(ratatui_textarea::CursorMove::Jump(cursor.0 as u16, cursor.1 as u16));
            t.text = ta;
            t.hal = n.hal;
            t.hal_valid = n.hal_valid;
            t.dirty = false;
            t.refresh_preview();
        }
    }

    fn close_tab(&mut self, force: bool) {
        if self.tabs.is_empty() {
            return;
        }
        if self.tabs[self.active].dirty && !force {
            self.set_status("unsaved changes: :w first or :q! to discard");
            return;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        if self.tabs.is_empty() {
            self.focus = Focus::Sidebar;
        }
    }

    /// Frontmatter + marker are kept verbatim; only the body is written.
    fn save(&mut self) {
        let Some(t) = self.tab() else { return };
        if t.readonly {
            self.set_status("read-only (PDF/HTML text)");
            return;
        }
        let rel = t.rel.clone();
        let abs = self.root().join(&rel);
        let current = std::fs::read_to_string(&abs).unwrap_or_default();
        let head_len = current.len() - hal::parse(&current).body.len();
        let head = &current[..head_len];
        let mut body = t.body();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let next = write::set_frontmatter_key(&format!("{head}{body}"), "updated", &write::today());
        match std::fs::write(&abs, &next) {
            Ok(()) => {
                let p = hal::parse(&next);
                if let Some(t) = self.tab_mut() {
                    t.dirty = false;
                    t.hal = p.hal;
                    t.hal_valid = p.hal_valid;
                }
                self.set_status(format!("saved {rel}"));
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("save failed: {e}")),
        }
    }

    fn toggle_checkbox_at_cursor(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        let c = t.text.cursor();
        let (row, col) = (c.0, c.1);
        let line = t.text.lines()[row].clone();
        let new_line = if let Some(i) = line.find("[ ]") {
            format!("{}[x]{}", &line[..i], &line[i + 3..])
        } else if let Some(i) = line.find("[x]").or_else(|| line.find("[X]")) {
            format!("{}[ ]{}", &line[..i], &line[i + 3..])
        } else {
            let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
            let rest = line.trim_start();
            let rest = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")).unwrap_or(rest);
            format!("{indent}- [ ] {rest}")
        };
        use ratatui_textarea::CursorMove;
        t.text.move_cursor(CursorMove::Jump(row as u16, 0));
        t.text.delete_line_by_end();
        t.text.insert_str(&new_line);
        t.text.move_cursor(CursorMove::Jump(row as u16, col.min(new_line.chars().count()) as u16));
        t.dirty = true;
    }

    // ------------------------------------------------------------- commands

    fn run(&mut self, cmd: Cmd, term: &mut DefaultTerminal) {
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
                    self.set_status(format!("path: {rel}"));
                }
            }
            Cmd::Neighbors => {
                self.show_neighbors = !self.show_neighbors;
                if self.show_neighbors {
                    self.fetch_neighbors();
                }
            }
            Cmd::Hal => self.show_hal = !self.show_hal,
            Cmd::Tags => self.open_tags(),
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
            Cmd::Help => self.overlay = Some(Overlay::Help(0)),
            Cmd::Quit => self.quit = true,
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

    fn open_tasks(&mut self, view: View) {
        match self.tasks.as_mut() {
            Some(t) => t.view = view,
            None => {
                self.tasks = Some(TasksView::new(view));
                self.scan_tasks();
            }
        }
        self.focus = Focus::Tasks;
    }

    fn periodic(&mut self, period: write::Period) {
        let root = self.root();
        match write::periodic(&root, period, None, self.ctx.cfg.operator.name.clone()) {
            Ok(d) => {
                if d.created {
                    self.kick(d.path.clone());
                    let parent = Path::new(&d.path)
                        .parent()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    self.tree.reload(&root, "");
                    self.tree.reload(&root, &parent);
                }
                self.open_note(&d.path);
                self.focus = Focus::Editor;
            }
            Err(e) => self.set_status(format!("periodic note failed: {e}")),
        }
    }

    fn selected_entry(&self) -> Option<tree::Entry> {
        self.tree.visible().get(self.sel).map(|(_, e)| e.clone())
    }

    /// Folder for a new note: the selected sidebar folder (or its parent), else inbox.
    fn target_folder(&self) -> String {
        match self.selected_entry() {
            Some(e) if e.is_dir => format!("{}/", e.rel),
            Some(e) => {
                Path::new(&e.rel).parent().map(|p| format!("{}/", p.to_string_lossy())).unwrap_or_default()
            }
            None => String::new(),
        }
    }

    fn create_note(&mut self, title: &str, template: Option<String>) {
        let folder = self.target_folder();
        let opts = write::CreateOpts {
            title: title.to_string(),
            path: if folder.is_empty() || folder == "/" { None } else { Some(folder.clone()) },
            template,
            doc_type: None,
            domain: None,
            tags: vec![],
            body: None,
            operator: self.ctx.cfg.operator.name.clone(),
            inbox: self.ctx.inbox().unwrap_or_else(|_| "inbox".into()),
            director: None,
            template_date: None,
        };
        let root = self.root();
        match write::create(&root, &opts) {
            Ok(w) => {
                let parent =
                    Path::new(&w.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                self.tree.reload(&root, &parent);
                self.tree.reload(&root, "");
                self.kick(w.path.clone());
                self.open_note(&w.path);
                self.focus = Focus::Editor;
                if let Some(t) = self.tab_mut() {
                    t.vim.mode = vim::Mode::Insert;
                    t.text.move_cursor(ratatui_textarea::CursorMove::Bottom);
                }
            }
            Err(e) => self.set_status(format!("create failed: {e}")),
        }
    }

    fn capture(&mut self, text: &str) {
        let root = self.root();
        let inbox = self.ctx.inbox().unwrap_or_else(|_| "inbox".into());
        match write::capture(&root, text, &inbox, self.ctx.cfg.operator.name.clone()) {
            Ok(w) => {
                self.tree.reload(&root, &inbox);
                self.tree.reload(&root, "");
                self.kick(w.path.clone());
                self.set_status(format!("captured -> {}", w.path));
            }
            Err(e) => self.set_status(format!("capture failed: {e}")),
        }
    }

    fn trash_current(&mut self) {
        let rel = match self.focus {
            Focus::Sidebar => match self.selected_entry() {
                Some(e) if !e.is_dir => e.rel,
                _ => {
                    self.set_status("select a note to trash");
                    return;
                }
            },
            _ => match self.tab() {
                Some(t) => t.rel.clone(),
                None => return,
            },
        };
        let root = self.root();
        match write::trash(&root, &rel, &self.trash_bucket) {
            Ok(t) => {
                let parent =
                    Path::new(&t.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                self.tree.reload(&root, &parent);
                if let Some(i) = self.tabs.iter().position(|x| x.rel == t.path) {
                    self.tabs.remove(i);
                    if self.active >= self.tabs.len() {
                        self.active = self.tabs.len().saturating_sub(1);
                    }
                    if self.tabs.is_empty() {
                        self.focus = Focus::Sidebar;
                    }
                }
                self.sel = self.sel.min(self.tree.visible().len().saturating_sub(1));
                self.set_status(format!("trashed -> {}  (Space l r restores)", t.trashed_to));
            }
            Err(e) => self.set_status(format!("trash failed: {e}")),
        }
    }

    fn restore(&mut self, trashed_rel: &str) {
        let root = self.root();
        match write::restore(&root, trashed_rel, &self.trash_bucket) {
            Ok(t) => {
                let parent =
                    Path::new(&t.path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                self.tree.reload(&root, &parent);
                self.tree.reload(&root, "");
                self.kick(t.path.clone());
                self.set_status(format!("restored {}", t.path));
                self.open_note(&t.path);
            }
            Err(e) => self.set_status(format!("restore failed: {e}")),
        }
    }

    fn external_editor(&mut self, term: &mut DefaultTerminal) {
        let Some(t) = self.tab() else {
            self.set_status("open a note first");
            return;
        };
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let abs = self.root().join(&t.rel);
        let rel = t.rel.clone();
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        let status = std::process::Command::new(&editor).arg(&abs).status();
        *term = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        match status {
            Ok(_) => {
                self.reload_tab(&rel);
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("{editor}: {e}")),
        }
    }

    fn toggle_task(&mut self, id: &str) {
        let root = self.root();
        match tasks::toggle(&root, id) {
            Ok(t) => {
                let rel = t.source_path.clone();
                if let Some(tv) = self.tasks.as_mut()
                    && let Some(slot) = tv.tasks.iter_mut().find(|x| x.id == t.id)
                {
                    *slot = t;
                }
                self.reload_tab(&rel);
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("toggle failed: {e}")),
        }
    }

    // ------------------------------------------------------------- messages

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Search(seq, r) => {
                    if let Some(Overlay::Palette(p)) = self.overlay.as_mut()
                        && seq == p.seq
                    {
                        match r {
                            Ok(hits) => p.set_hits(hits),
                            Err(e) => {
                                p.pending = false;
                                self.set_status(format!("search: {e}"));
                            }
                        }
                    }
                }
                Msg::Neighbors(rel, r) => {
                    if rel == self.neighbors_for {
                        match r {
                            Ok(n) => {
                                self.neighbors = n;
                                self.neighbors_sel = 0;
                            }
                            Err(e) => self.set_status(format!("neighbors: {e}")),
                        }
                    }
                }
                Msg::Reindexed(rel, r) => {
                    let s = match r {
                        Ok(Some(n)) => format!("{rel}: indexed ({n} chunks)"),
                        Ok(None) => format!("{rel}: index unchanged"),
                        Err(e) => format!("{rel}: reindex failed: {e}"),
                    };
                    self.set_status(s);
                }
                Msg::Documents(r) => {
                    if let Some(Overlay::Tags(b)) = self.overlay.as_mut() {
                        b.loading = false;
                        match r {
                            Ok(docs) => b.add_documents(&docs),
                            Err(e) => self.set_status(format!("documents: {e}")),
                        }
                    }
                }
                Msg::Tasks(list) => {
                    if let Some(Overlay::Tags(b)) = self.overlay.as_mut() {
                        b.add_tasks(&list);
                        if self.client.is_none() {
                            b.loading = false;
                        }
                    }
                    if let Some(t) = self.tasks.as_mut() {
                        t.set_tasks(list);
                    }
                }
                Msg::Health(ok) => self.lattice_ok = Some(ok),
                Msg::Fs(path) => {
                    let root = self.root();
                    if let Ok(rel) = path.strip_prefix(&root) {
                        let rel = rel.to_string_lossy().to_string();
                        let parent = Path::new(&rel)
                            .parent()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        if self.tree.children.contains_key(&parent) {
                            self.tree.reload(&root, &parent);
                        }
                        if self.tabs.iter().any(|t| t.rel == rel && !t.dirty) {
                            self.reload_tab(&rel);
                        }
                    }
                }
            }
        }
    }

    // ----------------------------------------------------------------- keys

    fn key(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        if k.kind != KeyEventKind::Press && k.kind != KeyEventKind::Repeat {
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
            match (ctrl, alt, k.code) {
                (true, _, KeyCode::Char('q')) => {
                    self.quit = true;
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

    fn editor_is_normal(&self) -> bool {
        self.tab().is_none_or(|t| t.vim.mode == vim::Mode::Normal)
    }

    fn cycle_focus(&mut self, back: bool) {
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

    fn leader_step(&mut self, keys: Vec<char>, term: &mut DefaultTerminal) {
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

    fn key_overlay(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
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
                        t.preview_scroll = *line as u16;
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

    fn key_sidebar(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
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
            KeyCode::Char('q') => self.quit = true,
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

    fn key_editor(&mut self, k: KeyEvent, term: &mut DefaultTerminal) {
        let Some(t) = self.tabs.get_mut(self.active) else {
            self.focus = Focus::Sidebar;
            return;
        };
        if t.readonly && t.vim.mode == vim::Mode::Normal {
            // Read-only buffers: navigation only.
            if let KeyCode::Char('i' | 'a' | 'o' | 'O' | 'I' | 'A' | 'x' | 'd' | 'c' | 'p' | 'r' | 'R') =
                k.code
            {
                self.set_status("read-only buffer");
                return;
            }
        }
        let before = t.text.lines().len() + t.text.lines().iter().map(String::len).sum::<usize>();
        let action = t.vim.input(key_input(k), &mut t.text);
        let after = t.text.lines().len() + t.text.lines().iter().map(String::len).sum::<usize>();
        if before != after
            || matches!(t.vim.mode, vim::Mode::Insert | vim::Mode::Replace(_))
                && !matches!(k.code, KeyCode::Esc)
        {
            t.dirty = t.dirty || before != after || matches!(k.code, KeyCode::Char(_));
        }
        t.refresh_preview();
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

    fn key_preview(&mut self, k: KeyEvent, _term: &mut DefaultTerminal) {
        let page = self.regions.preview.height.saturating_sub(2).max(1);
        let Some(t) = self.tabs.get_mut(self.active) else {
            self.focus = Focus::Sidebar;
            return;
        };
        let max = t.preview.len() as u16;
        match k.code {
            KeyCode::Char(' ') => self.overlay = Some(Overlay::Leader(vec![])),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help(0)),
            KeyCode::Esc => self.focus = if self.show_sidebar { Focus::Sidebar } else { Focus::Editor },
            KeyCode::Char('j') | KeyCode::Down => {
                t.preview_scroll = (t.preview_scroll + 1).min(max.saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => t.preview_scroll = t.preview_scroll.saturating_sub(1),
            KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                t.preview_scroll = (t.preview_scroll + page / 2).min(max.saturating_sub(1));
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                t.preview_scroll = t.preview_scroll.saturating_sub(page / 2);
            }
            KeyCode::PageDown => t.preview_scroll = (t.preview_scroll + page).min(max.saturating_sub(1)),
            KeyCode::PageUp => t.preview_scroll = t.preview_scroll.saturating_sub(page),
            KeyCode::Char('G') => t.preview_scroll = max.saturating_sub(1),
            KeyCode::Char('g') => t.preview_scroll = 0,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn key_tasks(&mut self, k: KeyEvent, _term: &mut DefaultTerminal) {
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

    // ---------------------------------------------------------------- mouse

    fn mouse(&mut self, m: MouseEvent) {
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
                        let widths: Vec<usize> =
                            self.tabs.iter().map(|t| tab_label(t).chars().count()).collect();
                        if let Some(i) = mouse::tab_at(&widths, x) {
                            self.active = i;
                            self.focus = Focus::Editor;
                        }
                    }
                    Target::Editor => {
                        self.focus = if self.tasks.is_some() { Focus::Tasks } else { Focus::Editor };
                    }
                    Target::Preview => self.focus = Focus::Preview,
                    Target::Bottom { row } => {
                        if self.show_neighbors
                            && let Some(n) = self.neighbors.get(row)
                        {
                            let p = n.path.clone();
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
                        let max = t.preview.len();
                        t.preview_scroll = mouse::scroll(t.preview_scroll as usize, max, down, 3) as u16;
                    }
                }
                Target::Editor => {
                    if let Some(tv) = self.tasks.as_mut() {
                        if down { tv.down() } else { tv.up() }
                    } else if let Some(t) = self.tab_mut() {
                        t.text.scroll((if down { 3 } else { -3 }, 0));
                    }
                }
                Target::Tabs { .. } | Target::Bottom { .. } => {}
            },
        }
    }

    // ----------------------------------------------------------------- draw

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        f.render_widget(Block::default().style(theme::base()), area);
        let [main, status] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        let (side, right) = if self.show_sidebar {
            let [s, r] = Layout::horizontal([Constraint::Percentage(self.sidebar_pct), Constraint::Fill(1)])
                .areas(main);
            (Some(s), r)
        } else {
            (None, main)
        };
        self.regions = Regions::default();
        if let Some(s) = side {
            self.regions.sidebar = s;
            self.draw_sidebar(f, s);
        }
        let [tabs, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(right);
        self.regions.tabs = tabs;
        self.draw_tabs(f, tabs);

        let (body, hal_area) = if self.show_hal {
            let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(32)]).areas(body);
            (a, Some(b))
        } else {
            (body, None)
        };
        let (body, nb_area) = if self.show_neighbors {
            let [a, b] = Layout::vertical([Constraint::Fill(1), Constraint::Length(9)]).areas(body);
            (a, Some(b))
        } else {
            (body, None)
        };

        if let Some(tv) = &self.tasks {
            self.regions.editor = body;
            tv.draw(f, body, self.focus == Focus::Tasks);
        } else {
            match self.split {
                Split::Both => {
                    let [e, p] =
                        Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(self.preview_pct)])
                            .areas(body);
                    self.regions.editor = e;
                    self.regions.preview = p;
                    self.draw_editor(f, e);
                    self.draw_preview(f, p);
                }
                Split::EditorOnly => {
                    self.regions.editor = body;
                    self.draw_editor(f, body);
                }
                Split::PreviewOnly => {
                    self.regions.preview = body;
                    self.draw_preview(f, body);
                }
            }
        }
        if let Some(a) = hal_area {
            self.draw_hal(f, a);
        }
        if let Some(a) = nb_area {
            self.regions.bottom = a;
            self.draw_neighbors(f, a);
        }
        self.draw_status(f, status);
        self.draw_overlay(f, area);
    }

    fn draw_sidebar(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.tree.visible();
        let inner_h = area.height.saturating_sub(2) as usize;
        if self.sel < self.sidebar_scroll {
            self.sidebar_scroll = self.sel;
        } else if inner_h > 0 && self.sel >= self.sidebar_scroll + inner_h {
            self.sidebar_scroll = self.sel + 1 - inner_h;
        }
        let open: HashSet<&str> = self.tabs.iter().map(|t| t.rel.as_str()).collect();
        let items: Vec<ListItem> = rows
            .iter()
            .map(|(depth, e)| {
                let pad = "  ".repeat(*depth);
                let glyph = if e.is_dir {
                    if self.tree.expanded.contains(&e.rel) { "▾ " } else { "▸ " }
                } else if open.contains(e.rel.as_str()) {
                    "● "
                } else {
                    "  "
                };
                let style = if e.is_dir {
                    Style::default().fg(theme::REGENT)
                } else if open.contains(e.rel.as_str()) {
                    Style::default().fg(theme::GOLD)
                } else {
                    Style::default().fg(theme::CREAM)
                };
                ListItem::new(Line::from(Span::styled(format!("{pad}{glyph}{}", e.name), style)))
            })
            .collect();
        let focused = self.focus == Focus::Sidebar;
        let name =
            self.ctx.vault.root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {name} "))
            .border_style(if focused { theme::focused() } else { theme::chrome() });
        let mut state = ListState::default()
            .with_selected(Some(self.sel.min(rows.len().saturating_sub(1))))
            .with_offset(self.sidebar_scroll);
        f.render_stateful_widget(
            List::new(items).block(block).highlight_style(theme::selected()),
            area,
            &mut state,
        );
        self.sidebar_scroll = state.offset();
    }

    fn draw_tabs(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        for (i, t) in self.tabs.iter().enumerate() {
            let label = tab_label(t);
            let style = if i == self.active { theme::selected() } else { theme::dim() };
            spans.push(Span::styled(format!(" {label} "), style));
            spans.push(Span::raw(" "));
        }
        if self.tabs.is_empty() {
            spans.push(Span::styled(" no notes open — Enter on a note, Ctrl+P to search ", theme::dim()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn draw_editor(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Editor;
        let border = if focused { theme::focused() } else { theme::chrome() };
        match self.tabs.get_mut(self.active) {
            Some(t) => {
                let title = format!(" {}{} ", t.rel, if t.dirty { " *" } else { "" });
                t.text.set_block(Block::default().borders(Borders::ALL).title(title).border_style(border));
                t.text.set_cursor_style(if focused { t.vim.cursor_style() } else { Style::default() });
                t.text.set_style(theme::base());
                f.render_widget(&t.text, area);
            }
            None => {
                let lines = vec![
                    Line::from(Span::styled("Lapis", theme::accent())),
                    Line::default(),
                    Line::from("Enter on a note to open it · Ctrl+P to search the lattice"),
                    Line::from("Space for the leader menu · ? for help"),
                ];
                f.render_widget(
                    Paragraph::new(Text::from(lines))
                        .block(Block::default().borders(Borders::ALL).border_style(border))
                        .wrap(Wrap { trim: true }),
                    area,
                );
            }
        }
    }

    fn draw_preview(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Preview;
        let border = if focused { theme::focused() } else { theme::chrome() };
        let wrap = self.wrap;
        let Some(t) = self.tabs.get_mut(self.active) else {
            f.render_widget(
                Block::default().borders(Borders::ALL).title(" preview ").border_style(border),
                area,
            );
            return;
        };
        t.refresh_preview();
        let max = t.preview.len() as u16;
        t.preview_scroll = t.preview_scroll.min(max.saturating_sub(1));
        let mut p = Paragraph::new(Text::from(t.preview.clone()))
            .block(Block::default().borders(Borders::ALL).title(" preview ").border_style(border))
            .scroll((t.preview_scroll, 0));
        if wrap {
            p = p.wrap(Wrap { trim: false });
        }
        f.render_widget(p, area);
    }

    fn draw_hal(&self, f: &mut Frame, area: Rect) {
        let lines: Vec<Line> = match self.tab() {
            Some(t) => hal_view::lines(&t.hal, t.hal_valid),
            None => vec![],
        };
        f.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().borders(Borders::ALL).title(" HAL ").border_style(theme::chrome()))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_neighbors(&self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = if self.neighbors.is_empty() {
            vec![ListItem::new(Span::styled("(no hop-1 neighbors, or still loading)", theme::dim()))]
        } else {
            neighbors_view::rows(&self.neighbors)
                .iter()
                .map(|r| ListItem::new(neighbors_view::line(r)))
                .collect()
        };
        let title = format!(" hop-1 neighbors · {} ", self.neighbors_for);
        f.render_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(title).border_style(theme::chrome())),
            area,
        );
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        let (mode, prompt) = match (self.focus, self.tab()) {
            (Focus::Editor, Some(t)) => (t.vim.mode_label(), t.vim.prompt.clone()),
            (Focus::Sidebar, _) => ("FILES".into(), None),
            (Focus::Preview, _) => ("PREVIEW".into(), None),
            (Focus::Tasks, _) => ("TASKS".into(), None),
            _ => ("LAPIS".into(), None),
        };
        spans.push(Span::styled(format!(" {mode} "), theme::mode(&mode)));
        if let Some(p) = prompt {
            spans.push(Span::styled(format!(" {}{}▏", p.kind, p.text), Style::default().fg(theme::GOLD)));
        } else {
            if let Some(t) = self.tab() {
                spans.push(Span::styled(format!(" {}", t.rel), Style::default().fg(theme::CREAM)));
                if t.dirty {
                    spans.push(Span::styled(" ●", Style::default().fg(theme::GOLD)));
                }
                if t.readonly {
                    spans.push(Span::styled(" [ro]", theme::dim()));
                }
                let c = t.text.cursor();
                spans.push(Span::styled(format!("  {}:{}", c.0 + 1, c.1 + 1), theme::dim()));
            }
            if self.status_at.elapsed() < Duration::from_secs(8) && !self.status.is_empty() {
                spans.push(Span::styled(format!("  {}", self.status), Style::default().fg(theme::REGENT)));
            }
        }
        let lattice = match self.lattice_ok {
            Some(true) => Span::styled(" lattice ✓ ", Style::default().fg(theme::OK)),
            Some(false) => Span::styled(" lattice ✗ ", Style::default().fg(theme::WARN)),
            None => Span::styled(" lattice … ", theme::dim()),
        };
        let tabs = if self.tabs.is_empty() {
            String::new()
        } else {
            format!(" {}/{} ", self.active + 1, self.tabs.len())
        };
        let right = Line::from(vec![Span::styled(tabs, theme::dim()), lattice]);
        let right_w = right.width() as u16;
        let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(spans)).style(theme::statusline()), l);
        f.render_widget(Paragraph::new(right).style(theme::statusline()), r);
    }

    fn draw_overlay(&self, f: &mut Frame, area: Rect) {
        let Some(ov) = &self.overlay else { return };
        match ov {
            Overlay::Leader(keys) => {
                let table = leader::table();
                let nodes = leader::resolve(&table, keys).unwrap_or_default();
                let mut lines: Vec<Line> = Vec::new();
                let title = if keys.is_empty() {
                    " Space ".to_string()
                } else {
                    format!(" Space {} ", keys.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(" "))
                };
                let cols = 3usize;
                let per_col = nodes.len().div_ceil(cols).max(1);
                for row in 0..per_col {
                    let mut spans = Vec::new();
                    for c in 0..cols {
                        if let Some(n) = nodes.get(c * per_col + row) {
                            spans.push(Span::styled(format!(" {} ", n.key()), theme::accent()));
                            spans.push(Span::styled(
                                format!("{:<26}", n.label()),
                                Style::default().fg(theme::CREAM),
                            ));
                        }
                    }
                    lines.push(Line::from(spans));
                }
                let h = (lines.len() as u16 + 2).min(area.height);
                let w = area.width.min(96);
                let popup = Rect {
                    x: (area.width - w) / 2,
                    y: area.height.saturating_sub(h + 1),
                    width: w,
                    height: h,
                };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new(Text::from(lines)).style(theme::overlay()).block(
                        Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()),
                    ),
                    popup,
                );
            }
            Overlay::Pane => {
                let popup = Rect {
                    x: area.width.saturating_sub(48),
                    y: area.height.saturating_sub(4),
                    width: 46.min(area.width),
                    height: 3,
                };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new("h/l focus · < > sidebar · - + preview · v split · o one · q close")
                        .style(theme::overlay())
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Ctrl+W ")
                                .border_style(theme::focused()),
                        ),
                    popup,
                );
            }
            Overlay::Palette(p) => {
                let w = area.width * 3 / 4;
                let h = (area.height * 2 / 3).max(8);
                let popup = Rect { x: (area.width - w) / 2, y: (area.height - h) / 2, width: w, height: h };
                f.render_widget(Clear, popup);
                let [input, list] =
                    Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(popup);
                let title = if p.is_commands() {
                    " commands "
                } else if p.pending {
                    " lattice search… "
                } else {
                    " lattice search  (type `>` for commands) "
                };
                f.render_widget(
                    Paragraph::new(format!("> {}▏", p.input)).style(theme::overlay()).block(
                        Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()),
                    ),
                    input,
                );
                let items: Vec<ListItem> = p
                    .items
                    .iter()
                    .map(|it| match it {
                        Item::Note { path, title, snippet } => ListItem::new(vec![
                            Line::from(vec![
                                Span::styled(
                                    title.clone(),
                                    Style::default().fg(theme::CREAM).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(format!("  {path}"), theme::dim()),
                            ]),
                            Line::from(Span::styled(
                                snippet
                                    .clone()
                                    .unwrap_or_default()
                                    .chars()
                                    .take(list.width as usize - 4)
                                    .collect::<String>(),
                                theme::chrome(),
                            )),
                        ]),
                        Item::Command { keys, cmd } => ListItem::new(Line::from(vec![
                            Span::styled(format!("{keys:<14}"), theme::accent()),
                            Span::raw(cmd.label()),
                        ])),
                    })
                    .collect();
                let mut st = ListState::default().with_selected(Some(p.sel));
                f.render_stateful_widget(
                    List::new(items)
                        .style(theme::overlay())
                        .block(Block::default().borders(Borders::ALL).border_style(theme::chrome()))
                        .highlight_style(theme::selected()),
                    list,
                    &mut st,
                );
            }
            Overlay::Help(scroll) => {
                let popup = centered(area, 90, 90);
                f.render_widget(Clear, popup);
                let mut lines: Vec<Line> = Vec::new();
                for s in help::sections() {
                    lines.push(Line::from(Span::styled(s.title, theme::accent())));
                    for (k, v) in s.rows {
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {k:<34}"), Style::default().fg(theme::GOLD)),
                            Span::raw(v),
                        ]));
                    }
                    lines.push(Line::default());
                }
                let max = lines.len().saturating_sub(popup.height.saturating_sub(2) as usize) as u16;
                let scroll = (*scroll as u16).min(max);
                f.render_widget(
                    Paragraph::new(Text::from(lines)).style(theme::overlay()).scroll((scroll, 0)).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" help  (j/k scroll · Esc close) ")
                            .border_style(theme::focused()),
                    ),
                    popup,
                );
            }
            Overlay::Outline(sel, items) => {
                self.draw_picker(
                    f,
                    area,
                    " outline ",
                    *sel,
                    items
                        .iter()
                        .map(|(_, lvl, t)| format!("{}{t}", "  ".repeat((*lvl as usize).saturating_sub(1))))
                        .collect(),
                );
            }
            Overlay::Buffers(sel) => {
                self.draw_picker(
                    f,
                    area,
                    " buffers  (Enter switch · x close) ",
                    *sel,
                    self.tabs.iter().map(tab_label).collect(),
                );
            }
            Overlay::Restore(sel, items) => {
                self.draw_picker(f, area, " trash  (Enter restores) ", *sel, items.clone());
            }
            Overlay::Tags(b) => {
                let mut rows = b.rows();
                if rows.is_empty() && !b.loading {
                    rows.push("(no tags)".into());
                }
                self.draw_picker(f, area, &b.title(), b.selected_index(), rows);
            }
            Overlay::Templates(sel, items) => {
                self.draw_picker(
                    f,
                    area,
                    " new from template ",
                    *sel,
                    items.iter().map(|t| format!("{:<24} {}   {}", t.id, t.name, t.target)).collect(),
                );
            }
            Overlay::Prompt(title, input, _) => {
                let w = area.width.min(70);
                let popup = Rect { x: (area.width - w) / 2, y: area.height / 3, width: w, height: 3 };
                f.render_widget(Clear, popup);
                f.render_widget(
                    Paragraph::new(format!("{input}▏")).style(theme::overlay()).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(format!(" {title} "))
                            .border_style(theme::focused()),
                    ),
                    popup,
                );
            }
        }
    }

    fn draw_picker(&self, f: &mut Frame, area: Rect, title: &str, sel: usize, rows: Vec<String>) {
        let popup = centered(area, 70, 60);
        f.render_widget(Clear, popup);
        let items: Vec<ListItem> = rows.into_iter().map(ListItem::new).collect();
        let mut st = ListState::default().with_selected(Some(sel));
        f.render_stateful_widget(
            List::new(items)
                .style(theme::overlay())
                .block(Block::default().borders(Borders::ALL).title(title).border_style(theme::focused()))
                .highlight_style(theme::selected()),
            popup,
            &mut st,
        );
    }
}

fn tab_label(t: &Tab) -> String {
    let name = Path::new(&t.rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| t.rel.clone());
    format!("{name}{}", if t.dirty { " *" } else { "" })
}

fn centered(area: Rect, pct_w: u16, pct_h: u16) -> Rect {
    let w = area.width * pct_w / 100;
    let h = area.height * pct_h / 100;
    Rect { x: (area.width - w) / 2, y: (area.height - h) / 2, width: w, height: h }
}

// ------------------------------------------------------------------ runner

pub async fn run(ctx: Ctx) -> Result<()> {
    tokio::task::block_in_place(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
            default_hook(info);
        }));
        let mut term = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
        let mut app = App::new(ctx);
        let result = ui_loop(&mut app, &mut term);
        let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        result
    })
}

fn ui_loop(app: &mut App, term: &mut DefaultTerminal) -> Result<()> {
    let mut last_health = Instant::now();
    while !app.quit {
        app.drain();
        if last_health.elapsed() > Duration::from_secs(30) {
            app.poll_health();
            last_health = Instant::now();
        }
        term.draw(|f| app.draw(f)).map_err(|e| LapisError::Internal(format!("draw: {e}")))?;
        if event::poll(Duration::from_millis(60)).map_err(|e| LapisError::Internal(e.to_string()))? {
            match event::read().map_err(|e| LapisError::Internal(e.to_string()))? {
                Event::Key(k) => app.key(k, term),
                Event::Mouse(m) => app.mouse(m),
                _ => {}
            }
        }
    }
    Ok(())
}
