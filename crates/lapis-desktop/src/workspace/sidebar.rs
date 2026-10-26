//! The sidebar list: Recent notes, then the folder tree with indent guides.
use super::chrome::{icon_button, section};
use super::*;
use gpui_kit::{
    InteractiveElement, IntoElement, ParentElement, Role, SharedString, StatefulInteractiveElement, Styled,
    div, px,
};
use gpui_omarchy::{ActiveTheme, IconName, icon};

const INDENT: f32 = 14.;

impl Workspace {
    /// Children of the scroll list come before the tree rows: the Recent header and
    /// rows when shown, then the Files header. Reveal scrolls by child index.
    fn tree_offset(&self) -> usize {
        if self.recent.is_empty() { 1 } else { self.recent.len() + 2 }
    }

    /// Bring a pending reveal into view once its row exists.
    pub(super) fn apply_reveal(&mut self) {
        let Some(path) = self.reveal.clone() else { return };
        if let Some(index) = self.tree.rows().iter().position(|r| r.entry.path == path) {
            self.file_scroll.scroll_to_item(self.tree_offset() + index);
            self.reveal = None;
        } else if !crate::file_tree::ancestors(&path).iter().any(|f| self.tree.is_loading(f))
            && !self.tree.is_loading("")
        {
            // Everything above it loaded and it is still absent: nothing to reveal.
            self.reveal = None;
        }
    }

    pub(super) fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.omarchy().clone();
        let active = self.tabs.get(self.active).map(|t| t.document.path.clone());
        let mut list = div()
            .id("file-list")
            .role(Role::List)
            .aria_label("Workspace files")
            .track_scroll(&self.file_scroll)
            .flex_1()
            // Without a zero minimum the list grows past the sidebar and never scrolls.
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            .gap_px();
        let header = |label: &'static str| {
            div().flex().flex_none().items_center().h(px(26.)).px_2().pt_2().child(section(label, &theme))
        };
        if !self.recent.is_empty() {
            list = list.child(header("RECENT"));
            for (i, path) in self.recent.iter().enumerate() {
                let name = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md").to_string();
                let open = path.clone();
                let mut row = row_base(SharedString::from(format!("recent-{i}")), &theme)
                    .aria_label(format!("Open recent {path}"))
                    .pl_2()
                    .child(icon(IconName::FileText).size(px(13.)).text_color(theme.secondary))
                    .child(div().flex_1().min_w_0().truncate().child(name));
                row = mark(row, active.as_deref() == Some(path.as_str()), &theme);
                list = list.child(
                    row.on_click(cx.listener(move |this, _, w, cx| this.open_file(open.clone(), w, cx))),
                );
            }
        }
        list = list.child(
            header("FILES").child(div().flex_1()).child(
                icon_button(
                    "collapse-folders",
                    IconName::ChevronsUpDown,
                    "Collapse all folders",
                    false,
                    true,
                    &theme,
                )
                .size(px(22.))
                .on_click(cx.listener(|this, _, _, cx| this.collapse_tree(cx))),
            ),
        );
        let rows = self.tree.rows();
        for row in &rows {
            let entry = row.entry;
            let path = entry.path.clone();
            let directory = entry.directory;
            let mut item = row_base(SharedString::from(format!("file-{path}")), &theme)
                .accessibility_id(format!("workspace.file.{path}"))
                .aria_label(format!("Open {} {}", if directory { "folder" } else { "file" }, entry.name))
                .pl(px(6. + row.depth as f32 * INDENT));
            // A hairline for each level above, so nesting reads at a glance.
            for level in 0..row.depth {
                item = item.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(12. + level as f32 * INDENT))
                        .w(px(1.))
                        .bg(theme.border),
                );
            }
            item = if directory {
                let chevron = if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight };
                let folder = if row.expanded { IconName::FolderOpen } else { IconName::Folder };
                item.child(icon(chevron).size(px(12.)).text_color(theme.secondary))
                    .child(icon(folder).size(px(14.)).text_color(theme.accent.opacity(0.75)))
                    .text_color(if self.folder == path { theme.bright } else { theme.foreground })
            } else {
                item.child(div().w(px(12.)).flex_none())
                    .child(icon(IconName::FileText).size(px(14.)).text_color(theme.secondary))
            };
            item = item.child(div().flex_1().min_w_0().truncate().child(entry.name.clone()));
            if row.loading {
                item = item.child(icon(IconName::LoaderCircle).size(px(12.)).text_color(theme.secondary));
            }
            item = mark(item, !directory && active.as_deref() == Some(path.as_str()), &theme);
            list = list.child(item.on_click(cx.listener(move |this, _, w, cx| {
                if directory {
                    this.toggle_folder(path.clone(), w, cx)
                } else {
                    this.open_file(path.clone(), w, cx)
                }
            })));
        }
        if rows.is_empty() {
            let note = if let Some(error) = self.tree.error("") {
                error.to_string()
            } else if self.tree.is_loading("") {
                "Loading files…".into()
            } else {
                "This vault is empty".into()
            };
            list = list.child(div().p_2().text_sm().text_color(theme.secondary).child(note));
        }
        list
    }
}

fn row_base(id: SharedString, theme: &gpui_omarchy::Theme) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .role(Role::Button)
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .gap_1p5()
        .h(px(26.))
        .pr_2()
        .rounded(px(6.))
        .cursor_pointer()
        .text_color(theme.foreground)
}

/// The open note: a lapis-blue wash with a gold edge. Everything else gets a hover wash.
fn mark(
    row: gpui_kit::Stateful<gpui_kit::Div>,
    active: bool,
    theme: &gpui_omarchy::Theme,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    if active {
        row.bg(crate::theme::lapis_blue().opacity(0.55)).text_color(theme.bright).child(
            div().absolute().left_0().top(px(5.)).bottom(px(5.)).w(px(2.)).rounded(px(1.)).bg(theme.accent),
        )
    } else {
        row.hover(|s| s.bg(theme.hover_fill()))
    }
}
