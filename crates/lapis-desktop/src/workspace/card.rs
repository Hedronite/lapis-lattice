//! The properties card above Reading view, matching the one Live paints on its canvas.
use super::*;
use crate::live::properties::{self, Piece};
use gpui_kit::{InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
use gpui_omarchy::ActiveTheme;

impl Workspace {
    pub(super) fn reading_card(&self, index: usize, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        let tab = self.tabs.get(index)?;
        let all = properties::rows(&tab.document.properties);
        if all.is_empty() {
            return None;
        }
        let theme = cx.omarchy().clone();
        let open = tab.live.read(cx).card_open();
        // Weak: a rendered listener must not keep the editor alive.
        let live = tab.live.downgrade();
        let mut header = div()
            .flex()
            .items_center()
            .text_xs()
            .text_color(theme.secondary)
            .child(div().flex_1().child(format!("PROPERTIES · {}", all.len())));
        if all.len() > properties::SUMMARY {
            header = header.child(
                div()
                    .id(("card-toggle", index))
                    .cursor_pointer()
                    .text_color(theme.accent)
                    .hover(|s| s.text_color(theme.bright))
                    .child(if open { "Show less".to_string() } else { format!("Show all {}", all.len()) })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = live.update(cx, |s, cx| s.toggle_card(cx));
                    })),
            );
        }
        let shown = if open { all.len() } else { all.len().min(properties::SUMMARY) };
        let mut card = div()
            .mb_6()
            .p_3p5()
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.inset)
            .flex()
            .flex_col()
            .gap_1()
            .text_sm()
            .child(header.mb_1());
        for (r, row) in all.into_iter().take(shown).enumerate() {
            let mut value = div().flex_1().min_w_0().flex().flex_wrap().text_color(theme.foreground);
            for (i, piece) in row.pieces.into_iter().enumerate() {
                value = match piece {
                    Piece::Text(text) => value.child(text),
                    Piece::Link { label, target } => value.child(
                        div()
                            .id(("card-link", r * 1000 + i))
                            .cursor_pointer()
                            .text_color(theme.accent)
                            .underline()
                            .hover(|s| s.text_color(theme.bright))
                            .child(label)
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.follow_link(target.clone(), true, w, cx)
                            })),
                    ),
                };
            }
            card = card.child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(128.))
                            .flex_none()
                            .text_xs()
                            .pt_0p5()
                            .text_color(theme.secondary)
                            .child(row.key),
                    )
                    .child(value),
            );
        }
        Some(card.into_any_element())
    }
}
