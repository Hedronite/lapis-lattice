//! Independent document surfaces share the existing editors and reader caches.
use super::*;
use gpui_kit::base::{h_resizable, resizable_panel};
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::ActiveTheme;
impl Workspace {
    pub(super) fn document_view(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.omarchy().clone();
        let tab = &self.tabs[index];
        let mut content = div().size_full().flex().flex_col().min_h_0().min_w_0();
        if let Some(pdf) = &tab.pdf {
            content = content.child(div().flex_1().min_h_0().min_w_0().child(pdf.clone()));
        } else {
            let editor = tab.editor.clone();
            let source = editor.read(cx).value();
            let kind = tab.document.kind;
            let view = tab.view;
            let dirty = Self::dirty(tab, cx);
            let mut views = div()
                .flex()
                .flex_none()
                .items_center()
                .gap_0p5()
                .p_0p5()
                .rounded(px(7.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.inset);
            for (label, choice) in [
                ("Live", View::Live),
                ("Source", View::Source),
                ("Reading", View::Reading),
                ("Split", View::Split),
            ] {
                if choice == View::Live && kind != FileKind::Markdown {
                    continue;
                }
                let selected = view == choice;
                let mut segment = div()
                    .id(label)
                    .role(Role::Button)
                    .aria_label(format!("{label} view"))
                    .px_2p5()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded(px(5.))
                    .cursor_pointer()
                    .child(label);
                segment = if selected {
                    segment.bg(theme.surface).text_color(theme.bright)
                } else {
                    segment.text_color(theme.secondary).hover(|s| s.text_color(theme.foreground))
                };
                views = views.child(segment.on_click(cx.listener(move |this, _, window, cx| {
                    this.select_document(index, window, cx);
                    this.set_view(index, choice, cx);
                    this.focus_active(window, cx);
                    cx.notify();
                })));
            }
            let mut save = div()
                .id("save")
                .role(Role::Button)
                .aria_label("Save document")
                .flex()
                .items_center()
                .h(px(24.))
                .px_2p5()
                .rounded(px(6.))
                .cursor_pointer()
                .child(if tab.saving {
                    "Saving…"
                } else if dirty {
                    "Save"
                } else {
                    "Saved"
                });
            save = if dirty && !tab.document.readonly {
                save.bg(theme.accent).text_color(theme.on_accent).hover(|s| s.bg(theme.accent.opacity(0.85)))
            } else {
                save.text_color(theme.secondary).hover(|s| s.bg(theme.hover_fill()))
            };
            let toolbar = div()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_4()
                .h(px(40.))
                .border_b_1()
                .border_color(theme.border)
                .text_sm()
                .child(views)
                .child(div().flex_1())
                .child(save.on_click(cx.listener(move |this, _, w, cx| {
                    this.select_document(index, w, cx);
                    this.save(w, cx);
                })))
                .child(
                    super::chrome::text_button("save-copy", None, "Save copy", &theme)
                        .aria_label("Save a copy")
                        .h(px(24.))
                        .text_color(theme.secondary)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.select_document(index, w, cx);
                            this.save_to(true, w, cx);
                        })),
                );
            content = content.child(toolbar);
            let mut working = div().flex().flex_1().min_h_0().min_w_0();
            let mut split = h_resizable(("source-reading-split", index)).with_state(&tab.split);
            if view == View::Live {
                editor.update(cx, |s, _| s.set_editor_style(crate::theme::source_style(&theme)));
                working = working.child(div().flex_1().min_w_0().h_full().child(tab.live.clone()));
            }
            if matches!(view, View::Source | View::Split) {
                editor.update(cx, |s, _| s.set_editor_style(crate::theme::source_style(&theme)));
                let pane =
                    div().flex_1().min_w_0().h_full().p_5().child(gpui_kit::base::Textarea::new(&editor));
                if view == View::Split {
                    let panel = resizable_panel().child(pane);
                    split = split.child(if let Some(width) = tab.split_width {
                        panel.size(px(width))
                    } else {
                        panel
                    });
                } else {
                    working = working.child(pane);
                }
            }
            if matches!(view, View::Reading | View::Split) {
                let reader = if kind == FileKind::Html {
                    gpui_omarchy::html("reading", source.clone(), window, cx)
                } else {
                    gpui_omarchy::markdown("reading", source.clone(), window, cx)
                };
                // A measure of about 72 characters, centred, however wide the pane is.
                let pane = div()
                    .id(("reading-scroll", index))
                    .track_scroll(&tab.reading_scroll)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_8()
                    .py_8()
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .child(div().w_full().max_w(px(720.)).child(reader.text_size(px(16.)))),
                    );
                if view == View::Split {
                    split = split.child(resizable_panel().child(pane));
                } else {
                    working = working.child(pane);
                }
            }
            if view == View::Split {
                working = working.child(split);
            }
            content = content.child(working);
        }

        content.into_any_element()
    }
}
