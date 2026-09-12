//! Command menu: the notes workflows (new note, periodic notes, templates, quick
//! capture, tags, tasks, trash and restore, properties) over the injected services,
//! plus the external-change watch that keeps open documents current.
use super::*;
use crate::services::{Period, TaskRow, TemplateInfo};
use gpui_kit::base::input::InputState;
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::ActiveTheme;
use std::time::Duration;

/// Named workspace actions, in menu order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    NewNote,
    Daily,
    Weekly,
    Monthly,
    FromTemplate,
    Capture,
    Tags,
    Tasks,
    TrashNote,
    Restore,
    Properties,
}
impl Action {
    const ALL: [Action; 11] = [
        Action::NewNote,
        Action::Daily,
        Action::Weekly,
        Action::Monthly,
        Action::FromTemplate,
        Action::Capture,
        Action::Tags,
        Action::Tasks,
        Action::TrashNote,
        Action::Restore,
        Action::Properties,
    ];
    fn label(self) -> &'static str {
        match self {
            Action::NewNote => "New note",
            Action::Daily => "Today's daily note",
            Action::Weekly => "This week's note",
            Action::Monthly => "This month's note",
            Action::FromTemplate => "New note from template…",
            Action::Capture => "Quick capture…",
            Action::Tags => "Tags…",
            Action::Tasks => "Tasks…",
            Action::TrashNote => "Move this note to trash",
            Action::Restore => "Restore from trash…",
            Action::Properties => "Properties of this note",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Action::NewNote => "Create in the current folder and open it",
            Action::Daily | Action::Weekly | Action::Monthly => {
                "Open, creating it from the built-in template"
            }
            Action::FromTemplate => "Pick a template, then a title",
            Action::Capture => "One line into the inbox, no title needed",
            Action::Tags => "Browse tags, then the notes carrying one",
            Action::Tasks => "Open tasks across the vault · Enter toggles",
            Action::TrashNote => "Kept under the trash bucket; restorable",
            Action::Restore => "Pick a trashed note to put back",
            Action::Properties => "HAL front matter of the active document",
        }
    }
    fn id(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

/// What the input and Enter mean while the menu is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Commands,
    Templates,
    Trash,
    Tags,
    TagNotes(String),
    Tasks,
    Properties,
    /// Enter submits the input as the title of a new note (from `template` when set).
    NewNoteTitle {
        template: Option<String>,
    },
    /// Enter submits the input as the capture text.
    CaptureText,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub label: String,
    pub detail: String,
    pub id: String,
}

