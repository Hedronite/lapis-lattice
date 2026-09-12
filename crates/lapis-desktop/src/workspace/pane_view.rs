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
            let mut toolbar = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .px_5()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .text_sm();
            for (label, choice) in [
                ("Live", View::Live),
                ("Source", View::Source),
                ("Reading", View::Reading),
                ("Split", View::Split),
            ] {
                if choice == View::Live && kind != FileKind::Markdown {
                    continue;
                }
                toolbar = toolbar.child(
                    div()
                        .id(label)
                        .role(Role::Button)
                        .aria_label(format!("{label} view"))
                        .cursor_pointer()
                        .text_color(if view == choice { theme.accent } else { theme.secondary })
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_document(index, window, cx);
                            this.set_view(index, choice, cx);
                            this.focus_active(window, cx);
                            cx.notify();
                        })),
                );
            }
            toolbar = toolbar
                .child(div().flex_1())
                .child(
                    div()
                        .id("save")
                        .role(Role::Button)
                        .aria_label("Save document")
                        .cursor_pointer()
                        .child(if tab.saving { "Saving…" } else { "Save" })
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.select_document(index, w, cx);
                            this.save(w, cx);
                        })),
                )
                .child(
                    div()
                        .id("save-copy")
                        .role(Role::Button)
                        .aria_label("Save a copy")
                        .cursor_pointer()
                        .child("Save copy")
                        .on_click(cx.listener(move |this, _, w, cx| {
                            this.select_document(index, w, cx);
                            this.save_to(true, w, cx);
                        })),
                );
            content = content.child(toolbar);
            let mut working = div().flex().flex_1().min_h_0().min_w_0();
            let mut split = h_resizable(("source-reading-split", index)).with_state(&tab.split);
            if view == View::Live {
                editor.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                working = working.child(div().flex_1().min_w_0().h_full().child(tab.live.clone()));
            }
            if matches!(view, View::Source | View::Split) {
                editor.update(cx, |s, _| s.set_editor_style(theme.input_style()));
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
                let pane = div()
                    .id(("reading-scroll", index))
                    .track_scroll(&tab.reading_scroll)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p_6()
                    .child(reader.text_size(px(16.)));
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
