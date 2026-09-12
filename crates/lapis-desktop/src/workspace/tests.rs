//! Headless GPUI dispatch tests complement, but never replace, native host smoke.
use super::*;
use crate::services::SearchPage;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext, WindowHandle};

struct Fixture {
    reject_save: bool,
    indexed: std::sync::atomic::AtomicBool,
}
impl WorkspaceServices for Fixture {
    fn directory(&self, _: &str) -> Result<Vec<FileEntry>, String> {
        Ok(vec![])
    }
    fn read(&self, path: &str) -> Result<Document, String> {
        if path == "missing.md" {
            return Err("Fixture missing file".into());
        }
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
    fn links(&self, path: &str) -> Result<Vec<crate::services::ContextLink>, String> {
        Ok(vec![crate::services::ContextLink {
            path: Some("related.md".into()),
            label: if self.indexed.load(std::sync::atomic::Ordering::SeqCst) {
                format!("Updated links to {path}")
            } else {
                format!("Related to {path}")
            },
            direction: "out".into(),
        }])
    }
    fn reindex(&self, _: &str) -> Result<(), String> {
        self.indexed.store(true, std::sync::atomic::Ordering::SeqCst);
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
        init_workspace(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let handle = cx.add_window(move |w, cx| {
        Workspace::new(Arc::new(Fixture { reject_save, indexed: false.into() }), w, cx)
    });
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

fn replace_fixture(handle: WindowHandle<Workspace>, source: &str, cx: &mut TestAppContext) {
    handle
        .update(cx, |this, w, cx| {
            assert!(this.tabs[0].view == View::Live);
            this.tabs[0].editor.update(cx, |s, cx| {
                s.set_value(source, w, cx);
                s.set_selected_range(0..0, cx);
            });
            cx.notify();
        })
        .unwrap();
}
#[gpui_kit::test]
fn live_mouse_selects_formatted_unicode_and_copies_source(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let source = "# Title\n\nA **漢🙂** and `code`.\n\nOther note.\n";
    replace_fixture(handle, source, cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    let start = source.find('漢').unwrap();
    let end = start + "漢🙂".len();
    let (from, to) = handle
        .read_with(cx, |this, cx| {
            let live = this.tabs[0].live.read(cx);
            let from = live.point_for_source(start).expect("formatted glyph has geometry");
            let to = live.point_for_source(end).expect("formatted glyph end has geometry");
            assert_eq!(live.source_at_point(from), start);
            (
                from + gpui_kit::point(gpui_kit::px(1.), gpui_kit::px(8.)),
                to + gpui_kit::point(gpui_kit::px(-1.), gpui_kit::px(8.)),
            )
        })
        .unwrap();
    visual.update(|w, cx| {
        w.drag(from, to, cx);
        w.dispatch_action(Box::new(gpui_kit::base::input::Copy), cx);
    });
    assert_eq!(
        handle.read_with(cx, |this, cx| this.tabs[0].editor.read(cx).selected_range()).unwrap(),
        start..end
    );
    assert_eq!(cx.update(|cx| cx.read_from_clipboard().and_then(|i| i.text())).as_deref(), Some("漢🙂"));
    visual.update(|w, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("first\r\n  second".into()));
        w.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx);
        w.press("u", cx);
    });
    assert_eq!(text(handle, cx), source);
}
#[gpui_kit::test]
fn live_empty_note_has_caret_geometry_and_accepts_native_text(cx: &mut TestAppContext) {
    let handle = setup(cx);
    replace_fixture(handle, "", cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    assert!(
        handle.read_with(cx, |this, cx| this.tabs[0].live.read(cx).point_for_source(0).is_some()).unwrap()
    );
    visual.update(|w, cx| {
        w.press("i", cx);
        w.input("# 新しい\n\nText", cx);
        w.press("escape", cx);
    });
    assert_eq!(text(handle, cx), "# 新しい\n\nText");
}
#[gpui_kit::test]
fn live_ime_uses_source_utf16_and_keeps_composition(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler;
    let handle = setup(cx);
    replace_fixture(handle, "🙂 alpha\n", cx);
    handle
        .update(cx, |this, w, cx| {
            this.tabs[0].live.update(cx, |live, cx| {
                assert_eq!(live.text_length_utf16(w, cx), Some(9));
                live.set_selected_text_range(3..8, w, cx);
                assert_eq!(live.selected_text_range(false, w, cx).unwrap().range, 3..8);
                live.replace_and_mark_text_in_range(None, "か", Some(1..1), w, cx);
                assert_eq!(live.marked_text_range(w, cx), Some(3..4));
                live.replace_and_mark_text_in_range(None, "漢字", Some(2..2), w, cx);
                live.unmark_text(w, cx);
                assert!(live.marked_text_range(w, cx).is_none());
            });
        })
        .unwrap();
    assert_eq!(text(handle, cx), "🙂 漢字\n");
}
#[gpui_kit::test]
fn live_cursor_follows_long_note_without_changing_text(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let source =
        (0..120).map(|i| format!("## Section {i}\n\nParagraph **bold** content.\n\n")).collect::<String>();
    replace_fixture(handle, &source, cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.press("G", cx);
        w.render_frame(cx);
    });
    let position = handle
        .read_with(cx, |this, cx| {
            let t = &this.tabs[0];
            t.live.read(cx).point_for_source(t.editor.read(cx).cursor()).unwrap()
        })
        .unwrap();
    visual.update(|w, _| {
        assert!(position.y > gpui_kit::px(80.));
        assert!(position.y < w.viewport_size().height - gpui_kit::px(30.));
    });
    assert_eq!(text(handle, cx), source);
}

#[gpui_kit::test]
fn live_platform_paste_normalizes_lines_and_respects_readonly(cx: &mut TestAppContext) {
    use gpui_kit::EntityInputHandler;
    let handle = setup(cx);
    for readonly in [true, false] {
        handle
            .update(cx, |this, w, cx| {
                let tab = &this.tabs[0];
                tab.editor.update(cx, |s, cx| s.set_readonly(readonly, cx));
                tab.live.update(cx, |live, cx| {
                    assert_eq!(live.accepts_text_input(w, cx), !readonly);
                    live.paste(ClipboardItem::new_string("A\r\n  B\rC\n".into()), w, cx);
                });
            })
            .unwrap();
        assert_eq!(
            text(handle, cx),
            if readonly { "one two\nsecond\n" } else { "A\n  B\nC\none two\nsecond\n" }
        );
    }
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.press("u", cx));
    assert_eq!(text(handle, cx), "one two\nsecond\n");
}

#[gpui_kit::test]
fn navigation_reuses_dirty_buffers_and_failed_open_does_not_move_history(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle
        .update(cx, |this, w, cx| {
            this.tabs[0].editor.update(cx, |s, cx| s.replace("unsaved ", w, cx));
            this.open_file("second.md".into(), w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.press(if cfg!(target_os = "macos") { "cmd-alt-left" } else { "alt-left" }, cx);
    });
    cx.run_until_parked();
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.active, 0);
            assert!(Workspace::dirty(&this.tabs[this.active], cx));
            assert!(this.tabs[0].editor.read(cx).value().starts_with("unsaved "));
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.open_file("missing.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.tabs[this.active].document.path, "fixture.md");
            assert_eq!(this.history.target(true).unwrap().1, "second.md");
        })
        .unwrap();
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.press(if cfg!(target_os = "macos") { "cmd-alt-right" } else { "alt-right" }, cx);
    });
    cx.run_until_parked();
    assert_eq!(
        handle.read_with(cx, |this, _| this.tabs[this.active].document.path.clone()).unwrap(),
        "second.md"
    );
    handle
        .update(cx, |this, w, cx| {
            this.close_tab(false, cx);
            this.focus_active(w, cx);
        })
        .unwrap();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.tabs[this.active].document.path, "fixture.md");
            assert_eq!(this.history.target(false).unwrap().1, "second.md");
        })
        .unwrap();
}

