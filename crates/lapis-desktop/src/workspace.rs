//! Native notes shell. Files load independently from the optional graph/index.

mod command;
mod context;
mod document;
mod graph;
mod history;
mod palette;
mod pane_view;
mod panes;
mod render;
mod session;
#[cfg(all(test, feature = "gui-tests"))]
mod tests;

use crate::{
    DesktopError, Options,
    services::{Document, FileEntry, FileKind, WorkspaceServices},
};
use gpui_kit::base::input::{InputEvent, TextareaState};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, KeyDownEvent, Subscription, Window,
    WindowOptions,
};
use std::sync::Arc;

gpui_kit::actions!(lapis_workspace, [NavigateBack, NavigateForward]);

fn init_workspace(cx: &mut App) {
    let (back, forward) =
        if cfg!(target_os = "macos") { ("cmd-alt-left", "cmd-alt-right") } else { ("alt-left", "alt-right") };
    cx.bind_keys([
        gpui_kit::KeyBinding::new(back, NavigateBack, Some("LapisWorkspace")),
        gpui_kit::KeyBinding::new(forward, NavigateForward, Some("LapisWorkspace")),
    ]);
}

use crate::session::View;

struct Tab {
    document: Document,
    editor: Entity<TextareaState>,
    live: Entity<crate::live::LiveEditor>,
    pdf: Option<Entity<crate::pdf_reader::PdfReader>>,
    _events: Subscription,
    view: View,
    split: Entity<gpui_kit::base::ResizableState>,
    split_width: Option<f32>,
    reading_scroll: gpui_kit::ScrollHandle,
    vim: crate::vim::Vim,
    close_after_save: bool,
    saving: bool,
}

struct Workspace {
    services: Arc<dyn WorkspaceServices>,
    layout: Entity<gpui_kit::base::ResizableState>,
    context_layout: Entity<gpui_kit::base::ResizableState>,
    files: Vec<FileEntry>,
    folder: String,
    files_loading: bool,
    file_epoch: u64,
    open_epoch: u64,
    opening: Option<String>,
    tabs: Vec<Tab>,
    tab_order: Vec<String>,
    restored: Vec<crate::session::Tab>,
    session_ready: bool,
    session_notice: Option<String>,
    sidebar_width: f32,
    context_width: f32,
    active: usize,
    status: String,
    error: bool,
    focus: FocusHandle,
    context: bool,
    context_state: context::Panel,
    history: history::History,
    graph: Option<Entity<crate::window::Root>>,
    graph_events: Option<Subscription>,
    graph_visible: bool,
    panes: crate::panes::Panes,
    document_layout: Entity<gpui_kit::base::ResizableState>,
    pane_load_epoch: u64,
    pane_loading: bool,
    pane_errors: std::collections::BTreeMap<String, String>,
    palette: Option<palette::Palette>,
    query_epoch: u64,
    indexing: bool,
    register: crate::vim::Register,
    command: Option<command::Prompt>,
}

impl Workspace {
    fn new(services: Arc<dyn WorkspaceServices>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = Workspace {
            services,
            layout: cx.new(|_| gpui_kit::base::ResizableState::default()),
            context_layout: cx.new(|_| gpui_kit::base::ResizableState::default()),
            files: vec![],
            folder: String::new(),
            files_loading: false,
            file_epoch: 0,
            open_epoch: 0,
            opening: None,
            tabs: vec![],
            tab_order: vec![],
            restored: vec![],
            session_ready: false,
            session_notice: None,
            sidebar_width: 232.,
            context_width: 280.,
            active: 0,
            status: String::new(),
            error: false,
            focus: cx.focus_handle(),
            context: false,
            context_state: context::Panel::default(),
            history: history::History::default(),
            graph: None,
            graph_events: None,
            graph_visible: false,
            panes: crate::panes::Panes::default(),
            document_layout: cx.new(|_| gpui_kit::base::ResizableState::default()),
            pane_load_epoch: 0,
            pane_loading: false,
            pane_errors: Default::default(),
            palette: None,
            query_epoch: 0,
            indexing: false,
            register: crate::vim::Register::default(),
            command: None,
        };
        workspace.focus.focus(window, cx);
        workspace
    }
    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph_visible {
            if let Some(graph) = &self.graph {
                graph.update(cx, |g, cx| g.focus(window, cx));
            }
            return;
        }
        if self.tabs.is_empty()
            && self.opening.is_none()
            && let Some(path) = self.tab_order.first().cloned()
        {
            self.open_file(path, window, cx);
            return;
        }
        if let Some(tab) = self.tabs.get(self.active) {
            self.panes.assign(tab.document.path.clone());
            self.history.commit(tab.document.path.clone().into(), None);
        }
        self.refresh_context(false, window, cx);
        self.update_pane_visibility(window, cx);
        if let Some(prompt) = &self.command {
            prompt.input.update(cx, |s, cx| s.focus(window, cx));
            return;
        }
        if let Some(pdf) = self.tabs.get(self.active).and_then(|t| t.pdf.as_ref()) {
            pdf.update(cx, |s, cx| s.focus(window, cx));
        } else if let Some(tab) = self.tabs.get(self.active).filter(|t| t.view != View::Reading) {
            tab.editor.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }
    fn dirty(tab: &Tab, cx: &App) -> bool {
        tab.editor.read(cx).value().as_ref() != tab.document.text
    }

