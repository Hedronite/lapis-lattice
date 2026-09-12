use super::*;
use gpui_kit::base::{h_resizable, resizable_panel};
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Render, Role, SharedString, StatefulInteractiveElement,
    Styled, div, px,
};
use gpui_omarchy::ActiveTheme;

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.omarchy().clone();
        let mut files = div()
            .id("file-list")
            .role(Role::List)
            .aria_label("Workspace files")
            .flex_1()
            .overflow_y_scroll()
            .px_2();
        if !self.folder.is_empty() {
            let parent = self.folder.rsplit_once('/').map_or(String::new(), |(p, _)| p.into());
            files = files.child(
                div()
                    .id("parent-folder")
                    .role(Role::Button)
                    .aria_label("Open parent folder")
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
                    .role(Role::Button)
                    .accessibility_id(format!("workspace.file.{path}"))
                    .aria_label(format!("Open {} {}", if directory { "folder" } else { "file" }, entry.name))
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
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme.border)
            .child(div().p_4().text_color(theme.accent).child("L A P I S"))
            .child(
                div()
                    .id("workspace-search-button")
                    .role(Role::Button)
                    .aria_label("Find in workspace")
                    .px_4()
                    .pb_3()
                    .cursor_pointer()
                    .text_color(theme.secondary)
                    .child("Find a note · ⌘/Ctrl P")
                    .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
            )
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
            .role(Role::TabList)
            .aria_label("Open documents")
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
                    .role(Role::Tab)
                    .aria_label(title.clone())
                    .aria_selected(index == self.active)
                    .accessibility_id(format!("workspace.tab.{}", tab.document.path))
                    .flex_shrink_0()
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .bg(if index == self.active { theme.normal_fill() } else { theme.background })
                    .child(title)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate(index, None, window, cx);
                    })),
            );
        }
        let mut navigation = div().flex().items_center().gap_4().px_4().py_2().text_sm();
        for (label, forward) in [("‹ Back", false), ("Forward ›", true)] {
            let enabled = self.history.target(forward).is_some();
            navigation = navigation.child(
                div()
                    .id(label)
                    .role(Role::Button)
                    .aria_label(label)
                    .text_color(if enabled { theme.foreground } else { theme.secondary })
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, w, cx| this.navigate(forward, w, cx))),
            );
        }
        navigation = navigation
            .child(div().flex_1())
            .child(
                div()
                    .id("reset-layout")
                    .role(Role::Button)
                    .aria_label("Reset pane widths")
                    .cursor_pointer()
                    .child("Reset layout")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                        this.context_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                        for tab in &mut this.tabs {
                            tab.split = cx.new(|_| gpui_kit::base::ResizableState::default());
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("toggle-context")
                    .role(Role::Button)
                    .aria_label("Toggle document context")
                    .cursor_pointer()
                    .child("Context")
                    .on_click(cx.listener(|this, _, w, cx| {
                        this.context = !this.context;
                        this.refresh_context(false, w, cx);
                        cx.notify();
                    })),
            );
        let mut content = div().flex_1().min_h_0().min_w_0().flex().flex_col().child(navigation).child(tabs);
        if let Some(tab) = self.tabs.get(self.active) {
            if let Some(pdf) = &tab.pdf {
                content = content.child(div().flex_1().min_h_0().min_w_0().child(pdf.clone()));
            } else {
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
                            .role(Role::Button)
                            .aria_label("Save document")
                            .cursor_pointer()
                            .child(if tab.saving { "Saving…" } else { "Save" })
                            .on_click(cx.listener(|this, _, w, cx| this.save(w, cx))),
                    )
                    .child(
                        div()
                            .id("save-copy")
                            .role(Role::Button)
                            .aria_label("Save a copy")
                            .cursor_pointer()
                            .child("Save copy")
                            .on_click(cx.listener(|this, _, w, cx| this.save_to(true, w, cx))),
                    );
                content = content.child(toolbar);
                let mut working = div().flex().flex_1().min_h_0().min_w_0();
                let mut split = h_resizable(("source-reading-split", self.active)).with_state(&tab.split);
                if view == View::Live {
                    editor.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                    working = working.child(div().flex_1().min_w_0().h_full().child(tab.live.clone()));
                }
                if matches!(view, View::Source | View::Split) {
                    editor.update(cx, |s, _| s.set_editor_style(theme.input_style()));
                    let pane =
                        div().flex_1().min_w_0().h_full().p_5().child(gpui_kit::base::Textarea::new(&editor));
                    if view == View::Split {
                        split = split.child(resizable_panel().child(pane));
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
                        .id("reading-scroll")
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
                            .role(Role::Button)
                            .aria_label("Find in workspace")
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .child("Find a note · Ctrl/Cmd+P")
                            .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
                    ),
            );
        }
        let mut center = div().size_full().flex().min_w_0().min_h_0();
        if self.context {
            center = center.child(
                h_resizable("context-panes")
                    .with_state(&self.context_layout)
                    .child(resizable_panel().child(content))
                    .child(
                        resizable_panel()
                            .flex_none()
                            .size(px(280.))
                            .size_range(px(180.)..px(520.))
                            .child(context::render(self, cx)),
                    ),
            );
        } else {
            center = center.child(content);
        }
        let workspace_body = div().flex().flex_1().min_h_0().child(
            h_resizable("workspace-panes")
                .with_state(&self.layout)
                .child(
                    resizable_panel()
                        .flex_none()
                        .size(px(232.))
                        .size_range(px(140.)..px(480.))
                        .child(sidebar),
                )
                .child(resizable_panel().child(center)),
        );
        let mode = self.tabs.get(self.active).map_or_else(
            || "READY".to_string(),
            |t| {
                if t.document.readonly { "READ ONLY".into() } else { t.vim.label() }
            },
        );
        let status = self
            .tabs
            .get(self.active)
            .filter(|t| t.vim.prompt.is_some())
            .map(|t| t.vim.label())
            .unwrap_or_else(|| {
                self.opening.as_ref().map(|p| format!("Opening {p}…")).unwrap_or_else(|| self.status.clone())
            });
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
            .key_context("LapisWorkspace")
            .capture_action(cx.listener(|this, _: &NavigateBack, w, cx| {
                this.navigate(false, w, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &NavigateForward, w, cx| {
                this.navigate(true, w, cx);
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(|this, e, w, cx| this.key(e, w, cx)))
            .capture_action(cx.listener(|this, _: &gpui_kit::base::input::Paste, w, cx| this.paste(w, cx)))
            .child(workspace_body)
            .child(
                div()
                    .id("workspace-status")
                    .role(Role::Status)
                    .aria_label(format!("{mode}. {status}"))
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
        if let Some(prompt) = &self.command {
            surface =
                surface.child(command::render(prompt, self.error.then(|| self.status.clone()), window, cx));
        }
        surface
    }
}
