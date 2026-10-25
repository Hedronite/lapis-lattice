use super::chrome::{hex_chain, hex_mark, icon_button, kbd, rule, section, text_button};
use super::*;
use gpui_kit::base::{h_resizable, resizable_panel};
use gpui_kit::{
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_omarchy::{ActiveTheme, IconName, icon};

/// Shortcut prefix shown in hints: the Command glyph on macOS, `Ctrl` elsewhere.
const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl " };

/// The last two path components, enough to tell same-named notes apart.
fn short_path(path: &str) -> String {
    let mut parts: Vec<&str> = path.rsplitn(3, '/').take(2).collect();
    parts.reverse();
    parts.join("/")
}

impl Workspace {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let mut files = div()
            .id("file-list")
            .role(Role::List)
            .aria_label("Workspace files")
            .flex_1()
            // Without a zero minimum the list grows past the sidebar and never scrolls.
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            .gap_px();
        if !self.folder.is_empty() {
            let parent = self.folder.rsplit_once('/').map_or(String::new(), |(p, _)| p.into());
            files = files.child(
                div()
                    .id("parent-folder")
                    .role(Role::Button)
                    .aria_label("Open parent folder")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(theme.secondary)
                    .hover(|s| s.bg(theme.hover_fill()))
                    .child(icon(IconName::ChevronLeft).size(px(14.)))
                    .child("Parent folder")
                    .on_click(cx.listener(move |this, _, w, cx| this.directory(parent.clone(), w, cx))),
            );
        }
        for entry in &self.files {
            let path = entry.path.clone();
            let directory = entry.directory;
            let active = self.tabs.get(self.active).is_some_and(|t| t.document.path == path);
            let glyph = if directory { IconName::Folder } else { IconName::FileText };
            let mut row = div()
                .id(SharedString::from(format!("file-{path}")))
                .role(Role::Button)
                .accessibility_id(format!("workspace.file.{path}"))
                .aria_label(format!("Open {} {}", if directory { "folder" } else { "file" }, entry.name))
                .relative()
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .child(icon(glyph).size(px(14.)).text_color(if directory {
                    theme.accent.opacity(0.75)
                } else {
                    theme.secondary
                }))
                .child(div().flex_1().min_w_0().truncate().child(entry.name.clone()));
            if active {
                row = row.bg(crate::theme::lapis_blue().opacity(0.55)).text_color(theme.bright).child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.))
                        .bottom(px(6.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(theme.accent),
                );
            } else {
                row = row.hover(|s| s.bg(theme.hover_fill()));
            }
            files = files.child(row.on_click(cx.listener(move |this, _, w, cx| {
                if directory {
                    this.directory(path.clone(), w, cx)
                } else {
                    this.open_file(path.clone(), w, cx)
                }
            })));
        }
        if self.files_loading {
            files = files.child(div().p_2().text_sm().text_color(theme.secondary).child("Loading files…"));
        } else if self.files.is_empty() {
            files =
                files.child(div().p_2().text_sm().text_color(theme.secondary).child("This folder is empty"));
        }
        let folder_name = if self.folder.is_empty() {
            "Vault".to_string()
        } else {
            self.folder.rsplit('/').next().unwrap_or("").into()
        };
        div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .bg(theme.inset)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div().flex().items_center().gap_2().px_4().pt_4().pb_3().child(hex_mark(&theme)).child(
                    div()
                        .font_family(crate::theme::SERIF)
                        .text_size(px(19.))
                        .text_color(theme.bright)
                        .child("Lapis"),
                ),
            )
            .child(
                div()
                    .id("workspace-search-button")
                    .role(Role::Button)
                    .aria_label("Find in workspace")
                    .mx_3()
                    .mb_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .px_2p5()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .cursor_pointer()
                    .text_color(theme.secondary)
                    .hover(|s| s.border_color(theme.accent.opacity(0.5)))
                    .child(icon(IconName::Search).size(px(14.)))
                    .child(div().flex_1().child("Find a note"))
                    .child(kbd(format!("{MOD}P"), &theme))
                    .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
            )
            .child(
                div()
                    .id("workspace-commands-button")
                    .role(Role::Button)
                    .aria_label("Commands")
                    .mx_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(30.))
                    .px_2p5()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(theme.secondary)
                    .hover(|s| s.bg(theme.hover_fill()))
                    .child(icon(IconName::Menu).size(px(14.)))
                    .child(div().flex_1().child("Commands"))
                    .child(kbd(format!("{MOD}K"), &theme))
                    .on_click(
                        cx.listener(|this, _, w, cx| this.open_menu(super::menu::Kind::Commands, w, cx)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .pt_4()
                    .pb_1p5()
                    .child(section("FILES", &theme))
                    .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.secondary).child(
                        if self.folder.is_empty() { String::new() } else { format!("· {folder_name}") },
                    )),
            )
            .child(files)
            .child(
                div()
                    .id("workspace-graph")
                    .role(Role::Button)
                    .aria_label("Open workspace graph")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .mx_2()
                    .my_2()
                    .h(px(32.))
                    .px_2p5()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .text_color(if self.graph_visible { theme.accent } else { theme.foreground })
                    .bg(if self.graph_visible { theme.accent.opacity(0.12) } else { theme.inset })
                    .hover(|s| s.bg(theme.hover_fill()))
                    .child(icon(IconName::Network).size(px(15.)).text_color(theme.accent))
                    .child(div().flex_1().child("Graph"))
                    .child(kbd(format!("{MOD}⇧G"), &theme))
                    .on_click(cx.listener(|this, _, w, cx| this.toggle_graph(w, cx))),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let mut tabs = div()
            .id("workspace-tabs")
            .role(Role::TabList)
            .aria_label("Open documents")
            .track_scroll(&self.tab_scroll)
            .flex()
            .flex_none()
            .w_full()
            .h(px(36.))
            .overflow_x_scroll()
            .bg(theme.inset)
            .border_b_1()
            .border_color(theme.border);
        for (ordinal, path) in self.tab_order.iter().enumerate() {
            let loaded = self.tabs.iter().enumerate().find(|(_, t)| &t.document.path == path);
            let title = loaded
                .map(|(_, t)| t.document.title.clone())
                .or_else(|| self.restored.iter().find(|t| &t.path == path).map(|t| t.title.clone()))
                .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path).into());
            let ambiguous =
                self.tab_order.iter().filter(|p| p.rsplit('/').next() == path.rsplit('/').next()).count() > 1
                    || self.tabs.iter().filter(|t| t.document.title == title).count() > 1;
            let title = if ambiguous { short_path(path) } else { title };
            let dirty = loaded.is_some_and(|(_, t)| Self::dirty(t, cx));
            let selected = !self.graph_visible && loaded.is_some_and(|(i, _)| i == self.active);
            let open = path.clone();
            let close = path.clone();
            let mut tab = div()
                .id(("tab", ordinal))
                .role(Role::Tab)
                .aria_label(format!("{title}{}", if dirty { " •" } else { "" }))
                .aria_selected(selected)
                .accessibility_id(format!("workspace.tab.{path}"))
                .relative()
                .flex()
                .gap_2()
                .items_center()
                .flex_shrink_0()
                .max_w(px(240.))
                .h_full()
                .pl_3()
                .pr_1p5()
                .border_r_1()
                .border_color(theme.border)
                .cursor_pointer()
                .text_sm()
                .child(div().min_w_0().truncate().child(title));
            tab = if selected {
                tab.bg(theme.background)
                    .text_color(theme.bright)
                    .child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(theme.accent))
            } else {
                tab.text_color(theme.secondary).hover(|s| s.bg(theme.hover_fill()))
            };
            tab = tab.child(
                div()
                    .id(("close-tab", ordinal))
                    .role(Role::Button)
                    .aria_label(format!("Close {path}"))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded(px(4.))
                    .text_color(if dirty { theme.accent } else { theme.secondary })
                    .hover(|s| s.bg(theme.hover_fill()).text_color(theme.bright))
                    .child(if dirty {
                        div().size(px(7.)).rounded(px(4.)).bg(theme.accent).into_any_element()
                    } else {
                        icon(IconName::Close).size(px(12.)).into_any_element()
                    })
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.close_path(&close, w, cx);
                        cx.stop_propagation();
                    })),
            );
            tabs = tabs
                .child(tab.on_click(cx.listener(move |this, _, w, cx| this.open_file(open.clone(), w, cx))));
        }
        tabs
    }

    fn render_navigation(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let mut navigation = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .h(px(40.))
            .px_2()
            .border_b_1()
            .border_color(theme.border);
        for (id, glyph, label, forward) in [
            ("‹ Back", IconName::ArrowLeft, "Back", false),
            ("Forward ›", IconName::ArrowRight, "Forward", true),
        ] {
            let enabled = self.history.target(forward).is_some();
            navigation = navigation.child(
                icon_button(id, glyph, label, false, enabled, &theme)
                    .on_click(cx.listener(move |this, _, w, cx| this.navigate(forward, w, cx))),
            );
        }
        // Where the active document lives, folder segments muted and the name bright.
        let mut crumbs =
            div().flex().flex_1().min_w_0().items_center().gap_1p5().px_2().text_sm().overflow_hidden();
        let active_path = if self.graph_visible {
            None
        } else {
            self.tabs.get(self.active).map(|t| t.document.path.clone()).or_else(|| self.opening.clone())
        };
        if let Some(path) = active_path {
            let parts: Vec<&str> = path.split('/').collect();
            let last = parts.len() - 1;
            let skip = last.saturating_sub(3);
            if skip > 0 {
                crumbs = crumbs
                    .child(div().flex_none().text_color(theme.secondary).child("…"))
                    .child(icon(IconName::ChevronRight).size(px(12.)).text_color(theme.border));
            }
            for (i, part) in parts.iter().enumerate().skip(skip) {
                if i > skip {
                    crumbs =
                        crumbs.child(icon(IconName::ChevronRight).size(px(12.)).text_color(theme.border));
                }
                let piece = div().min_w_0().truncate().child(part.to_string());
                crumbs = crumbs.child(if i == last {
                    piece.text_color(theme.bright)
                } else {
                    piece.text_color(theme.secondary)
                });
            }
        } else if self.graph_visible {
            crumbs = crumbs.child(div().text_color(theme.bright).child("Graph"));
        }
        navigation = navigation.child(crumbs);
        if self.panes.paths.len() > 1 {
            navigation = navigation.child(
                text_button("next-document-pane", Some(IconName::ArrowRight), "Next pane", &theme)
                    .aria_label("Focus next document pane")
                    .text_sm()
                    .on_click(cx.listener(|this, _, w, cx| this.cycle_pane(w, cx))),
            );
        }
        navigation
            .child(
                icon_button(
                    "split-documents-across",
                    IconName::PanelRight,
                    "Split documents side by side",
                    false,
                    true,
                    &theme,
                )
                .on_click(cx.listener(|this, _, w, cx| this.split_pane(false, w, cx))),
            )
            .child(
                icon_button(
                    "split-documents-down",
                    IconName::PanelBottom,
                    "Split documents above and below",
                    false,
                    true,
                    &theme,
                )
                .on_click(cx.listener(|this, _, w, cx| this.split_pane(true, w, cx))),
            )
            .child(
                icon_button("reset-layout", IconName::RotateCw, "Reset pane widths", false, true, &theme)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.panes.sizes.clear();
                        this.document_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                        this.sidebar_width = 232.;
                        this.context_width = 280.;
                        this.layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                        this.context_layout = cx.new(|_| gpui_kit::base::ResizableState::default());
                        for saved in &mut this.restored {
                            saved.split_width = None;
                        }
                        for tab in &mut this.tabs {
                            tab.split_width = None;
                            tab.split = cx.new(|_| gpui_kit::base::ResizableState::default());
                        }
                        cx.notify();
                    })),
            )
            .child(rule(&theme))
            .child(
                icon_button(
                    "toggle-context",
                    if self.context { IconName::PanelRightClose } else { IconName::PanelRightOpen },
                    "Toggle document context",
                    self.context,
                    true,
                    &theme,
                )
                .on_click(cx.listener(|this, _, w, cx| {
                    this.context = !this.context;
                    this.refresh_context(false, w, cx);
                    cx.notify();
                })),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let action = |id: &'static str, glyph: IconName, label: &'static str, hint: String| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .flex()
                .items_center()
                .gap_3()
                .w(px(260.))
                .h(px(34.))
                .px_3()
                .rounded(px(8.))
                .cursor_pointer()
                .text_color(theme.foreground)
                .hover(|s| s.bg(theme.hover_fill()))
                .child(icon(glyph).size(px(15.)).text_color(theme.accent))
                .child(div().flex_1().child(label))
                .child(kbd(hint, &theme))
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(hex_chain(&theme))
            .child(
                div()
                    .mt_4()
                    .font_family(crate::theme::SERIF)
                    .text_size(px(46.))
                    .text_color(theme.bright)
                    .child("Lapis"),
            )
            .child(div().text_color(theme.accent).child("write  ·  link  ·  recall"))
            .child(
                div()
                    .mt_6()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        action("welcome-search", IconName::Search, "Find a note", format!("{MOD}P"))
                            .aria_label("Find in workspace")
                            .on_click(cx.listener(|this, _, w, cx| this.open_palette(w, cx))),
                    )
                    .child(
                        action("welcome-commands", IconName::Menu, "Commands", format!("{MOD}K")).on_click(
                            cx.listener(|this, _, w, cx| this.open_menu(super::menu::Kind::Commands, w, cx)),
                        ),
                    )
                    .child(
                        action("welcome-graph", IconName::Network, "Open the graph", format!("{MOD}⇧G"))
                            .on_click(cx.listener(|this, _, w, cx| this.toggle_graph(w, cx))),
                    ),
            )
            .child(div().mt_4().text_sm().text_color(theme.secondary).child("Or open a note from Files."))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let mode = if self.graph_visible {
            "GRAPH".into()
        } else {
            self.tabs.get(self.active).map_or_else(
                || "READY".to_string(),
                |t| {
                    if t.document.readonly { "READ ONLY".into() } else { t.vim.label() }
                },
            )
        };
        let status = self
            .tabs
            .get(self.active)
            .filter(|t| t.vim.prompt.is_some())
            .map(|t| t.vim.label())
            .unwrap_or_else(|| {
                self.opening.as_ref().map(|p| format!("Opening {p}…")).unwrap_or_else(|| self.status.clone())
            });
        // Word count for prose the size of a note; skipped for very large buffers.
        let words = self
            .tabs
            .get(self.active)
            .filter(|t| !self.graph_visible && t.pdf.is_none() && t.document.kind == FileKind::Markdown)
            .map(|t| t.editor.read(cx).value())
            .filter(|text| text.len() <= 256 * 1024)
            .map(|text| text.split_whitespace().count());
        let mut bar = div()
            .id("workspace-status")
            .role(Role::Status)
            .aria_label(format!("{mode}. {status}"))
            .flex()
            .flex_none()
            .items_center()
            .gap_3()
            .h(px(26.))
            .px_3()
            .bg(theme.inset)
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .child(
                div()
                    .flex_none()
                    .px_1p5()
                    .rounded(px(4.))
                    .bg(theme.accent.opacity(0.14))
                    .text_color(theme.accent)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(mode),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if self.error { theme.danger } else { theme.secondary })
                    .child(status),
            );
        if self.code_graph.is_empty() {
            // Empty code-graph overlay. No nodes, no dialog, no process.
            bar = bar.child(
                div()
                    .id("code-graph-overlay")
                    .role(Role::Status)
                    .aria_label(crate::PHANTOM_NOT_FOUND)
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .text_color(theme.secondary)
                    .child(div().size(px(6.)).rounded(px(3.)).bg(theme.secondary.opacity(0.5)))
                    .child(crate::PHANTOM_NOT_FOUND),
            );
        }
        if let Some(words) = words {
            bar = bar.child(
                div()
                    .flex_none()
                    .text_color(theme.secondary)
                    .child(format!("{words} {}", if words == 1 { "word" } else { "words" })),
            );
        }
        bar
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.omarchy().clone();
        let sidebar = self.render_sidebar(cx);
        let mut content = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .child(self.render_navigation(cx))
            .child(self.render_tabs(cx));
        if !self.code_graph.rows().is_empty() || self.code_graph.truncated() {
            // Already-capped rows, drawn as text. A row is not a path.
            let mut rows = div()
                .id("code-graph-rows")
                .role(Role::List)
                .aria_label("Code graph rows")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .max_h(px(240.))
                .overflow_y_scroll()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(theme.border)
                .text_color(theme.secondary);
            for (index, row) in self.code_graph.rows().iter().enumerate() {
                rows =
                    rows.child(div().id(("code-graph-row", index)).role(Role::ListItem).child(row.clone()));
            }
            if self.code_graph.truncated() {
                rows = rows.child(div().id("code-graph-truncated").child("truncated"));
            }
            content = content.child(rows);
        }
        if self.graph_visible {
            if let Some(graph) = &self.graph {
                content = content.child(div().flex_1().min_h_0().child(graph.clone()));
            }
        } else if !self.tabs.is_empty() || self.panes.paths.len() > 1 {
            content = content.child(self.document_panes(window, cx));
        } else {
            content = content.child(self.render_welcome(cx));
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
                            .size(px(self.context_width))
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
                        .size(px(self.sidebar_width))
                        .size_range(px(140.)..px(480.))
                        .child(sidebar),
                )
                .child(resizable_panel().child(center)),
        );
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
            .child(self.render_status(cx));
        if let Some(notice) = &self.session_notice {
            surface = surface.child(div().p_2().text_sm().text_color(theme.danger).child(notice.clone()));
        }
        if self.palette.is_some() {
            surface = surface.child(self.draw_palette(window, cx));
        }
        if self.menu.is_some() {
            surface = surface.child(self.draw_menu(window, cx));
        }
        if let Some(prompt) = &self.command {
            surface =
                surface.child(command::render(prompt, self.error.then(|| self.status.clone()), window, cx));
        }
        surface
    }
}
