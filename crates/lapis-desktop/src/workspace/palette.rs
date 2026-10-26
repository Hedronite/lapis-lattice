use super::*;
use gpui_kit::base::input::InputState;
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::{ActiveTheme, IconName, icon};

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
    /// Keyword hits are showing and the hybrid ranking is still on its way.
    pub(super) refining: bool,
    /// The hybrid ranking has answered (or failed); later keyword replies are stale.
    ranked: bool,
    pub(super) vector_note: Option<String>,
}

/// Which of the two searches an answer came from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Arm {
    Keyword,
    Hybrid,
}

impl Workspace {
    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command = None;
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.vim.prompt = None;
        }
        self.query_epoch += 1;
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find notes, paths, and ideas…"));
        let events = cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.query_epoch += 1;
                if let Some(p) = this.palette.as_mut() {
                    p.state = State::Draft;
                    p.hits.clear();
                    p.selected = 0;
                    p.refining = false;
                    p.vector_note = None;
                }
                cx.notify();
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.list_scroll = gpui_kit::ScrollHandle::new();
        self.palette = Some(Palette {
            input,
            _events: events,
            hits: vec![],
            selected: 0,
            state: State::Draft,
            modalities: vec![],
            refining: false,
            ranked: false,
            vector_note: None,
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
        p.refining = false;
        p.ranked = false;
        p.vector_note = None;
        self.query_epoch += 1;
        let epoch = self.query_epoch;
        // Both searches start now: keyword hits show in a fraction of a second,
        // the hybrid ranking replaces them when the embedder has answered.
        for arm in [Arm::Keyword, Arm::Hybrid] {
            let service = self.services.clone();
            let query = query.clone();
            let task = cx.background_executor().spawn(async move {
                match arm {
                    Arm::Keyword => service.search_fast(&query),
                    Arm::Hybrid => service.search(&query),
                }
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |this, _, cx| this.apply_search(epoch, arm, result, cx));
            })
            .detach();
        }
        cx.notify();
    }

    /// Merge one search answer into the open palette. Whichever arrives first shows;
    /// the hybrid answer wins once it lands, keeping the selection on the same note.
    pub(super) fn apply_search(
        &mut self,
        epoch: u64,
        arm: Arm,
        result: Result<crate::services::SearchPage, String>,
        cx: &mut Context<Self>,
    ) {
        if self.query_epoch != epoch {
            return;
        }
        let Some(p) = self.palette.as_mut() else { return };
        if arm == Arm::Keyword && p.ranked {
            return;
        }
        match result {
            Ok(page) => {
                let keep = p.hits.get(p.selected).map(|h| h.path.clone());
                let empty_index = page.indexed_documents == 0 && page.hits.is_empty();
                p.hits = page.hits;
                p.modalities = page.modalities;
                p.selected = keep.and_then(|path| p.hits.iter().position(|h| h.path == path)).unwrap_or(0);
                self.list_scroll.scroll_to_item(p.selected);
                p.state = if empty_index { State::NeedsIndex(page.can_build_index) } else { State::Ready };
                p.refining = arm == Arm::Keyword;
            }
            Err(e) if arm == Arm::Hybrid && matches!(p.state, State::Ready) => {
                // Keyword hits are already showing; keep them and say why they stay.
                p.refining = false;
                p.vector_note = Some(e);
            }
            Err(e) if arm == Arm::Keyword => {
                // The hybrid answer may still arrive; only show the error if it already failed.
                if p.ranked && !matches!(p.state, State::Ready) {
                    p.state = State::Error(e);
                }
            }
            Err(e) => {
                p.hits.clear();
                p.state = State::Error(e);
            }
        }
        if arm == Arm::Hybrid {
            p.ranked = true;
        }
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
                let status = if p.refining {
                    " · refining with vectors…".to_string()
                } else if p.vector_note.is_some() {
                    " · keyword only (vector ranking unavailable)".to_string()
                } else {
                    String::new()
                };
                format!(
                    "{} results · {}{status} · ↑ ↓ choose · Enter open",
                    p.hits.len(),
                    p.modalities.join(" + ")
                )
            }
            State::Error(e) => format!("Search failed: {e}. Enter retries."),
        };
        let mut results = div()
            .id("palette-results")
            .role(Role::ListBox)
            .aria_label("Search results")
            .track_scroll(&self.list_scroll)
            .max_h(px(440.))
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            .gap_0p5();
        for (index, hit) in p.hits.iter().enumerate() {
            let path = hit.path.clone();
            // One run of prose: the clamp counts wrapped lines, not the snippet's own newlines.
            let snippet = hit
                .snippet
                .as_deref()
                .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
                .filter(|s| !s.is_empty());
            let mut row = div()
                .id(("result", index))
                .role(Role::ListBoxOption)
                .aria_label(format!("{} · {}", hit.title, hit.path))
                .aria_selected(index == p.selected)
                .flex()
                .flex_none()
                .flex_col()
                .gap_0p5()
                .px_3()
                .py_2()
                .rounded(px(7.))
                .cursor_pointer()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(icon(IconName::FileText).size(px(14.)).text_color(theme.accent.opacity(0.8)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.bright)
                                .child(hit.title.clone()),
                        )
                        .child(super::chrome::kbd(hit.kind.to_string(), &theme)),
                )
                .child(
                    div()
                        .pl(px(22.))
                        .truncate()
                        .text_xs()
                        .text_color(theme.secondary)
                        .child(hit.path.clone()),
                )
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.palette = None;
                    this.query_epoch += 1;
                    this.open_file(path.clone(), w, cx);
                }));
            if let Some(snippet) = snippet {
                row = row.child(
                    div()
                        .pl(px(22.))
                        .line_clamp(2)
                        .text_sm()
                        .text_color(theme.foreground.opacity(0.8))
                        .child(snippet),
                );
            }
            results = results.child(if index == p.selected {
                row.bg(crate::theme::lapis_blue().opacity(0.6))
            } else {
                row.hover(|s| s.bg(theme.hover_fill()))
            });
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
            .pt(px(72.))
            .bg(theme.inset.opacity(0.7))
            .child(
                super::chrome::dialog("search-dialog", "Find in workspace", &theme)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_3()
                            .child(super::chrome::section("FIND IN WORKSPACE", &theme))
                            .child(input)
                            .child(div().text_xs().text_color(theme.secondary).child(label)),
                    )
                    .child(results),
            )
            .into_any_element()
    }
}
