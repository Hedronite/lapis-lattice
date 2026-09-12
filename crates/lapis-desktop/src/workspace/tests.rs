//! Headless GPUI dispatch tests complement, but never replace, native host smoke.
use super::*;
use crate::services::SearchPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext, WindowHandle};

struct Fixture {
    reject_save: bool,
}
impl WorkspaceServices for Fixture {
    fn directory(&self, _: &str) -> Result<Vec<FileEntry>, String> {
        Ok(vec![])
    }
    fn read(&self, path: &str) -> Result<Document, String> {
        Ok(Document {
            path: path.into(),
            title: "Fixture".into(),
            kind: FileKind::Markdown,
            text: "one two\nsecond\n".into(),
            original: "one two\nsecond\n".into(),
            properties: serde_json::json!({"custom":"keep"}),
            readonly: false,
        })
    }
    fn save(&self, d: &Document, text: &str) -> Result<Document, String> {
        if self.reject_save {
            return Err("Fixture save rejected".into());
        }
        let mut d = d.clone();
        d.text = text.into();
        d.original = text.into();
        Ok(d)
    }
    fn save_copy(&self, d: &Document, text: &str) -> Result<Document, String> {
        self.save(d, text)
    }
    fn search(&self, _: &str) -> Result<SearchPage, String> {
        Err("offline fixture".into())
    }
    fn reindex(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn build_index(&self) -> Result<u64, String> {
        Ok(1)
    }
}
fn setup(cx: &mut TestAppContext) -> WindowHandle<Workspace> {
    setup_with(cx, false)
}
fn setup_with(cx: &mut TestAppContext, reject_save: bool) -> WindowHandle<Workspace> {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let handle = cx.add_window(move |w, cx| Workspace::new(Arc::new(Fixture { reject_save }), w, cx));
    handle.update(cx, |this, w, cx| this.open_file("fixture.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(handle.read_with(cx, |this, _| this.tabs.len()).unwrap(), 1);
    handle
}
fn text(handle: WindowHandle<Workspace>, cx: &TestAppContext) -> String {
    handle.read_with(cx, |this, cx| this.tabs[0].editor.read(cx).value().to_string()).unwrap()
}
#[gpui_kit::test]
fn source_operator_and_native_undo_share_one_history(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.press("d", cx);
        w.press("w", cx);
    });
    assert_eq!(text(handle, cx), "two\nsecond\n");
    visual.update(|w, cx| w.press("u", cx));
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    visual.update(|w, cx| w.press("ctrl-r", cx));
    assert_eq!(text(handle, cx), "two\nsecond\n");
}
#[gpui_kit::test]
fn crlf_paste_is_literal_atomic_and_preserves_mode(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("# Paste\r\n\r\n  漢🙂\t:q!\r\n".into()));
        w.render_frame(cx);
        w.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx);
        w.render_frame(cx);
    });
    assert_eq!(text(handle, cx), "# Paste\n\n  漢🙂\t:q!\none two\nsecond\n");
    assert_eq!(handle.read_with(cx, |this, _| this.tabs[0].vim.mode).unwrap(), crate::vim::Mode::Normal);
    visual.update(|w, cx| w.press("u", cx));
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    visual.update(|w, cx| w.press("ctrl-r", cx));
    assert!(text(handle, cx).starts_with("# Paste\n\n"));
}
#[gpui_kit::test]
fn command_input_owns_ime_and_rejects_multiline_paste(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.press(":", cx);
        w.input("w", cx);
    });
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    assert_eq!(
        handle
            .read_with(cx, |this, cx| this.command.as_ref().unwrap().input.read(cx).value().to_string())
            .unwrap(),
        "w"
    );
    visual.update(|w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("q!\nother".into()));
        w.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx);
        w.render_frame(cx);
    });
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    assert!(handle.read_with(cx, |this, _| this.error).unwrap());
    visual.update(|w, cx| w.press("escape", cx));
    assert!(handle.read_with(cx, |this, _| this.command.is_none()).unwrap());
    visual.update(|w, cx| {
        w.press("i", cx);
        w.input("漢🙂", cx);
        w.press("escape", cx);
    });
    assert!(text(handle, cx).starts_with("漢🙂one two"));
}

#[gpui_kit::test]
fn save_and_close_waits_for_success_and_retains_failed_buffer(cx: &mut TestAppContext) {
    for reject in [false, true] {
        let handle = setup_with(cx, reject);
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        visual.update(|w, cx| {
            w.press("i", cx);
            w.input("changed ", cx);
            w.press("escape", cx);
            w.press(":", cx);
            w.input("wq", cx);
            w.press("enter", cx);
        });
        cx.run_until_parked();
        assert_eq!(handle.read_with(cx, |this, _| this.tabs.len()).unwrap(), usize::from(reject));
        if reject {
            assert!(text(handle, cx).starts_with("changed one"));
            assert!(handle.read_with(cx, |this, _| this.error).unwrap());
        }
    }
}
#[gpui_kit::test]
fn reading_source_switch_preserves_native_undo_and_focus(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.press("d", cx);
        w.press("w", cx);
    });
    handle
        .update(cx, |this, w, cx| {
            this.tabs[0].view = View::Reading;
            this.focus_active(w, cx);
            cx.notify();
        })
        .unwrap();
    visual.update(|w, cx| w.render_frame(cx));
    handle
        .update(cx, |this, w, cx| {
            this.tabs[0].view = View::Source;
            this.focus_active(w, cx);
            cx.notify();
        })
        .unwrap();
    visual.update(|w, cx| w.press("u", cx));
    assert_eq!(text(handle, cx), "one two\nsecond\n");
}

#[gpui_kit::test]
fn large_native_widget_paste_round_trips_in_one_undo(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let payload = "line with  spaces\t漢🙂\n".repeat(45_000);
    assert!(payload.len() >= 1024 * 1024);
    let expected = format!("{payload}one two\nsecond\n");
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    let started = std::time::Instant::now();
    visual.update(|w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string(payload));
        w.render_frame(cx);
        w.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx);
        w.render_frame(cx);
    });
    eprintln!(
        "headless GPUI paste+frame: {:.3} ms (not native latency)",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(text(handle, cx), expected);
    visual.update(|w, cx| w.press("u", cx));
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    visual.update(|w, cx| w.press("ctrl-r", cx));
    assert_eq!(text(handle, cx), expected);
}
