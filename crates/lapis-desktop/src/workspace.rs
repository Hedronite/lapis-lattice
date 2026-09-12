//! Native notes shell. Files load independently from the optional graph/index.

mod palette;
mod render;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Source,
    Reading,
    Split,
}

struct Tab {
    document: Document,
    editor: Entity<TextareaState>,
    _events: Subscription,
    view: View,
    insert: bool,
    saving: bool,
}

struct Workspace {
    services: Arc<dyn WorkspaceServices>,
    files: Vec<FileEntry>,
    folder: String,
    files_loading: bool,
    file_epoch: u64,
    open_epoch: u64,
    opening: Option<String>,
    tabs: Vec<Tab>,
    active: usize,
    status: String,
    error: bool,
    focus: FocusHandle,
    context: bool,
    palette: Option<palette::Palette>,
    query_epoch: u64,
}

impl Workspace {
    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active).filter(|t| t.view != View::Reading) {
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

    fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.open_epoch += 1;
        let epoch = self.open_epoch;
        if let Some(index) = self.tabs.iter().position(|t| t.document.path == path) {
            self.active = index;
            self.opening = None;
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        self.opening = Some(path.clone());
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
                        let editor = cx.new(|cx| {
                            let mut input =
                                TextareaState::new(window, cx).default_value(document.text.clone()).rows(30);
                            input.set_readonly(document.readonly, cx);
                            input
                        });
                        let events = cx.subscribe(&editor, |_, _, event, cx| {
                            if matches!(event, InputEvent::Change) {
                                cx.notify();
                            }
                        });
                        let view = if document.kind == FileKind::Html { View::Reading } else { View::Source };
                        editor.update(cx, |s, cx| s.focus(window, cx));
                        this.tabs.push(Tab {
                            document,
                            editor,
                            _events: events,
                            view,
                            insert: false,
                            saving: false,
                        });
                        this.active = this.tabs.len() - 1;
                        this.focus_active(window, cx);
                        this.status.clear();
                        this.error = false;
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
                let Some(tab) = this.tabs.iter_mut().find(|t| t.editor == editor) else {
                    return;
                };
                tab.saving = false;
                match result {
                    Ok(document) => {
                        this.status = format!("Saved {}", document.path);
                        this.error = false;
                        let path = document.path.clone();
                        if copy {
                            this.open_file(path.clone(), window, cx);
                            this.directory(this.folder.clone(), window, cx);
                        } else {
                            tab.document = document;
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
            if let Err(error) = task.await {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.status = format!("Saved {path}; indexing failed: {error}. Search may be stale.");
                    this.error = true;
                    cx.notify();
                });
            }
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
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        if tab.saving || (!discard && Self::dirty(tab, cx)) {
            self.status = "Unsaved changes retained. Save, or Shift+Ctrl+W to discard this tab.".into();
            self.error = true;
        } else {
            self.status =
                format!("{} {}", if discard { "Discarded changes in" } else { "Closed" }, tab.document.path);
            self.error = false;
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
        }
        cx.notify();
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.platform || modifiers.control {
            match key {
                "p" => self.open_palette(window, cx),
                "s" => self.save_to(modifiers.shift, window, cx),
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
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if !tab.editor.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        if key == "escape" {
            tab.insert = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if tab.insert || tab.document.readonly {
            return;
        }
        match key {
            "i" => tab.insert = true,
            "a" => {
                tab.insert = true;
                tab.editor.update(cx, |s, cx| {
                    let p = s.cursor();
                    let v = s.value();
                    let end = v[p..].chars().next().map_or(p, |c| p + c.len_utf8());
                    s.set_selected_range(end..end, cx);
                });
            }
            "h" | "l" => tab.editor.update(cx, |s, cx| {
                let p = s.cursor();
                let value = s.value();
                let next = if key == "h" {
                    value[..p].char_indices().last().map_or(0, |(i, _)| i)
                } else {
                    value[p..].chars().next().map_or(p, |c| p + c.len_utf8())
                };
                s.set_selected_range(next..next, cx);
            }),
            "j" | "k" => tab.editor.update(cx, |s, cx| {
                let p = s.cursor_position();
                let row = if key == "j" { p.line + 1 } else { p.line.saturating_sub(1) };
                s.set_cursor_position(
                    gpui_kit::base::input::Position { line: row, character: p.character },
                    window,
                    cx,
                );
            }),
            _ if key.chars().count() != 1 => return,
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }
}

pub fn open(opts: Options, services: Arc<dyn WorkspaceServices>) -> Result<(), DesktopError> {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx: &mut App| {
        gpui_omarchy::init(cx);
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
            let entity = cx.new(|cx| {
                let workspace = Workspace {
                    services,
                    files: vec![],
                    folder: String::new(),
                    files_loading: false,
                    file_epoch: 0,
                    open_epoch: 0,
                    opening: None,
                    tabs: vec![],
                    active: 0,
                    status: String::new(),
                    error: false,
                    focus: cx.focus_handle(),
                    context: false,
                    palette: None,
                    query_epoch: 0,
                };
                workspace.focus.focus(window, cx);
                workspace
            });
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
                    this.directory(String::new(), window, cx);
                    if let Some(path) = seed {
                        this.open_file(path, window, cx);
                    }
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
