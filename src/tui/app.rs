//! App state, watcher, and backend IO. Keys live in `keys`; drawing in `draw`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use notify::{RecursiveMode, Watcher};
use ratatui::DefaultTerminal;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui_textarea::TextArea;

use crate::http::{Document, ListParams, Neighbor};
use crate::ops::{self, Ctx, SearchQuery};
use crate::tasks::Task;
use crate::{hal, notes, tasks, templates, write};
use lapis_lattice::{Hit, Mode as SearchMode};

use super::mouse::Regions;
use super::omarchy;
use super::palette::Palette;
use super::preview;
use super::tags_view::TagsBrowser;
use super::tasks_view::{TasksView, View};
use super::theme;
use super::tree::{self, Tree};
use super::vim::{self, Vim};

pub(crate) enum Msg {
    Search(u64, std::result::Result<Vec<Hit>, String>),
    Neighbors(String, std::result::Result<Vec<Neighbor>, String>),
    Documents(std::result::Result<Vec<Document>, String>),
    Reindexed(String, std::result::Result<Option<u64>, String>),
    Tasks(Vec<Task>),
    Health(bool),
    Fs(PathBuf),
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum Focus {
    Sidebar,
    Editor,
    Preview,
    Tasks,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(crate) enum Split {
    Both,
    EditorOnly,
    PreviewOnly,
}

/// Modal overlays; at most one at a time.
pub(crate) enum Overlay {
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
pub(crate) enum PromptKind {
    NewNote,
    NewFromTemplate(usize),
    QuickCapture,
}

pub(crate) struct Tab {
    pub(crate) rel: String,
    pub(crate) text: TextArea<'static>,
    pub(crate) vim: Vim,
    pub(crate) hal: serde_json::Map<String, serde_json::Value>,
    pub(crate) hal_valid: bool,
    pub(crate) dirty: bool,
    pub(crate) preview_scroll: u16,
    pub(crate) preview: Vec<Line<'static>>,
    pub(crate) preview_for: String,
    pub(crate) readonly: bool,
    /// Exact disk contents last read/saved, for detecting external edits.
    pub(crate) saved_source: Option<String>,
}

impl Tab {
    pub(crate) fn body(&self) -> String {
        self.text.lines().join("\n")
    }
    pub(crate) fn refresh_preview(&mut self) {
        let body = self.body();
        if body != self.preview_for {
            self.preview = preview::render(&body);
            self.preview_for = body;
        }
    }
}

pub(crate) struct App {
    pub(crate) ctx: Ctx,
    pub(crate) tree: Tree,
    pub(crate) sel: usize,
    pub(crate) sidebar_scroll: usize,
    pub(crate) show_sidebar: bool,
    pub(crate) sidebar_pct: u16,
    pub(crate) preview_pct: u16,
    pub(crate) split: Split,
    pub(crate) wrap: bool,
    pub(crate) line_numbers: bool,
    pub(crate) focus: Focus,
    pub(crate) tabs: Vec<Tab>,
    pub(crate) active: usize,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) tasks: Option<TasksView>,
    pub(crate) show_hal: bool,
    pub(crate) show_neighbors: bool,
    pub(crate) neighbors: Vec<Neighbor>,
    pub(crate) neighbors_for: String,
    pub(crate) neighbors_sel: usize,
    pub(crate) status: String,
    pub(crate) status_at: Instant,
    pub(crate) lattice_ok: Option<bool>,
    pub(crate) tx: Sender<Msg>,
    pub(crate) rx: Receiver<Msg>,
    pub(crate) watcher: Option<notify::RecommendedWatcher>,
    pub(crate) watched: HashSet<PathBuf>,
    pub(crate) trash_bucket: String,
    pub(crate) regions: Regions,
    pub(crate) pending_g: bool,
    pub(crate) pending_templates: Option<Vec<templates::Template>>,
    /// Name of the live Omarchy theme, when this is an Omarchy box.
    pub(crate) omarchy_theme: Option<String>,
    pub(crate) quit: bool,
}

impl App {
    pub(crate) fn new(ctx: Ctx) -> Self {
        let (tx, rx) = channel();
        let trash_bucket = crate::ops::trash_bucket(&ctx);
        let mut tree = Tree::default();
        tree.load(&ctx.vault.root, "");
        let mut app = App {
            ctx,
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
            omarchy_theme: None,
            quit: false,
        };
        app.start_watcher();
        app.poll_health();
        app
    }

    pub(crate) fn root(&self) -> PathBuf {
        self.ctx.vault.root.clone()
    }

    pub(crate) fn set_status(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.status_at = Instant::now();
    }

    pub(crate) fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }
    pub(crate) fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub(crate) fn request_quit(&mut self) {
        if let Some(t) = self.tabs.iter().find(|t| t.dirty) {
            self.set_status(format!("unsaved changes in {}: :w to save or :q! to discard that tab", t.rel));
        } else {
            self.quit = true;
        }
    }

    // ------------------------------------------------------------ background

    pub(crate) fn poll_health(&self) {
        let Ok(backend) = self.ctx.backend() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let ok = backend.health().await.is_ok();
            let _ = tx.send(Msg::Health(ok));
        });
    }

    pub(crate) fn start_watcher(&mut self) {
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
        // Watch the parent `current/`, not the theme directory or a link target:
        // the swap mechanism differs per Omarchy flavour (symlink vs real
        // directory), and watching the parent covers all of them.
        if self.ctx.cfg.theme.is_omarchy()
            && let Some(state) = omarchy::state_root()
            && state.is_dir()
        {
            self.watch_dir(&state);
        }
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

    pub(crate) fn watch_dir(&mut self, dir: &Path) {
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

    pub(crate) fn kick(&self, rel: String) {
        let Ok(backend) = self.ctx.backend() else { return };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r = backend.reindex(&rel).await.map(|r| r.chunks).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Reindexed(rel, r));
        });
    }

    pub(crate) fn fetch_neighbors(&mut self) {
        let Some(rel) = self.tab().map(|t| t.rel.clone()) else { return };
        let backend = match self.ctx.backend() {
            Ok(b) => b,
            Err(e) => {
                self.set_status(format!("lattice: {e}"));
                return;
            }
        };
        self.neighbors_for = rel.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let r =
                backend.neighbors(&rel, "both", true).await.map(|n| n.neighbors).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Neighbors(rel, r));
        });
    }

    pub(crate) fn scan_tasks(&mut self) {
        let root = self.root();
        let tx = self.tx.clone();
        let filter = tasks::Filter {
            prefix: self.tasks.as_ref().and_then(|t| t.scope.clone()),
            exclude: self.ctx.cfg.agent.task_exclude.clone(),
            ..Default::default()
        };
        if let Some(t) = self.tasks.as_mut() {
            t.loading = true;
        }
        tokio::task::spawn_blocking(move || {
            let list = tasks::list(&root, &filter).unwrap_or_default();
            let _ = tx.send(Msg::Tasks(list));
        });
    }

    /// `Space #`: tags from the lattice document table plus inline task tags.
    pub(crate) fn open_tags(&mut self) {
        self.overlay = Some(Overlay::Tags(TagsBrowser::new()));
        let tx = self.tx.clone();
        match self.ctx.backend() {
            Ok(backend) => {
                tokio::spawn(async move {
                    let p = ListParams { limit: 1000, ..ListParams::default() };
                    let r = backend.documents(&p).await.map_err(|e| e.to_string());
                    let _ = tx.send(Msg::Documents(r));
                });
            }
            Err(e) => {
                self.set_status(format!("lattice: {e}; tags from tasks only"));
                if let Some(Overlay::Tags(b)) = self.overlay.as_mut() {
                    b.loading = false;
                }
            }
        }
        let root = self.root();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let list = tasks::list(&root, &tasks::Filter::default()).unwrap_or_default();
            let _ = tx.send(Msg::Tasks(list));
        });
    }

    pub(crate) fn lattice_search(&mut self, mode: SearchMode) {
        let Some(Overlay::Palette(p)) = self.overlay.as_mut() else { return };
        let q = p.query();
        if q.is_empty() || p.is_commands() {
            return;
        }
        if q == p.asked {
            return;
        }
        p.seq += 1;
        p.pending = true;
        p.asked = q.clone();
        let seq = p.seq;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            let r = ops::search(
                &ctx,
                SearchQuery { query: q, limit: 25, mode, per_doc: true, ..Default::default() },
            )
            .await
            .map(|p| p.result.hits)
            .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Search(seq, r));
        });
    }

    // ------------------------------------------------------------------ tabs

    pub(crate) fn open_note(&mut self, rel: &str) {
        if let Some(i) = self.tabs.iter().position(|t| t.rel == rel) {
            self.active = i;
            self.after_open();
            return;
        }
        match notes::read(&self.root(), rel) {
            Ok(n) => {
                let saved_source = if n.kind == notes::Kind::Markdown {
                    std::fs::read_to_string(self.root().join(&n.path)).ok()
                } else {
                    None
                };
                let body = saved_source.as_deref().map(|s| hal::raw_parts(s).1).unwrap_or(&n.body);
                let mut ta = TextArea::from(
                    body.replace("\r\n", "\n").split('\n').map(str::to_string).collect::<Vec<_>>(),
                );
                ta.set_cursor_line_style(Style::default());
                ta.set_line_number_style(theme::dim());
                ta.set_selection_style(theme::selected());
                ta.set_search_style(Style::default().bg(theme::gold()).fg(theme::BLUE_DEEP));
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
                    saved_source,
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

    pub(crate) fn after_open(&mut self) {
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

    pub(crate) fn reload_tab(&mut self, rel: &str) {
        let Some(i) = self.tabs.iter().position(|t| t.rel == rel) else { return };
        if let Ok(n) = notes::read(&self.root(), rel) {
            let saved_source = if n.kind == notes::Kind::Markdown {
                std::fs::read_to_string(self.root().join(rel)).ok()
            } else {
                None
            };
            let body = saved_source.as_deref().map(|s| hal::raw_parts(s).1).unwrap_or(&n.body);
            let t = &mut self.tabs[i];
            let cursor = t.text.cursor();
            let mut ta = TextArea::from(
                body.replace("\r\n", "\n").split('\n').map(str::to_string).collect::<Vec<_>>(),
            );
            ta.set_cursor_line_style(Style::default());
            ta.set_line_number_style(theme::dim());
            ta.set_selection_style(theme::selected());
            ta.move_cursor(ratatui_textarea::CursorMove::Jump(cursor.0 as u16, cursor.1 as u16));
            t.text = ta;
            t.hal = n.hal;
            t.hal_valid = n.hal_valid;
            t.dirty = false;
            t.saved_source = saved_source;
            t.refresh_preview();
        }
    }

    pub(crate) fn close_tab(&mut self, force: bool) {
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
    pub(crate) fn save(&mut self) {
        let Some(t) = self.tab() else { return };
        if t.readonly {
            self.set_status("read-only (PDF/HTML text)");
            return;
        }
        let rel = t.rel.clone();
        let abs = self.root().join(&rel);
        let current = match std::fs::read_to_string(&abs) {
            Ok(current) => current,
            Err(e) => {
                self.set_status(format!("save failed reading {rel}: {e}; buffer retained"));
                return;
            }
        };
        if t.saved_source.as_ref().is_some_and(|saved| saved != &current) {
            self.set_status(format!("{rel} changed on disk; save cancelled, buffer retained for comparison"));
            return;
        }
        let (head, _) = hal::raw_parts(&current);
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
                    t.saved_source = Some(next.clone());
                }
                self.set_status(format!("saved {rel}"));
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("save failed: {e}")),
        }
    }

    pub(crate) fn toggle_checkbox_at_cursor(&mut self) {
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

    /// `Space z t`. On Omarchy this hops the OS theme so every app moves
    /// together; the watcher then restyles us. Off Omarchy — or if the Omarchy
    /// tools are missing or unhappy — it fails open onto the brand palettes
    /// rather than leaving the user half-styled.
    pub(crate) fn next_theme(&mut self) {
        if self.ctx.cfg.theme.is_omarchy() {
            let current = self.omarchy_theme.clone().unwrap_or_default();
            if let Some(next) = omarchy::next_theme(&current) {
                match omarchy::theme_set(&next) {
                    Ok(()) => {
                        self.set_status(format!("omarchy theme: {next}"));
                        return;
                    }
                    Err(e) => {
                        // fail open: say why, keep the current palette, fall
                        // through to the private palettes below
                        self.set_status(format!("omarchy theme unchanged: {e}"));
                        return;
                    }
                }
            }
        }
        let next = theme::current().next();
        theme::install(next);
        self.set_status(format!("theme: {}", next.name));
    }

    pub(crate) fn open_tasks(&mut self, view: View) {
        match self.tasks.as_mut() {
            Some(t) => t.view = view,
            None => {
                let mut tv = TasksView::new(view);
                tv.full = self.ctx.cfg.agent.task_unscoped_full();
                self.tasks = Some(tv);
                self.scan_tasks();
            }
        }
        self.focus = Focus::Tasks;
    }

    pub(crate) fn periodic(&mut self, period: write::Period) {
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

    pub(crate) fn selected_entry(&self) -> Option<tree::Entry> {
        self.tree.visible().get(self.sel).map(|(_, e)| e.clone())
    }

    /// Folder for a new note: the selected sidebar folder (or its parent), else inbox.
    pub(crate) fn target_folder(&self) -> String {
        match self.selected_entry() {
            Some(e) if e.is_dir => format!("{}/", e.rel),
            Some(e) => {
                Path::new(&e.rel).parent().map(|p| format!("{}/", p.to_string_lossy())).unwrap_or_default()
            }
            None => String::new(),
        }
    }

    pub(crate) fn create_note(&mut self, title: &str, template: Option<String>) {
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
            dry_run: false,
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

    pub(crate) fn capture(&mut self, text: &str) {
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

    pub(crate) fn trash_current(&mut self) {
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

    pub(crate) fn restore(&mut self, trashed_rel: &str) {
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

    pub(crate) fn external_editor(&mut self, term: &mut DefaultTerminal) {
        let Some(t) = self.tab() else {
            self.set_status("open a note first");
            return;
        };
        if t.dirty {
            self.set_status("save with :w before opening the external editor");
            return;
        }
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
        let abs = self.root().join(&t.rel);
        let rel = t.rel.clone();
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste, DisableMouseCapture);
        ratatui::restore();
        let status = std::process::Command::new(&editor).arg(&abs).status();
        *term = ratatui::init();
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
        match status {
            Ok(_) => {
                self.reload_tab(&rel);
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("{editor}: {e}")),
        }
    }

    pub(crate) fn toggle_task(&mut self, id: &str) {
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

    pub(crate) fn drain(&mut self) {
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
                    }
                    if let Some(t) = self.tasks.as_mut() {
                        t.set_tasks(list);
                    }
                }
                Msg::Health(ok) => self.lattice_ok = Some(ok),
                Msg::Fs(path) => {
                    // A theme swap restyles in place; no restart, no reindex.
                    if let Some(state) = omarchy::state_root()
                        && path.starts_with(&state)
                    {
                        if let Some(l) = omarchy::load(&state) {
                            theme::install(l.palette);
                            let note = if l.fallbacks.is_empty() {
                                String::new()
                            } else {
                                format!(" ({} role(s) fell back)", l.fallbacks.len())
                            };
                            self.set_status(format!("theme: {}{note}", l.name));
                            self.omarchy_theme = Some(l.name);
                        }
                        continue;
                    }
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
}

/// `[theme]` → palette: named (default lapis) plus `custom` overrides.
pub(crate) fn palette_from_config(t: &crate::config::ThemeConfig) -> theme::Palette {
    let base = t.name.as_deref().and_then(theme::named).unwrap_or(theme::LAPIS);
    base.with_overrides(t.custom.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

/// Startup palette. On Omarchy the active theme wins; everywhere else this
/// falls back to the brand palettes without the caller needing to know which
/// kind of machine it is on. Returns the Omarchy theme name when one is live.
pub(crate) fn resolve_palette(t: &crate::config::ThemeConfig) -> (theme::Palette, Option<String>) {
    if t.is_omarchy()
        && let Some(root) = omarchy::state_root()
        && let Some(l) = omarchy::load(&root)
    {
        return (l.palette, Some(l.name));
    }
    (palette_from_config(t), None)
}

pub(crate) fn tab_label(t: &Tab) -> String {
    let name = Path::new(&t.rel)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| t.rel.clone());
    format!("{name}{}", if t.dirty { " *" } else { "" })
}
