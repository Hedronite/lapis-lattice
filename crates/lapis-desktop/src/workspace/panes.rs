//! Document panes keep focus, ownership and lazy restoration explicit.
use super::*;
use crate::panes::Axis;
use gpui_kit::base::{h_resizable, resizable_panel, v_resizable};
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, SharedString, StatefulInteractiveElement, Styled,
    div, px,
};
use gpui_omarchy::ActiveTheme;
impl Workspace {
    pub(super) fn select_document(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index < self.tabs.len() {
            self.active = index;
            self.panes.assign(self.tabs[index].document.path.clone());
            self.focus_active(window, cx);
        }
    }
    pub(super) fn focus_pane(&mut self, pane: usize, window: &mut Window, cx: &mut Context<Self>) {
        if pane >= self.panes.paths.len() {
            return;
        }
        if pane != self.panes.focused {
            // A late file-open response must not land in the newly focused pane.
            self.open_epoch += 1;
            self.opening = None;
        }
        self.panes.focused = pane;
        self.active = self.panes.paths[pane]
            .as_ref()
            .and_then(|p| self.tabs.iter().position(|t| &t.document.path == p))
            .unwrap_or(usize::MAX);
        if let Some(path) = self.panes.paths[pane].clone().filter(|_| self.active == usize::MAX) {
            self.pane_errors.remove(&path);
            self.open_file(path, window, cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }
    pub(super) fn split_pane(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.hide_graph(cx);
        if !self.panes.split(if down { Axis::Down } else { Axis::Across }) {
            self.status =
                "Up to four document panes can be shown. Close a pane to rearrange the workspace.".into();
            self.error = false;
            cx.notify();
            return;
        }
        self.open_epoch += 1;
        self.opening = None;
        self.document_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
        self.active = usize::MAX;
        self.command = None;
        self.status = "Choose a file or tab for this pane".into();
        self.error = false;
        self.focus_active(window, cx);
        cx.notify();
    }
    pub(super) fn cycle_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.graph_visible {
            self.hide_graph(cx);
        }
        self.focus_pane((self.panes.focused + 1) % self.panes.paths.len(), window, cx);
    }
    pub(super) fn close_pane(&mut self, pane: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.panes.close(pane);
        self.status = "Closed pane; document tabs and unsaved edits retained".into();
        self.error = false;
        self.open_epoch += 1;
        self.opening = None;
        self.document_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
        self.focus_pane(self.panes.focused, window, cx);
        // Closing a view does not close or discard its editor tab.
        self.update_pane_visibility(window, cx);
        cx.notify();
    }
    pub(super) fn update_pane_visibility(&self, window: &mut Window, cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if let Some(pdf) = &tab.pdf {
                let visible = !self.graph_visible && self.panes.contains(&tab.document.path);
                pdf.update(cx, |s, cx| s.set_visible(visible, window, cx));
            }
        }
    }
    pub(super) fn load_visible_panes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane_loading {
            return;
        }
        let paths: Vec<String> = self
            .panes
            .paths
            .iter()
            .flatten()
            .filter(|p| {
                !self.tabs.iter().any(|t| &t.document.path == *p)
                    && self.opening.as_ref() != Some(*p)
                    && !self.pane_errors.contains_key(*p)
            })
            .cloned()
            .collect();
        if paths.is_empty() {
            return;
        }
        self.pane_loading = true;
        self.pane_load_epoch += 1;
        let epoch = self.pane_load_epoch;
        let services = self.services.clone();
        let task = cx.background_executor().spawn(async move {
            paths
                .into_iter()
                .map(|p| {
                    let result = services.read(&p);
                    (p, result)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.pane_load_epoch != epoch {
                    return;
                }
                this.pane_loading = false;
                for (path, result) in results {
                    if !this.panes.contains(&path) || !this.tab_order.contains(&path) {
                        continue;
                    }
                    match result {
                        Ok(document) => {
                            this.install_document(document, window, cx);
                        }
                        Err(error) => {
                            this.pane_errors.insert(path, error);
                        }
                    }
                }
                this.update_pane_visibility(window, cx);
                this.load_visible_panes(window, cx);
                cx.notify();
                window.refresh();
            });
        })
        .detach();
    }
    pub(super) fn document_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.omarchy().clone();
        let mut split = if self.panes.axis == Axis::Across {
            h_resizable("document-panes")
        } else {
            v_resizable("document-panes")
        }
        .with_state(&self.document_layout);
        for (pane, path) in self.panes.paths.clone().into_iter().enumerate() {
            let index = path.as_ref().and_then(|p| self.tabs.iter().position(|t| &t.document.path == p));
            let focused = self.panes.focused == pane;
            let mut body = div()
                .id(("document-pane", pane))
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
                    if this.panes.focused != pane {
                        this.focus_pane(pane, w, cx);
                    }
                }));
            if self.panes.paths.len() > 1 {
                let title = path.clone().unwrap_or_else(|| "Choose a note".into());
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .border_b_1()
                        .border_color(if focused { theme.accent } else { theme.border })
                        .child(
                            div()
                                .id(("focus-document-pane", pane))
                                .role(Role::Button)
                                .aria_label(format!("Focus pane {}: {title}", pane + 1))
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_sm()
                                .cursor_pointer()
                                .child(title)
                                .on_click(cx.listener(move |this, _, w, cx| this.focus_pane(pane, w, cx))),
                        )
                        .child(
                            div()
                                .id(("close-document-pane", pane))
                                .role(Role::Button)
                                .aria_label(format!("Close pane {}", pane + 1))
                                .cursor_pointer()
                                .child("×")
                                .on_click(cx.listener(move |this, _, w, cx| this.close_pane(pane, w, cx))),
                        ),
                );
            }
            if let Some(index) = index {
                body = body.child(self.document_view(index, window, cx));
            } else {
                if let Some(error_path) = path.as_ref().filter(|p| self.pane_errors.contains_key(*p)) {
                    body = body.child(
                        div()
                            .id(("retry-document-pane", pane))
                            .role(Role::Button)
                            .aria_label(format!("Retry {error_path}"))
                            .p_3()
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .child("Retry file")
                            .on_click(cx.listener(move |this, _, w, cx| this.focus_pane(pane, w, cx))),
                    );
                }
                let message = path
                    .as_ref()
                    .map(|p| self.pane_errors.get(p).cloned().unwrap_or_else(|| format!("Loading {p}…")))
                    .unwrap_or_else(|| "Open a file or choose a tab for this pane.".into());
                body = body.child(
                    div().flex_1().p_4().child(message).child(
                        div()
                            .id(SharedString::from(format!("pane-find-{pane}")))
                            .role(Role::Button)
                            .aria_label("Find a note for this pane")
                            .py_3()
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .child("Find a note")
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.focus_pane(pane, w, cx);
                                this.open_palette(w, cx);
                            })),
                    ),
                );
            }
            let panel = resizable_panel().size_range(px(100.)..px(10000.)).child(body);
            split = split.child(if let Some(size) = self.panes.sizes.get(pane) {
                panel.size(px(*size))
            } else {
                panel
            });
        }
        div().flex_1().min_h_0().min_w_0().child(split).into_any_element()
    }
}
