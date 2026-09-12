use super::*;
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    div, px,
};
use gpui_omarchy::ActiveTheme;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.omarchy().clone();
        let mut files = div().id("file-list").flex_1().overflow_y_scroll().px_2();
        if !self.folder.is_empty() {
            let parent = self.folder.rsplit_once('/').map_or(String::new(), |(p, _)| p.into());
            files = files.child(
                div()
                    .id("parent-folder")
                    .p_2()
                    .cursor_pointer()
                    .child("‹ Parent folder")
                    .on_click(cx.listener(move |this, _, w, cx| this.directory(parent.clone(), w, cx))),
            );
        }
        for entry in &self.files {
            let path = entry.path.clone();
            let directory = entry.directory;
            let active = self.tabs.get(self.active).is_some_and(|t| t.document.path == path);
            files = files.child(
                div()
                    .id(SharedString::from(format!("file-{path}")))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if active { theme.normal_fill() } else { theme.background })
                    .child(format!("{}{}", if directory { "›  " } else { "   " }, entry.name))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        if directory {
                            this.directory(path.clone(), w, cx)
                        } else {
                            this.open_file(path.clone(), w, cx)
                        }
                    })),
            );
        }
        if self.files_loading {
            files = files.child(div().p_2().text_color(theme.secondary).child("Loading files…"));
        } else if self.files.is_empty() {
            files = files.child(div().p_2().text_color(theme.secondary).child("This folder is empty"));
        }
        let sidebar = div()
            .w(px(232.))
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.border)
            .child(div().p_4().text_color(theme.accent).child("L A P I S"))
            .child(
                div().px_4().pb_2().text_sm().text_color(theme.secondary).child(if self.folder.is_empty() {
                    "Files".into()
                } else {
                    self.folder.clone()
                }),
            )
            .child(files);
        let mut tabs = div()
            .id("workspace-tabs")
            .flex()
            .w_full()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(theme.border);
        for (index, tab) in self.tabs.iter().enumerate() {
            let title = format!("{}{}", tab.document.title, if Self::dirty(tab, cx) { " •" } else { "" });
            let title = if self.tabs.iter().filter(|t| t.document.title == tab.document.title).count() > 1 {
                format!("{}{}", tab.document.path, if Self::dirty(tab, cx) { " •" } else { "" })
            } else {
                title
            };
            tabs = tabs.child(
                div()
                    .id(("tab", index))
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .bg(if index == self.active { theme.normal_fill() } else { theme.background })
                    .child(title)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active = index;
                        this.focus_active(window, cx);
                        cx.notify();
                    })),
            );
        }
        let mut content = div().flex_1().min_h_0().min_w_0().flex().flex_col().child(tabs);
        if let Some(tab) = self.tabs.get(self.active) {
            let editor = tab.editor.clone();
            let source = editor.read(cx).value();
            let kind = tab.document.kind;
            let view = tab.view;
            let mut toolbar = div()
                .flex()
                .items_center()
                .gap_3()
                .px_5()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .text_sm();
            for (label, choice) in
                [("Source", View::Source), ("Reading", View::Reading), ("Split", View::Split)]
            {
                toolbar = toolbar.child(
                    div()
                        .id(label)
                        .cursor_pointer()
                        .text_color(if view == choice { theme.accent } else { theme.secondary })
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.tabs[this.active].view = choice;
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
                        .cursor_pointer()
                        .child(if tab.saving { "Saving…" } else { "Save" })
                        .on_click(cx.listener(|this, _, w, cx| this.save(w, cx))),
                )
                .child(
                    div()
                        .id("save-copy")
                        .cursor_pointer()
                        .child("Save copy")
                        .on_click(cx.listener(|this, _, w, cx| this.save_to(true, w, cx))),
                )
                .child(div().id("context").cursor_pointer().child("Properties").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.context = !this.context;
                        cx.notify();
                    },
                )));
            content = content.child(toolbar);
            let mut working = div().flex().flex_1().min_h_0().min_w_0();
            if view != View::Reading {
                editor.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                working = working.child(
                    div().flex_1().min_w_0().h_full().p_5().child(gpui_kit::base::Textarea::new(&editor)),
                );
            }
            if view != View::Source {
                let reader = if kind == FileKind::Html {
                    gpui_omarchy::html("reading", source.clone(), window, cx)
                } else {
                    gpui_omarchy::markdown("reading", source.clone(), window, cx)
                };
                working = working.child(
                    div()
                        .id("reading-scroll")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .p_6()
                        .child(reader.text_size(px(16.))),
                );
            }
            if self.context {
                working = working.child(
                    div()
                        .id("context-scroll")
                        .w(px(240.))
                        .h_full()
                        .overflow_y_scroll()
                        .border_l_1()
                        .border_color(theme.border)
                        .p_4()
                        .text_sm()
                        .child("Properties")
                        .child(div().mt_3().text_color(theme.secondary).child(
                            serde_json::to_string_pretty(&tab.document.properties).unwrap_or_default(),
                        )),
                );
            }
            content = content.child(working);
        } else {
            content = content.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(div().text_2xl().text_color(theme.accent).child("L A P I S"))
                    .child(
                        div()
                            .text_color(theme.secondary)
                            .child("Write clearly. Link ideas. Recall what matters."),
                    )
                    .child("Open a note from Files to begin.")
                    .child(
                        div()
                            .id("welcome-search")
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .child("Find a note · Ctrl/Cmd+P")
                            .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
                    ),
            );
        }
        let mode = self.tabs.get(self.active).map_or("READY", |t| {
            if t.document.readonly {
                "READ ONLY"
            } else if t.insert {
                "INSERT"
            } else {
                "NORMAL"
            }
        });
        let status =
            self.opening.as_ref().map(|p| format!("Opening {p}…")).unwrap_or_else(|| self.status.clone());
        let mut surface = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font.clone())
            .text_size(px(14.))
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|this, e, w, cx| this.key(e, w, cx)))
            .child(div().flex().flex_1().min_h_0().child(sidebar).child(content))
            .child(
                div()
                    .flex()
                    .gap_4()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .text_sm()
                    .child(div().text_color(theme.accent).child(mode))
                    .child(
                        div()
                            .text_color(if self.error { theme.danger } else { theme.secondary })
                            .child(status),
                    ),
            );
        if self.palette.is_some() {
            surface = surface.child(self.draw_palette(window, cx));
        }
        surface
    }
}
