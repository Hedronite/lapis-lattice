use super::*;
#[gpui_kit::test]
fn mouse_focus_routes_edits_to_distinct_panes_and_closing_view_retains_dirty_tab(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle
        .update(cx, |this, w, cx| {
            this.split_pane(false, w, cx);
            assert!(this.tabs.get(this.active).is_none());
            this.open_file("second.md".into(), w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.render_frame(cx);
    });
    let (left, right) = handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.panes.paths, vec![Some("fixture.md".into()), Some("second.md".into())]);
            (
                this.tabs[0].live.read(cx).point_for_source(0).unwrap(),
                this.tabs[1].live.read(cx).point_for_source(0).unwrap(),
            )
        })
        .unwrap();
    assert!(right.x > left.x + gpui_kit::px(100.));
    visual.update(|w, cx| {
        w.drag(left, left, cx);
        w.press("i", cx);
        w.input("left ", cx);
        w.press("escape", cx);
    });
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.active, 0);
            assert!(this.tabs[0].editor.read(cx).value().starts_with("left "));
            assert_eq!(this.tabs[1].editor.read(cx).value().as_ref(), "one two\nsecond\n");
        })
        .unwrap();
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.drag(right, right, cx);
        w.press("i", cx);
        w.input("right ", cx);
        w.press("escape", cx);
    });
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.active, 1);
            assert!(this.tabs[1].editor.read(cx).value().starts_with("right "));
            assert!(this.tabs[0].editor.read(cx).value().starts_with("left "));
        })
        .unwrap();
    handle
        .update(cx, |this, w, cx| {
            this.close_pane(0, w, cx);
        })
        .unwrap();
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.panes.paths, vec![Some("second.md".into())]);
            assert_eq!(this.tabs.len(), 2);
            assert!(this.status.contains("retained"));
            assert!(Workspace::dirty(&this.tabs[0], cx));
            assert!(Workspace::dirty(&this.tabs[1], cx));
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.open_file("fixture.md".into(), w, cx)).unwrap();
    assert!(handle.read_with(cx, |this, _| this.status.is_empty() && !this.error).unwrap());
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.press("u", cx);
    });
    assert_eq!(text(handle, cx), "one two\nsecond\n");
    handle
        .update(cx, |this, w, cx| {
            this.open_file("second.md".into(), w, cx);
            this.close_tab(false, cx);
        })
        .unwrap();
    assert_eq!(
        handle.read_with(cx, |this, _| this.tabs.len()).unwrap(),
        2,
        "Dirty file close is still guarded"
    );
}
#[gpui_kit::test]
fn visible_saved_panes_restore_without_loading_hidden_tabs_or_stealing_focus(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::base::init(cx);
        init_workspace(cx);
        gpui_omarchy::Theme::tokyo_night().apply(cx);
    });
    let saved = crate::session::Session {
        tabs: ["first.md", "second.md", "hidden.md"]
            .into_iter()
            .map(|p| crate::session::Tab::new(p.into()))
            .collect(),
        active: Some("second.md".into()),
        panes: crate::panes::Panes {
            paths: vec![Some("first.md".into()), Some("second.md".into())],
            focused: 1,
            axis: crate::panes::Axis::Down,
            sizes: vec![280., 320.],
        },
        ..Default::default()
    };
    let service = Arc::new(SessionFixture { session: saved, reads: Default::default() });
    let services = service.clone();
    let handle = cx.add_window(move |w, cx| Workspace::new(services, w, cx));
    handle.update(cx, |this, w, cx| this.restore_session(None, w, cx)).unwrap();
    cx.run_until_parked();
    let mut reads = service.reads.lock().unwrap().clone();
    reads.sort();
    assert_eq!(reads, vec!["first.md", "second.md"]);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.render_frame(cx);
    });
    handle
        .read_with(cx, |this, cx| {
            assert_eq!(this.tabs[this.active].document.path, "second.md");
            assert_eq!(this.panes.focused, 1);
            let state = this.session_snapshot(cx);
            state.validate().unwrap();
            assert_eq!(state.version, 2);
            assert_eq!(state.panes.axis, crate::panes::Axis::Down);
            assert_eq!(this.document_layout.read(cx).sizes().len(), 2);
            assert_eq!(state.panes.paths, vec![Some("first.md".into()), Some("second.md".into())]);
            assert_eq!(this.restored.len(), 1);
        })
        .unwrap();
    handle
        .update(cx, |this, w, cx| {
            this.split_pane(false, w, cx);
            let state = this.session_snapshot(cx);
            state.validate().unwrap();
            assert_eq!(state.active, None);
            assert_eq!(state.panes.paths[state.panes.focused], None);
        })
        .unwrap();
}
#[test]
fn session_v1_without_panes_is_backward_compatible() {
    let mut json = serde_json::to_value(crate::session::Session::default()).unwrap();
    json["version"] = 1.into();
    json.as_object_mut().unwrap().remove("panes");
    let state: crate::session::Session = serde_json::from_value(json).unwrap();
    state.validate().unwrap();
    assert_eq!(state.panes, crate::panes::Panes::default());
}