#[gpui_kit::test]
fn context_follows_active_file_and_keeps_tree_capability_error_visible(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle
        .update(cx, |this, w, cx| {
            this.context = true;
            this.refresh_context(false, w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.context_state.path.as_deref(), Some("fixture.md"));
            assert!(
                this.context_state.links.as_ref().unwrap().as_ref().unwrap()[0].label.contains("fixture.md")
            );
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.context_tree(w, cx)).unwrap();
    cx.run_until_parked();
    assert!(handle.read_with(cx, |this, _| this.context_state.tree.as_ref().unwrap().is_err()).unwrap());
    handle.update(cx, |this, w, cx| this.open_file("related.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.context_state.path.as_deref(), Some("related.md"));
            assert!(this.context_state.tree.is_none());
        })
        .unwrap();
}

#[gpui_kit::test]
fn save_refreshes_indexed_context_without_reopening_editor(cx: &mut TestAppContext) {
    for reject_save in [false, true] {
        let handle = setup_with(cx, reject_save);
        handle
            .update(cx, |this, w, cx| {
                this.context = true;
                this.refresh_context(false, w, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        visual.update(|w, cx| {
            w.press("i", cx);
            w.input("edited ", cx);
            w.press("escape", cx);
        });
        handle.update(cx, |this, w, cx| this.save(w, cx)).unwrap();
        cx.run_until_parked();
        handle
            .read_with(cx, |this, cx| {
                let label = &this.context_state.links.as_ref().unwrap().as_ref().unwrap()[0].label;
                assert_eq!(label.starts_with("Updated links"), !reject_save);
                assert_eq!(Workspace::dirty(&this.tabs[0], cx), reject_save);
                assert!(this.tabs[0].editor.read(cx).value().starts_with("edited "));
            })
            .unwrap();
        visual.update(|w, cx| w.press("u", cx));
        assert_eq!(text(handle, cx), "one two\nsecond\n");
    }
}

#[gpui_kit::test]
fn resizing_sidebar_and_source_split_preserves_editor_and_selection(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle
        .update(cx, |this, _, cx| {
            this.tabs[0].view = View::Split;
            this.tabs[0].editor.update(cx, |s, cx| s.set_selected_range(1..4, cx));
            cx.notify();
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.render_frame(cx);
    });
    handle
        .update(cx, |this, w, cx| {
            this.layout.update(cx, |s, cx| s.resize_panel(0, gpui_kit::px(300.), w, cx));
        })
        .unwrap();
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.render_frame(cx);
    });
    handle
        .update(cx, |this, w, cx| {
            this.tabs[0].split.update(cx, |s, cx| s.resize_panel(0, gpui_kit::px(450.), w, cx));
        })
        .unwrap();
    visual.update(|w, cx| w.render_frame(cx));
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.layout.read(cx).sizes()[0], gpui_kit::px(300.));
            assert_eq!(this.tabs[0].split.read(cx).sizes()[0], gpui_kit::px(450.));
            assert_eq!(this.tabs[0].editor.read(cx).selected_range(), 1..4);
            assert_eq!(this.tabs[0].editor.read(cx).value().as_ref(), "one two\nsecond\n");
        })
        .unwrap();
}

