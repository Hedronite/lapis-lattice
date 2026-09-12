//! One editor entity per file, shared by tabs, panes and history.
use super::*;
impl Workspace {
    pub(super) fn install_document(
        &mut self,
        document: Document,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if let Some(index) = self.tabs.iter().position(|t| t.document.path == document.path) {
            return index;
        }
        self.pane_errors.remove(&document.path);
        let restored = self.restored.iter().find(|t| t.path == document.path).cloned();
        let editor = cx.new(|cx| {
            let mut input = TextareaState::new(window, cx).default_value(document.text.clone()).rows(30);
            input.set_readonly(document.readonly, cx);
            input
        });
        let events = cx.subscribe(&editor, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut view = match document.kind {
            FileKind::Markdown => View::Live,
            FileKind::Html | FileKind::Pdf => View::Reading,
            _ => View::Source,
        };
        let pdf = if document.kind == FileKind::Pdf {
            Some(cx.new(|cx| {
                crate::pdf_reader::PdfReader::new(document.path.clone(), self.services.clone(), window, cx)
            }))
        } else {
            None
        };
        let reading_scroll = gpui_kit::ScrollHandle::new();
        if let Some(saved) = &restored {
            view = match document.kind {
                FileKind::Pdf | FileKind::Html => View::Reading,
                FileKind::Markdown => saved.view,
                _ if saved.view == View::Live => View::Source,
                _ => saved.view,
            };
            editor.update(cx, |s, cx| {
                s.set_selected_range(saved.selection[0]..saved.selection[1], cx);
                s.set_scroll_offset(
                    gpui_kit::point(
                        gpui_kit::px(saved.source_scroll[0]),
                        gpui_kit::px(saved.source_scroll[1]),
                    ),
                    cx,
                );
            });
            reading_scroll.set_offset(gpui_kit::point(
                gpui_kit::px(saved.reading_scroll[0]),
                gpui_kit::px(saved.reading_scroll[1]),
            ));
            if let Some(pdf) = &pdf {
                pdf.update(cx, |s, cx| s.restore_position(saved.pdf_page, saved.pdf_zoom, window, cx));
            }
        }
        let live = cx.new(|cx| crate::live::LiveEditor::new(editor.clone(), cx));
        editor.update(cx, |s, cx| s.set_cursor_blink(view != View::Live, cx));
        if let Some(saved) = &restored {
            live.update(cx, |s, cx| s.restore_scroll(saved.live_scroll, cx));
        }
        if !self.tab_order.contains(&document.path) {
            self.tab_order.push(document.path.clone());
        }
        self.restored.retain(|t| t.path != document.path);
        self.tabs.push(Tab {
            document,
            editor,
            live,
            pdf,
            _events: events,
            split_width: restored.and_then(|t| t.split_width),
            reading_scroll,
            view,
            split: cx.new(|_| gpui_kit::base::ResizableState::default()),
            vim: crate::vim::Vim::default(),
            close_after_save: false,
            saving: false,
        });
        self.tabs.len() - 1
    }

    /// Switch a tab's view. Only Live hides the shared source widget behind its own
    /// steady caret, so only Live keeps the widget blink off; visible source blinks.
    pub(super) fn set_view(&mut self, index: usize, view: View, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[index];
        tab.view = view;
        tab.editor.update(cx, |s, cx| s.set_cursor_blink(view != View::Live, cx));
    }
}
