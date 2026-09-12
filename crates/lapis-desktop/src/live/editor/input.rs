//! IME uses full-source UTF-16 ranges, with bounds resolved through the live projection.
use super::*;
use gpui_kit::{EntityInputHandler, TextInputConfiguration, UTF16Selection};
impl EntityInputHandler for LiveEditor {
    fn text_for_range(
        &mut self,
        r: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.source.update(cx, |s, cx| s.text_for_range(r, adjusted, w, cx))
    }
    fn selected_text_range(
        &mut self,
        ignore: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.source.update(cx, |s, cx| s.selected_text_range(ignore, w, cx))
    }
    fn marked_text_range(&self, w: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.source.update(cx, |s, cx| s.marked_text_range(w, cx))
    }
    fn unmark_text(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.source.update(cx, |s, cx| s.unmark_text(w, cx));
    }
    fn paste(&mut self, item: gpui_kit::ClipboardItem, w: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = item.text() else {
            return;
        };
        match crate::vim::normalize_paste(&text) {
            Ok(text) => {
                self.input_error = None;
                self.replace_text_in_range(None, &text, w, cx);
            }
            Err(error) => self.input_error = Some(error.into()),
        }
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source.update(cx, |s, cx| s.replace_text_in_range(r, text, w, cx));
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        r: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source.update(cx, |s, cx| s.replace_and_mark_text_in_range(r, text, selected, w, cx));
    }
    fn bounds_for_range(
        &mut self,
        r: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let byte = byte_for_utf16(&self.text, r.start);
        self.point_for_source(byte).map(|p| Bounds::new(p, gpui_kit::size(px(2.), px(24.))))
    }
    fn character_index_for_point(
        &mut self,
        p: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.text[..self.source_at_point(p)].encode_utf16().count())
    }
    fn set_selected_text_range(&mut self, r: Range<usize>, _: &mut Window, cx: &mut Context<Self>) {
        self.source.update(cx, |s, cx| {
            let text = s.value();
            s.set_selected_range(byte_for_utf16(&text, r.start)..byte_for_utf16(&text, r.end), cx);
        });
    }
    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        Some(self.source.read(cx).value().encode_utf16().count())
    }
    fn accepts_text_input(&self, _: &mut Window, cx: &mut Context<Self>) -> bool {
        self.source.read(cx).is_editable()
    }
    fn text_input_configuration(&mut self, w: &mut Window, cx: &mut Context<Self>) -> TextInputConfiguration {
        self.source.update(cx, |s, cx| s.text_input_configuration(w, cx))
    }
    fn text_input_editable_range(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<Range<usize>> {
        (self.source.read(cx).is_editable()).then(|| 0..self.source.read(cx).value().encode_utf16().count())
    }
}

fn byte_for_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units + ch.len_utf16() > offset {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}