pub(super) struct Menu {
    pub kind: Kind,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
    pub(super) input: Entity<InputState>,
    _events: Subscription,
}
impl Menu {
    fn title(&self) -> String {
        match &self.kind {
            Kind::Commands => "Commands".into(),
            Kind::Templates => "New note from template".into(),
            Kind::Trash => "Restore from trash".into(),
            Kind::Tags => "Tags".into(),
            Kind::TagNotes(tag) => format!("Notes tagged #{tag}"),
            Kind::Tasks => "Tasks".into(),
            Kind::Properties => "Properties".into(),
            Kind::NewNoteTitle { template: None } => "New note".into(),
            Kind::NewNoteTitle { template: Some(t) } => format!("New note from {t}"),
            Kind::CaptureText => "Quick capture".into(),
        }
    }
    fn hint(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        if self.loading {
            return "Loading…".into();
        }
        match &self.kind {
            Kind::NewNoteTitle { .. } => "Type a title · Enter creates and opens it · Esc cancels".into(),
            Kind::CaptureText => "Type one line · Enter captures it to the inbox · Esc cancels".into(),
            Kind::Properties if self.entries.is_empty() => "No front matter".into(),
            Kind::Properties => "Read-only · Esc closes".into(),
            Kind::Tasks => "Type to filter · Enter toggles the task · Esc closes".into(),
            _ if self.entries.is_empty() => "Nothing here".into(),
            _ => "Type to filter · ↑ ↓ choose · Enter opens · Esc closes".into(),
        }
    }
    fn takes_text(&self) -> bool {
        matches!(self.kind, Kind::NewNoteTitle { .. } | Kind::CaptureText)
    }
    /// Indexes of the entries matching the typed filter.
    pub(super) fn visible(&self, cx: &App) -> Vec<usize> {
        if self.takes_text() {
            return vec![];
        }
        let filter = self.input.read(cx).value().to_string().to_lowercase();
        let words: Vec<&str> = filter.split_whitespace().collect();
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let hay = format!("{} {}", e.label, e.detail).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

fn task_entry(t: &TaskRow) -> Entry {
    Entry {
        label: format!("{} {}", if t.checked { "☑" } else { "☐" }, t.content),
        detail: match t.line {
            Some(line) => format!("{} · line {line}", t.path),
            None => t.path.clone(),
        },
        id: t.id.clone(),
    }
}

impl Workspace {
    pub(super) fn open_menu(&mut self, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        self.palette = None;
        self.query_epoch += 1;
        self.command = None;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.vim.prompt = None;
        }
        let placeholder = match &kind {
            Kind::NewNoteTitle { .. } => "Title",
            Kind::CaptureText => "What to capture",
            Kind::Properties => "Filter properties",
            _ => "Type to filter",
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
        let events = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change)
                && let Some(m) = this.menu.as_mut()
            {
                m.selected = 0;
                cx.notify();
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        let mut menu = Menu {
            kind: kind.clone(),
            entries: vec![],
            selected: 0,
            loading: false,
            error: None,
            input,
            _events: events,
        };
        match &kind {
            Kind::Commands => {
                menu.entries = Action::ALL
                    .iter()
                    .map(|a| Entry { label: a.label().into(), detail: a.detail().into(), id: a.id() })
                    .collect();
            }
            Kind::Properties => {
                menu.entries = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.document.properties.as_object())
                    .map(|map| {
                        map.iter()
                            .map(|(k, v)| Entry {
                                label: k.clone(),
                                detail: match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                },
                                id: k.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            Kind::NewNoteTitle { .. } | Kind::CaptureText => {}
            Kind::Templates | Kind::Trash | Kind::Tags | Kind::TagNotes(_) | Kind::Tasks => {
                menu.loading = true
            }
        }
        self.menu = Some(menu);
        match kind {
            Kind::Templates => self.menu_load(
                window,
                cx,
                |s| s.templates(),
                |list: Vec<TemplateInfo>| {
                    list.into_iter()
                        .map(|t| Entry { label: t.name, detail: t.id.clone(), id: t.id })
                        .collect()
                },
            ),
            Kind::Trash => self.menu_load(
                window,
                cx,
                |s| s.trash_list(),
                |list: Vec<String>| {
                    list.into_iter()
                        .map(|p| Entry { label: p.clone(), detail: "trashed".into(), id: p })
                        .collect()
                },
            ),
            Kind::Tags => self.menu_load(
                window,
                cx,
                |s| s.tags(),
                |list: Vec<(String, u64)>| {
                    list.into_iter()
                        .map(|(tag, n)| Entry {
                            label: format!("#{tag}"),
                            detail: format!("{n} notes"),
                            id: tag,
                        })
                        .collect()
                },
            ),
            Kind::TagNotes(tag) => self.menu_load(
                window,
                cx,
                move |s| s.tagged(&tag),
                |list: Vec<String>| {
                    list.into_iter()
                        .map(|p| Entry { label: p.clone(), detail: String::new(), id: p })
                        .collect()
                },
            ),
            Kind::Tasks => self.menu_load(
                window,
                cx,
                |s| s.tasks(),
                |list: Vec<TaskRow>| list.iter().map(task_entry).collect(),
            ),
            _ => {}
        }
        cx.notify();
    }

    /// Fill the open menu from a blocking service call.
    fn menu_load<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        load: impl FnOnce(&dyn WorkspaceServices) -> Result<T, String> + Send + 'static,
        entries: impl FnOnce(T) -> Vec<Entry> + 'static,
    ) {
        let kind = self.menu.as_ref().map(|m| m.kind.clone());
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { load(service.as_ref()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                let Some(m) = this.menu.as_mut().filter(|m| Some(&m.kind) == kind.as_ref()) else { return };
                m.loading = false;
                match result {
                    Ok(value) => m.entries = entries(value),
                    Err(e) => m.error = Some(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Run a blocking service call, then apply its result on the workspace.
    fn menu_task<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        run: impl FnOnce(&dyn WorkspaceServices) -> Result<T, String> + Send + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { run(service.as_ref()) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(value) => apply(this, value, window, cx),
                    Err(e) => {
                        if let Some(m) = this.menu.as_mut() {
                            m.loading = false;
                            m.error = Some(e.clone());
                        }
                        this.status = e;
                        this.error = true;
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Keys while the menu is open. Returns whether the key was consumed.
    pub(super) fn menu_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(m) = self.menu.as_ref() else { return false };
        match key {
            "escape" => self.close_menu(window, cx),
            "enter" => self.menu_enter(window, cx),
            "down" => {
                let n = m.visible(cx).len();
                let m = self.menu.as_mut().unwrap();
                m.selected = (m.selected + 1).min(n.saturating_sub(1));
            }
            "up" => {
                let m = self.menu.as_mut().unwrap();
                m.selected = m.selected.saturating_sub(1);
            }
            _ => return false,
        }
        true
    }

    fn menu_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (kind, text, chosen, loading) = {
            let Some(m) = self.menu.as_ref() else { return };
            let visible = m.visible(cx);
            (
                m.kind.clone(),
                m.input.read(cx).value().to_string(),
                visible.get(m.selected).map(|&i| m.entries[i].clone()),
                m.loading,
            )
        };
        if loading {
            return;
        }
        match kind {
            Kind::NewNoteTitle { template } => {
                let title = text.trim().to_string();
                if title.is_empty() {
                    return;
                }
                let folder = self.folder.clone();
                self.menu.as_mut().unwrap().loading = true;
                self.menu_task(
                    window,
                    cx,
                    move |s| s.create_note(&title, &folder, template.as_deref()),
                    |this, path, window, cx| {
                        this.menu = None;
                        this.status = format!("Created {path}");
                        this.error = false;
                        this.refresh_directory(window, cx);
                        this.open_file(path, window, cx);
                    },
                );
            }
            Kind::CaptureText => {
                let line = text.trim().to_string();
                if line.is_empty() {
                    return;
                }
                self.menu.as_mut().unwrap().loading = true;
                self.menu_task(
                    window,
                    cx,
                    move |s| s.capture(&line),
                    |this, path, window, cx| {
                        this.menu = None;
                        this.status = format!("Captured to {path}");
                        this.error = false;
                        this.refresh_directory(window, cx);
                        this.focus_active(window, cx);
                    },
                );
            }
            Kind::Commands => {
                let Some(entry) = chosen else { return };
                let Some(action) = Action::ALL.iter().copied().find(|a| a.id() == entry.id) else { return };
                self.run_action(action, window, cx);
            }
            Kind::Templates => {
                let Some(entry) = chosen else { return };
                self.open_menu(Kind::NewNoteTitle { template: Some(entry.id) }, window, cx);
            }
            Kind::Tags => {
                let Some(entry) = chosen else { return };
                self.open_menu(Kind::TagNotes(entry.id), window, cx);
            }
            Kind::TagNotes(_) => {
                let Some(entry) = chosen else { return };
                self.menu = None;
                self.open_file(entry.id, window, cx);
            }
            Kind::Trash => {
                let Some(entry) = chosen else { return };
                self.menu.as_mut().unwrap().loading = true;
                self.menu_task(
                    window,
                    cx,
                    move |s| s.restore(&entry.id),
                    |this, path, window, cx| {
                        this.menu = None;
                        this.status = format!("Restored {path}");
                        this.error = false;
                        this.refresh_directory(window, cx);
                        this.open_file(path, window, cx);
                    },
                );
            }
            Kind::Tasks => {
                let Some(entry) = chosen else { return };
                self.menu.as_mut().unwrap().loading = true;
                self.menu_task(
                    window,
                    cx,
                    move |s| s.toggle_task(&entry.id),
                    |this, checked, window, cx| {
                        this.status = if checked { "Task done".into() } else { "Task reopened".into() };
                        this.error = false;
                        // The task's note changed on disk; reload the list and any clean tab.
                        this.open_menu(Kind::Tasks, window, cx);
                    },
                );
            }
            Kind::Properties => self.close_menu(window, cx),
        }
    }

    pub(super) fn run_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::NewNote => self.open_menu(Kind::NewNoteTitle { template: None }, window, cx),
            Action::FromTemplate => self.open_menu(Kind::Templates, window, cx),
            Action::Capture => self.open_menu(Kind::CaptureText, window, cx),
            Action::Tags => self.open_menu(Kind::Tags, window, cx),
            Action::Tasks => self.open_menu(Kind::Tasks, window, cx),
            Action::Restore => self.open_menu(Kind::Trash, window, cx),
            Action::Properties => {
                if self.tabs.get(self.active).is_none() {
                    self.status = "Open a note to see its properties".into();
                    self.error = true;
                    self.close_menu(window, cx);
                    return;
                }
                self.open_menu(Kind::Properties, window, cx);
            }
            Action::Daily | Action::Weekly | Action::Monthly => {
                let period = match action {
                    Action::Daily => Period::Daily,
                    Action::Weekly => Period::Weekly,
                    _ => Period::Monthly,
                };
                if let Some(m) = self.menu.as_mut() {
                    m.loading = true;
                }
                self.menu_task(
                    window,
                    cx,
                    move |s| s.periodic(period),
                    |this, path, window, cx| {
                        this.menu = None;
                        this.status.clear();
                        this.error = false;
                        this.refresh_directory(window, cx);
                        this.open_file(path, window, cx);
                    },
                );
            }
            Action::TrashNote => self.trash_active(window, cx),
        }
    }

    fn trash_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            self.status = "Open a note to trash it".into();
            self.error = true;
            self.close_menu(window, cx);
            return;
        };
        if Self::dirty(tab, cx) {
            self.status = "Unsaved changes: save or discard the buffer before trashing".into();
            self.error = true;
            self.close_menu(window, cx);
            return;
        }
        let path = tab.document.path.clone();
        if let Some(m) = self.menu.as_mut() {
            m.loading = true;
        }
        let closing = path.clone();
        self.menu_task(
            window,
            cx,
            move |s| s.trash(&path),
            move |this, trashed, window, cx| {
                this.menu = None;
                this.close_path(&closing, window, cx);
                this.status = format!("Moved to {trashed} · Restore from trash puts it back");
                this.error = false;
                this.refresh_directory(window, cx);
                this.focus_active(window, cx);
            },
        );
    }

    fn refresh_directory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folder = self.folder.clone();
        self.directory(folder, window, cx);
    }

    /// Poll the service for files changed outside the workspace: reload clean open
    /// documents, keep dirty buffers, and refresh the file list.
    pub(super) fn start_change_watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(service) = this.read_with(cx, |this, _| this.services.clone()) else { break };
                let changed = cx.background_executor().spawn(async move { service.changed_paths() }).await;
                if changed.is_empty() {
                    continue;
                }
                if this
                    .update_in(cx, |this, window, cx| this.apply_external_changes(changed, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_external_changes(&mut self, changed: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_directory(window, cx);
        for path in changed {
            let Some(index) = self.tabs.iter().position(|t| t.document.path == path) else { continue };
            if Self::dirty(&self.tabs[index], cx) {
                self.status = format!("{path} changed on disk · your unsaved buffer is retained");
                self.error = false;
                continue;
            }
            let read_path = path.clone();
            self.menu_task(
                window,
                cx,
                move |s| s.read(&read_path),
                move |this, document, window, cx| {
                    let Some(tab) = this.tabs.iter_mut().find(|t| t.document.path == document.path) else {
                        return;
                    };
                    if Self::dirty(tab, cx) {
                        return;
                    }
                    let text = document.text.clone();
                    tab.document = document;
                    tab.editor.update(cx, |s, cx| {
                        let cursor = s.cursor().min(text.len());
                        s.set_value(text, window, cx);
                        s.set_selected_range(cursor..cursor, cx);
                    });
                    this.status = format!("{path} reloaded from disk");
                    this.error = false;
                    this.refresh_context(true, window, cx);
                },
            );
        }
        cx.notify();
    }

    pub(super) fn draw_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let m = self.menu.as_ref().unwrap();
        let theme = cx.omarchy().clone();
        let visible = m.visible(cx);
        let mut list = div()
            .id("menu-entries")
            .role(Role::ListBox)
            .aria_label(m.title())
            .max_h(px(420.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (position, &index) in visible.iter().enumerate() {
            let entry = &m.entries[index];
            let id = entry.id.clone();
            list = list.child(
                div()
                    .id(("menu-entry", position))
                    .role(Role::ListBoxOption)
                    .aria_label(format!("{} · {}", entry.label, entry.detail))
                    .aria_selected(position == m.selected)
                    .p_3()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if position == m.selected { theme.normal_fill() } else { theme.background })
                    .child(entry.label.clone())
                    .child(div().text_sm().text_color(theme.secondary).child(entry.detail.clone()))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        let position = this
                            .menu
                            .as_ref()
                            .and_then(|m| m.visible(cx).iter().position(|&i| m.entries[i].id == id));
                        if let Some(p) = position {
                            this.menu.as_mut().unwrap().selected = p;
                            this.menu_enter(w, cx);
                        }
                    })),
            );
        }
        let input = gpui_omarchy::input("workspace-menu", &m.input, window, cx);
        let hint_color = if m.error.is_some() { theme.danger } else { theme.secondary };
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(64.))
            .bg(theme.background.opacity(0.75))
            .child(
                div()
                    .id("menu-dialog")
                    .role(Role::Dialog)
                    .aria_label(m.title())
                    .w(px(640.))
                    .max_w_full()
                    .h_auto()
                    .self_start()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(m.title())
                    .child(input)
                    .child(div().text_sm().text_color(hint_color).child(m.hint()))
                    .child(list),
            )
            .into_any_element()
    }
}
