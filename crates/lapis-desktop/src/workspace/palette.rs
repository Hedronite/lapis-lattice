use super::*;
use gpui_kit::base::input::InputState;
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::ActiveTheme;

pub(super) enum State {
    Draft,
    Loading,
    Ready,
    Error(String),
    NeedsIndex(bool),
    Indexing,
}
pub(super) struct Palette {
    input: Entity<InputState>,
    _events: Subscription,
    pub(super) hits: Vec<lapis_lattice::Hit>,
    pub(super) selected: usize,
    state: State,
    modalities: Vec<String>,
}

impl Workspace {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query_epoch += 1;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find notes, paths, and ideas…"));
        let events = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query_epoch += 1;
                if let Some(p) = this.palette.as_mut() {
                    p.state = State::Draft;
                    p.hits.clear();
                    p.selected = 0;
                }
                cx.notify();
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.palette = Some(Palette {
            input,
            _events: events,
            hits: vec![],
            selected: 0,
            state: State::Draft,
            modalities: vec![],
        });
        cx.notify();
    }

    pub(super) fn palette_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.palette.as_mut() else { return };
        if matches!(p.state, State::Ready)
            && let Some(hit) = p.hits.get(p.selected)
        {
            let path = hit.path.clone();
            self.palette = None;
            self.open_file(path, window, cx);
            return;
        }
        let query = p.input.read(cx).value().to_string();
        if query.trim().is_empty() || matches!(p.state, State::Loading | State::Indexing) {
            return;
        }
        p.state = State::Loading;
        self.query_epoch += 1;
        let epoch = self.query_epoch;
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.search(&query) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                if this.query_epoch != epoch {
                    return;
                }
                let Some(p) = this.palette.as_mut() else { return };
                match result {
                    Ok(page) => {
                        let empty_index = page.indexed_documents == 0 && page.hits.is_empty();
                        p.hits = page.hits;
                        p.modalities = page.modalities;
                        p.selected = 0;
                        p.state =
                            if empty_index { State::NeedsIndex(page.can_build_index) } else { State::Ready };
                    }
                    Err(e) => {
                        p.hits.clear();
                        p.state = State::Error(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn build_index(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.indexing {
            return;
        }
        let Some(p) = self.palette.as_mut() else { return };
        if !matches!(p.state, State::NeedsIndex(true)) {
            return;
        }
        p.state = State::Indexing;
        self.query_epoch += 1;
        let epoch = self.query_epoch;
        self.indexing = true;
        self.status = "Indexing workspace… Files remain available.".into();
        self.error = false;
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.build_index() });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.indexing = false;
                match result {
                    Ok(count) => {
                        this.status = format!("Indexed {count} notes. Search is ready.");
                        this.error = false;
                        if this.query_epoch == epoch
                            && let Some(p) = this.palette.as_mut()
                        {
                            p.state = State::Draft;
                        }
                    }
                    Err(e) => {
                        this.status = format!("Indexing failed: {e}");
                        this.error = true;
                        if this.query_epoch == epoch
                            && let Some(p) = this.palette.as_mut()
                        {
                            p.state = State::Error(e);
                        }
                    }
                }
                window.refresh();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn draw_palette(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let p = self.palette.as_ref().unwrap();
        let theme = cx.omarchy().clone();
        let label = match &p.state {
            State::Draft => "Enter to search · Esc to close".into(),
            State::Loading => "Searching…".into(),
            State::Indexing => "Indexing workspace… Files remain available; Esc closes search.".into(),
            State::NeedsIndex(true) => {
                "The index has no notes yet. Build it to search this workspace.".into()
            }
            State::NeedsIndex(false) => {
                "The HTTP service reports no indexed notes. Index the vault through that service, then retry."
                    .into()
            }
            State::Ready if p.hits.is_empty() => "No matching notes. Change the query and try again.".into(),
            State::Ready => {
                format!("{} results · {} · ↑ ↓ choose · Enter open", p.hits.len(), p.modalities.join(" + "))
            }
            State::Error(e) => format!("Search failed: {e}. Enter retries."),
        };
        let mut results = div()
            .id("palette-results")
            .role(Role::ListBox)
            .aria_label("Search results")
            .max_h(px(420.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        for (index, hit) in p.hits.iter().enumerate() {
            let path = hit.path.clone();
            results = results.child(
                div()
                    .id(("result", index))
                    .role(Role::ListBoxOption)
                    .aria_label(format!("{} · {}", hit.title, hit.path))
                    .aria_selected(index == p.selected)
                    .p_3()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if index == p.selected { theme.normal_fill() } else { theme.background })
                    .child(hit.title.clone())
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.secondary)
                            .child(format!("{} · {}", hit.kind, hit.path)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.secondary)
                            .child(hit.snippet.clone().unwrap_or_default()),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.palette = None;
                        this.query_epoch += 1;
                        this.open_file(path.clone(), w, cx);
                    })),
            );
        }
        if matches!(p.state, State::NeedsIndex(true)) {
            results = results.child(
                div()
                    .id("build-index")
                    .role(Role::Button)
                    .aria_label("Build workspace index")
                    .p_3()
                    .cursor_pointer()
                    .text_color(theme.accent)
                    .child("Build index · Ctrl+Shift+I")
                    .on_click(cx.listener(|this, _, w, cx| this.build_index(w, cx))),
            );
        }
        let input = gpui_omarchy::input("workspace-search", &p.input, window, cx);
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(64.))
            .bg(theme.background.opacity(0.75))
            .child(
                div()
                    .id("search-dialog")
                    .role(Role::Dialog)
                    .aria_label("Find in workspace")
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
                    .child("Find in workspace")
                    .child(input)
                    .child(div().text_sm().text_color(theme.secondary).child(label))
                    .child(results),
            )
            .into_any_element()
    }
}
