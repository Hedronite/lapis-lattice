//! Commands use a real native input, so IME/clipboard cannot edit a hidden note.
use super::*;
use gpui_kit::base::input::InputState;
use gpui_kit::{InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, px};
use gpui_omarchy::ActiveTheme;

pub(super) struct Prompt {
    pub input: Entity<InputState>,
    pub kind: char,
}
impl Workspace {
    pub(super) fn open_command(&mut self, kind: char, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(if kind == '/' {
                "Find in this document"
            } else {
                "w · q · q! · wq"
            })
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.command = Some(Prompt { input, kind });
        cx.notify();
    }
    pub(super) fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command.is_some() || self.palette.is_some() {
            // Prompts deliberately reject multiline paste instead of silently joining lines.
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text())
                && text.contains(['\n', '\r'])
            {
                self.status = "This prompt accepts one line. Multiline paste was not inserted.".into();
                self.error = true;
                cx.stop_propagation();
                cx.notify();
            } else {
                cx.propagate();
            }
            return;
        }
        let Some(tab) = self.tabs.get_mut(self.active) else {
            cx.propagate();
            return;
        };
        if !tab.editor.read(cx).focus_handle(cx).is_focused(window) {
            cx.propagate();
            return;
        }
        cx.stop_propagation();
        if tab.document.readonly {
            self.status = "Read-only reference".into();
            self.error = true;
            cx.notify();
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.status = "Clipboard has no readable text".into();
            self.error = true;
            cx.notify();
            return;
        };
        match crate::vim::normalize_paste(&text) {
            Ok(text) => {
                let insert = tab.vim.mode == crate::vim::Mode::Insert;
                tab.vim.reset();
                if insert {
                    tab.vim.mode = crate::vim::Mode::Insert;
                }
                tab.editor.update(cx, |s, cx| s.replace(text, window, cx));
            }
            Err(error) => {
                self.status = error.into();
                self.error = true;
            }
        }
        cx.notify();
    }
}
pub(super) fn render(
    prompt: &Prompt,
    message: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let theme = cx.omarchy().clone();
    div()
        .id("vim-command")
        .role(gpui_kit::Role::Group)
        .aria_label(if prompt.kind == '/' { "Find in document" } else { "Vim command" })
        .absolute()
        .bottom_0()
        .left_0()
        .right_0()
        .p_3()
        .bg(theme.background)
        .border_t_1()
        .border_color(theme.accent)
        .flex()
        .items_center()
        .gap_3()
        .child(prompt.kind.to_string())
        .child(div().flex_1().child(gpui_omarchy::input("vim-command-input", &prompt.input, window, cx)))
        .child(
            div()
                .text_size(px(12.))
                .text_color(if message.is_some() { theme.danger } else { theme.secondary })
                .child(message.unwrap_or_else(|| "Enter · execute   Esc · cancel".into())),
        )
}
