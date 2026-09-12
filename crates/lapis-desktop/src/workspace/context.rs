//! On-demand context through canonical services, with independent link/tree errors.
use super::*;
use crate::services::{ContextLink, ContextTree};
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::ActiveTheme;

#[derive(Default)]
pub(super) struct Panel {
    pub(super) path: Option<String>,
    epoch: u64,
    loading: bool,
    pub(super) links: Option<Result<Vec<ContextLink>, String>>,
    tree_loading: bool,
    pub(super) tree: Option<Result<ContextTree, String>>,
}
impl Workspace {
    pub(super) fn refresh_context(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.context {
            return;
        }
        let path = self.tabs.get(self.active).map(|t| t.document.path.clone());
        if !force && self.context_state.path == path {
            return;
        }
        let p = &mut self.context_state;
        p.epoch += 1;
        p.path = path.clone();
        p.links = None;
        p.tree = None;
        p.tree_loading = false;
        p.loading = path.is_some();
        let Some(path) = path else { return };
        let epoch = p.epoch;
        let service = self.services.clone();
        let query = path.clone();
        let task = cx.background_executor().spawn(async move { service.links(&query) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                let p = &mut this.context_state;
                if p.epoch != epoch || p.path.as_ref() != Some(&path) {
                    return;
                }
                p.loading = false;
                p.links = Some(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn context_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = &mut self.context_state;
        if p.tree_loading {
            return;
        }
        let Some(path) = p.path.clone() else { return };
        p.tree_loading = true;
        let epoch = p.epoch;
        let query = path.clone();
        let service = self.services.clone();
        let task = cx.background_executor().spawn(async move { service.tree(&query) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, _, cx| {
                let p = &mut this.context_state;
                if p.epoch != epoch || p.path.as_ref() != Some(&path) {
                    return;
                }
                p.tree_loading = false;
                p.tree = Some(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

pub(super) fn render(this: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    let panel = &this.context_state;
    let mut content = div()
        .id("workspace-context")
        .role(Role::Group)
        .aria_label("Document context")
        .w_full()
        .h_full()
        .overflow_y_scroll()
        .border_l_1()
        .border_color(theme.border)
        .p_4()
        .text_sm()
        .child(div().text_color(theme.accent).child("Document context"));
    let Some(tab) = this.tabs.get(this.active) else {
        return content.child("Open a document to see its context.");
    };
    content = content.child(div().mt_2().text_color(theme.secondary).child(tab.document.path.clone())).child(
        div()
            .id("context-refresh")
            .role(Role::Button)
            .aria_label("Refresh document links")
            .mt_3()
            .cursor_pointer()
            .child("Refresh links")
            .on_click(cx.listener(|this, _, w, cx| this.refresh_context(true, w, cx))),
    );
    if panel.loading {
        content = content.child(div().mt_3().child("Loading links…"));
    } else if let Some(result) = &panel.links {
        match result {
            Err(error) => content = content.child(div().mt_3().text_color(theme.danger).child(error.clone())),
            Ok(links) if links.is_empty() => {
                content = content.child(
                    div().mt_3().text_color(theme.secondary).child("No indexed links for this document."),
                )
            }
            Ok(links) => {
                for (label, direction) in [("Backlinks", "in"), ("Outgoing links", "out")] {
                    content = content.child(div().mt_4().text_color(theme.accent).child(label));
                    let mut count = 0;
                    for (i, link) in links.iter().enumerate().filter(|(_, l)| l.direction == direction) {
                        count += 1;
                        let mut row = div().id(("context-link", i)).mt_2().child(link.label.clone());
                        if let Some(path) = &link.path {
                            let path = path.clone();
                            row = row
                                .role(Role::Button)
                                .aria_label(format!("Open link {path}"))
                                .cursor_pointer()
                                .on_click(
                                    cx.listener(move |this, _, w, cx| this.open_file(path.clone(), w, cx)),
                                );
                        } else {
                            row = row.text_color(theme.secondary).child(" · unresolved");
                        }
                        content = content.child(row);
                    }
                    if count == 0 {
                        content = content.child(div().text_color(theme.secondary).child("None"));
                    }
                }
            }
        }
    }
    content = content.child(
        div()
            .id("context-tree")
            .role(Role::Button)
            .aria_label("Retrieve related tree")
            .mt_4()
            .cursor_pointer()
            .text_color(theme.accent)
            .child(if panel.tree_loading { "Retrieving tree…" } else { "Traverse related tree" })
            .on_click(cx.listener(|this, _, w, cx| this.context_tree(w, cx))),
    );
    if let Some(result) = &panel.tree {
        match result {
            Err(error) => content = content.child(div().mt_2().text_color(theme.danger).child(error.clone())),
            Ok(tree) => {
                content = content
                    .child(div().mt_2().text_color(theme.secondary).child(format!("Seed: {}", tree.seed)));
                for (i, (path, depth)) in tree.nodes.iter().enumerate() {
                    let path = path.clone();
                    content = content.child(
                        div()
                            .id(("context-tree-node", i))
                            .role(Role::Button)
                            .aria_label(format!("Open tree node {path}"))
                            .mt_2()
                            .pl(px((*depth).min(8) as f32 * 8.))
                            .cursor_pointer()
                            .child(path.clone())
                            .on_click(cx.listener(move |this, _, w, cx| this.open_file(path.clone(), w, cx))),
                    );
                }
                if tree.truncated {
                    content = content.child("Showing the first 50 tree nodes.");
                }
            }
        }
    }
    content.child(div().mt_5().text_color(theme.accent).child("Properties")).child(
        div()
            .mt_2()
            .text_color(theme.secondary)
            .child(serde_json::to_string_pretty(&tab.document.properties).unwrap_or_default()),
    )
}