struct SessionFixture {
    session: crate::session::Session,
    reads: std::sync::Mutex<Vec<String>>,
}
impl WorkspaceServices for SessionFixture {
    fn directory(&self, _: &str) -> Result<Vec<FileEntry>, String> {
        Ok(vec![])
    }
    fn load_session(&self) -> Result<Option<crate::session::Session>, String> {
        Ok(Some(self.session.clone()))
    }
    fn read(&self, path: &str) -> Result<Document, String> {
        self.reads.lock().unwrap().push(path.into());
        Fixture { reject_save: false, indexed: false.into() }.read(path)
    }
    fn save(&self, _: &Document, _: &str) -> Result<Document, String> {
        Err("unused fixture save".into())
    }
    fn save_copy(&self, d: &Document, text: &str) -> Result<Document, String> {
        self.save(d, text)
    }
    fn search(&self, _: &str) -> Result<SearchPage, String> {
        Err("unused search".into())
    }
    fn reindex(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn build_index(&self) -> Result<u64, String> {
        Ok(0)
    }
}
#[gpui_kit::test]
fn session_restore_is_lazy_clamps_changed_text_and_preserves_tab_order(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        init_workspace(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let mut first = crate::session::Tab::new("first.md".into());
    first.view = View::Split;
    first.selection = [100, 100];
    first.split_width = Some(350.);
    let service = Arc::new(SessionFixture {
        session: crate::session::Session {
            tabs: vec![
                first,
                crate::session::Tab::new("second.md".into()),
                crate::session::Tab::new("missing.md".into()),
            ],
            active: Some("first.md".into()),
            sidebar_width: 300.,
            context_width: 240.,
            context: true,
            ..Default::default()
        },
        reads: Default::default(),
    });
    let services = service.clone();
    let handle = cx.add_window(move |w, cx| Workspace::new(services, w, cx));
    handle.update(cx, |this, w, cx| this.restore_session(None, w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(*service.reads.lock().unwrap(), vec!["first.md"]);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.render_frame(cx);
    });
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.layout.read(cx).sizes()[0], gpui_kit::px(300.));
            assert_eq!(this.context_layout.read(cx).sizes()[1], gpui_kit::px(240.));
            assert_eq!(this.tabs[0].split.read(cx).sizes()[0], gpui_kit::px(350.));
        })
        .unwrap();

    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.tabs.len(), 1);
            assert!(this.tabs[0].view == View::Split);
            assert_eq!(this.tabs[0].editor.read(cx).selected_range(), 15..15);
            assert_eq!(this.sidebar_width, 300.);
            assert!(this.context);
            assert_eq!(
                this.session_snapshot(cx).tabs.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(),
                vec!["first.md", "second.md", "missing.md"]
            );
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.open_file("second.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(*service.reads.lock().unwrap(), vec!["first.md", "second.md"]);
    handle.update(cx, |this, w, cx| this.open_file("missing.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.tabs[this.active].document.path, "second.md");
            assert!(this.error);
            assert!(this.tab_order.contains(&"missing.md".into()));
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.close_path("missing.md", w, cx)).unwrap();
    assert!(!handle.read_with(cx, |this, _| this.error).unwrap());
    handle.update(cx, |this, w, cx| this.close_path("first.md", w, cx)).unwrap();
    assert_eq!(
        handle.read_with(cx, |this, _| this.tabs[this.active].document.path.clone()).unwrap(),
        "second.md"
    );

    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.session_snapshot(cx).tabs.len(), 1);
            assert_eq!(this.session_snapshot(cx).active.as_deref(), Some("second.md"));
        })
        .unwrap();
}
#[gpui_kit::test]
fn closing_loaded_tab_can_activate_lazy_neighbor_and_dirty_tabs_stay(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle
        .update(cx, |this, w, cx| {
            this.tab_order.push("neighbor.md".into());
            this.restored.push(crate::session::Tab::new("neighbor.md".into()));
            this.tabs[0].editor.update(cx, |s, cx| s.replace("dirty ", w, cx));
            this.close_path("fixture.md", w, cx);
            assert_eq!(this.tabs.len(), 1);
            assert_eq!(this.tab_order.len(), 2);
            this.close_tab(true, cx);
            this.focus_active(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.tabs.len(), 1);
            assert_eq!(this.tabs[0].document.path, "neighbor.md");
            assert_eq!(this.session_snapshot(cx).tabs.len(), 1);
        })
        .unwrap();
}