    fn directory(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.file_epoch += 1;
        let epoch = self.file_epoch;
        self.files_loading = true;
        let service = self.services.clone();
        let folder = path.clone();
        let task = cx.background_executor().spawn(async move { service.directory(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            if std::env::var_os("LAPIS_UI_TRACE").is_some() {
                eprintln!(
                    "lapis workspace: directory completed: {}",
                    result.as_ref().map_or_else(|e| e.clone(), |v| format!("{} entries", v.len()))
                );
            }
            let applied = this.update_in(cx, |this, window, cx| {
                if std::env::var_os("LAPIS_UI_TRACE").is_some() {
                    eprintln!("lapis workspace: directory apply epoch {epoch}/{}", this.file_epoch);
                }
                if this.file_epoch != epoch {
                    return;
                }
                this.files_loading = false;
                match result {
                    Ok(files) => {
                        this.files = files;
                        this.folder = folder;
                    }
                    Err(e) => {
                        this.status = e;
                        this.error = true;
                    }
                }
                cx.notify();
                window.refresh();
            });
            if let Err(error) = applied {
                eprintln!("lapis workspace: directory callback unavailable: {error}");
            }
        })
        .detach();
        cx.notify();
    }

    fn navigate(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((index, visit)) = self.history.target(forward) {
            match visit {
                history::Visit::Note(path) => self.open_visit(path, Some(index), window, cx),
                history::Visit::Graph => self.show_graph(Some(index), window, cx),
            }
        }
    }
    fn activate(&mut self, index: usize, travel: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_graph(cx);
        self.active = index;
        self.panes.assign(self.tabs[index].document.path.clone());
        self.history.commit(self.tabs[index].document.path.clone().into(), travel);
        self.refresh_context(false, window, cx);
        self.focus_active(window, cx);
        cx.notify();
    }
    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_visit(path, None, window, cx);
    }
    fn open_visit(
        &mut self,
        path: String,
        travel: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_epoch += 1;
        let epoch = self.open_epoch;
        if let Some(index) = self.tabs.iter().position(|t| t.document.path == path) {
            self.opening = None;
            self.status.clear();
            self.error = false;
            self.activate(index, travel, window, cx);
            return;
        }
        if !self.tab_order.contains(&path) && self.tab_order.len() >= 128 {
            self.status = "Close a tab before opening more than 128 documents".into();
            self.error = true;
            cx.notify();
            return;
        }
        self.opening = Some(path.clone());
        let requested = path.clone();
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.read(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.open_epoch != epoch {
                    return;
                }
                this.opening = None;
                match result {
                    Ok(document) => {
                        let index = this.install_document(document, window, cx);
                        this.activate(index, travel, window, cx);
                        this.status.clear();
                        this.error = false;
                    }
                    Err(e) => {
                        if this.panes.contains(&requested) {
                            this.pane_errors.insert(requested.clone(), e.clone());
                        }
                        this.status = e;
                        this.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_to(false, window, cx);
    }

    fn save_to(&mut self, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active) else { return };
        if tab.saving {
            return;
        }
        if tab.document.readonly {
            self.status = "Read-only reference".into();
            self.error = true;
            cx.notify();
            return;
        }
        tab.saving = true;
        let original = tab.document.clone();
        let text = tab.editor.read(cx).value().to_string();
        let editor = tab.editor.clone();
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move {
            if copy { service.save_copy(&original, &text) } else { service.save(&original, &text) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(index) = this.tabs.iter().position(|t| t.editor == editor) else {
                    return;
                };
                let tab = &mut this.tabs[index];
                tab.saving = false;
                let close_requested = std::mem::take(&mut tab.close_after_save);
                match result {
                    Ok(document) => {
                        this.status = format!("Saved {}", document.path);
                        this.error = false;
                        let path = document.path.clone();
                        let close =
                            close_requested && !copy && tab.editor.read(cx).value().as_ref() == document.text;
                        if copy {
                            this.open_file(path.clone(), window, cx);
                            this.directory(this.folder.clone(), window, cx);
                        } else {
                            tab.document = document;
                        }
                        if close {
                            let was_active = this.active == index;
                            let path = this.tabs[index].document.path.clone();
                            this.tab_order.retain(|p| p != &path);
                            this.panes.remove_path(&path);
                            this.tabs.remove(index);
                            this.active = if index < this.active {
                                this.active - 1
                            } else {
                                this.active.min(this.tabs.len().saturating_sub(1))
                            };
                            if was_active {
                                this.focus_active(window, cx);
                            }
                        }
                        this.reindex(path, window, cx);
                    }
                    Err(e) => {
                        this.status = e;
                        this.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn reindex(&self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let service = self.services.clone();
        let rel = path.clone();
        let task = cx.background_executor().spawn(async move { service.reindex(&rel) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = result {
                    this.status = format!("Saved {path}; indexing failed: {error}. Search may be stale.");
                    this.error = true;
                } else {
                    // A saved note can change backlinks of the currently visible note too.
                    this.refresh_context(true, window, cx);
                    if let Some(graph) = &this.graph {
                        graph.update(cx, |g, cx| g.invalidate(window, cx));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn may_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs.iter().any(|t| Self::dirty(t, cx) || t.saving) {
            self.status =
                "Unsaved changes retained. Save each changed tab, or discard it with Shift+Ctrl+W.".into();
            self.error = true;
            cx.notify();
            false
        } else {
            true
        }
    }

    fn close_tab(&mut self, discard: bool, cx: &mut Context<Self>) {
        self.command = None;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.vim.prompt = None;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            if let Some(path) = self.tab_order.first().cloned() {
                self.tab_order.retain(|p| p != &path);
                self.panes.remove_path(&path);
                self.restored.retain(|t| t.path != path);
                self.open_epoch += 1;
                self.opening = None;
                cx.notify();
            }
            return;
        };
        if tab.saving || (!discard && Self::dirty(tab, cx)) {
            self.status = "Unsaved changes retained. Save, or Shift+Ctrl+W to discard this tab.".into();
            self.error = true;
        } else {
            self.status =
                format!("{} {}", if discard { "Discarded changes in" } else { "Closed" }, tab.document.path);
            self.error = false;
            let path = tab.document.path.clone();
            self.tab_order.retain(|p| p != &path);
            self.panes.remove_path(&path);
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if (modifiers.platform || modifiers.control)
            && !(modifiers.control
                && !modifiers.platform
                && key == "r"
                && self.palette.is_none()
                && self.tabs.get(self.active).is_some_and(|t| t.vim.mode != crate::vim::Mode::Insert))
        {
            match key {
                "i" if modifiers.shift && self.palette.is_some() => self.build_index(window, cx),
                "backslash" | "\\" => self.split_pane(modifiers.shift, window, cx),
                "tab" if modifiers.alt => self.cycle_pane(window, cx),
                "g" if modifiers.shift => self.toggle_graph(window, cx),
                "p" => self.open_palette(window, cx),
                "s" => self.save_to(modifiers.shift, window, cx),
                "w" if !self.graph_visible
                    && self.panes.paths.len() > 1
                    && self.panes.paths[self.panes.focused].is_none() =>
                {
                    self.close_pane(self.panes.focused, window, cx)
                }
                "w" if self.graph_visible => self.toggle_graph(window, cx),
                "w" => {
                    self.close_tab(modifiers.shift, cx);
                    self.focus_active(window, cx);
                }
                "q" => {
                    if self.may_close(cx) {
                        cx.quit();
                    }
                }
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if self.palette.is_some() {
            match key {
                "escape" => {
                    self.palette = None;
                    self.query_epoch += 1;
                    self.focus_active(window, cx);
                }
                "enter" => self.palette_enter(window, cx),
                "down" => {
                    let p = self.palette.as_mut().unwrap();
                    p.selected = (p.selected + 1).min(p.hits.len().saturating_sub(1));
                }
                "up" => {
                    let p = self.palette.as_mut().unwrap();
                    p.selected = p.selected.saturating_sub(1);
                }
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.graph_visible {
            return;
        }
        if self.command.is_some() {
            match key {
                "escape" => {
                    self.command = None;
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.vim.reset();
                    }
                    self.focus_active(window, cx);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                "enter" => {
                    let prompt = self.command.take().unwrap();
                    if let Some(tab) = self.tabs.get_mut(self.active) {
                        tab.vim.prompt = Some((prompt.kind, prompt.input.read(cx).value().to_string()));
                    }
                    self.focus_active(window, cx);
                }
                _ => return,
            }
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if !tab.editor.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        if modifiers.shift && matches!(key, "left" | "right" | "up" | "down" | "home" | "end") {
            tab.vim.reset();
            return;
        }
        use crate::vim::{Effect, Mode};
        let key = event.keystroke.key_char.as_deref().filter(|_| modifiers.shift).unwrap_or(key);
        let state = tab.editor.read(cx);
        let source = state.value();
        let cursor = state.cursor();
        let selection = state.selected_range();
        let old_register = self.register.generation;
        let effect = tab.vim.key(key, modifiers.control, &source, cursor, selection, &mut self.register);
        if self.register.generation != old_register {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(self.register.text.clone()));
        }
        match effect {
            Effect::Pass => return,
            Effect::None => {}
            Effect::Select(range) => tab.editor.update(cx, |s, cx| s.set_selected_range(range, cx)),
            Effect::Edit { range, text, cursor } => {
                if tab.document.readonly {
                    tab.vim.reset();
                    self.status = "Read-only reference".into();
                    self.error = true;
                } else {
                    tab.editor.update(cx, |s, cx| {
                        s.set_selected_range(range, cx);
                        s.replace(text, window, cx);
                        s.set_selected_range(cursor..cursor, cx);
                    });
                }
            }
            Effect::Undo => window.dispatch_action(Box::new(gpui_kit::base::input::Undo), cx),
            Effect::Redo => window.dispatch_action(Box::new(gpui_kit::base::input::Redo), cx),
            Effect::Save => self.save(window, cx),
            Effect::Close(discard) => {
                self.close_tab(discard, cx);
                self.focus_active(window, cx);
            }
            Effect::SaveClose => {
                if tab.document.readonly || tab.saving {
                    self.status = "Cannot save and close while read-only or saving".into();
                    self.error = true;
                } else {
                    tab.close_after_save = true;
                    self.save(window, cx);
                }
            }
            Effect::Status(message) => {
                self.status = message;
                self.error = false;
            }
        }
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.document.readonly
            && tab.vim.mode == Mode::Insert
        {
            tab.vim.reset();
        }
        if self.command.is_none()
            && let Some((kind, _)) = self.tabs.get(self.active).and_then(|t| t.vim.prompt.as_ref())
        {
            self.open_command(*kind, window, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }
}

pub fn open(opts: Options, services: Arc<dyn WorkspaceServices>) -> Result<(), DesktopError> {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        gpui_omarchy::init(cx);
        init_workspace(cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            titlebar: Some(gpui_kit::TitlebarOptions {
                title: Some(opts.title.clone().into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            ..Default::default()
        };
        let seed = opts.seed;
        let opened = cx.open_window(options, move |window, cx| {
            let entity = cx.new(|cx| Workspace::new(services, window, cx));
            let weak = entity.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                weak.update(cx, |this, cx| this.may_close(cx)).unwrap_or(true)
            });
            entity
        });
        match opened {
            Ok(handle) => {
                // The root entity and window must be registered before a fast I/O completion
                // tries to update them. No loading work belongs inside the constructor.
                if let Err(error) = handle.update(cx, move |this, window, cx| {
                    this.restore_session(seed, window, cx);
                    let entity = cx.entity();
                    // Keep metadata entities until quit, including a native window-close event.
                    App::on_app_quit(cx, move |cx| {
                        let this = entity.read(cx);
                        let snapshot = this.session_ready.then(|| this.session_snapshot(cx));
                        let services = this.services.clone();
                        cx.background_executor().spawn(async move {
                            if let Some(snapshot) = snapshot
                                && let Err(error) = services.save_session(&snapshot)
                            {
                                eprintln!("lapis desktop: workspace state was not saved: {error}");
                            }
                        })
                    })
                    .detach();
                }) {
                    eprintln!("lapis desktop: workspace initialization failed: {error}");
                    cx.quit();
                }
                cx.activate(true);
            }
            Err(e) => {
                eprintln!("lapis desktop: {e}");
                cx.quit();
            }
        }
    });
    Ok(())
}
