use super::*;

/// Six properties for `props.md`, one of them a wikilink; one for every other file.
pub(super) fn fixture_properties(path: &str) -> serde_json::Value {
    if path != "props.md" {
        return serde_json::json!({"custom": "keep"});
    }
    serde_json::json!({
        "type": "note", "status": "live", "updated": "2026-10-25",
        "parent": "[[notes/alpha|Alpha]]", "tags": ["a", "b"], "custom": "keep",
    })
}

fn active_path(handle: WindowHandle<Workspace>, cx: &TestAppContext) -> Option<String> {
    handle.read_with(cx, |this, _| this.tabs.get(this.active).map(|t| t.document.path.clone())).unwrap()
}

#[gpui_kit::test]
fn clicking_a_wikilink_in_live_opens_the_note_it_names(cx: &mut TestAppContext) {
    let handle = setup(cx);
    // The caret sits in the first paragraph, so the second one is closed and shows its link.
    let source = "Intro.\n\nGo to [[notes/alpha|Alpha]] now.\n";
    replace_fixture(handle, source, cx);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    let at = handle
        .read_with(cx, |this, cx| {
            let live = this.tabs[0].live.read(cx);
            let p = live.point_for_source(source.find("Alpha").unwrap() + 1).expect("link has geometry");
            p + gpui_kit::point(gpui_kit::px(2.), gpui_kit::px(8.))
        })
        .unwrap();
    visual.update(|w, cx| w.drag(at, at, cx));
    cx.run_until_parked();
    assert_eq!(active_path(handle, cx).as_deref(), Some("notes/alpha.md"));
    // The click followed the link; it did not move the caret into the source.
    let selection = handle.read_with(cx, |this, cx| this.tabs[0].editor.read(cx).selected_range()).unwrap();
    assert_eq!(selection, 0..0);
}

#[gpui_kit::test]
fn a_link_to_nothing_reports_and_stays(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle.update(cx, |this, w, cx| this.follow_link("nowhere".into(), true, w, cx)).unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.tabs.len(), 1);
            assert!(this.error);
            assert_eq!(this.status, "No note matches nowhere");
        })
        .unwrap();
}

#[gpui_kit::test]
fn the_properties_card_expands_and_its_links_follow(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle.update(cx, |this, w, cx| this.open_file("props.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    let live = handle.read_with(cx, |this, _| this.tabs[this.active].live.clone()).unwrap();
    let (rows, toggle) = cx.read(|cx| {
        let live = live.read(cx);
        (live.card_rows(), live.card_targets().0)
    });
    assert_eq!(rows, crate::live::properties::SUMMARY);
    let toggle = toggle.expect("six properties offer a toggle");
    visual.update(|w, cx| {
        w.drag(toggle, toggle, cx);
        w.render_frame(cx);
    });
    let (rows, links) = cx.read(|cx| {
        let live = live.read(cx);
        (live.card_rows(), live.card_targets().1)
    });
    assert_eq!(rows, 6);
    let link = *links.first().expect("parent is a wikilink");
    visual.update(|w, cx| w.drag(link, link, cx));
    cx.run_until_parked();
    assert_eq!(active_path(handle, cx).as_deref(), Some("notes/alpha.md"));
}

#[gpui_kit::test]
fn reading_view_has_the_card_and_routes_rewritten_wikilinks(cx: &mut TestAppContext) {
    let handle = setup(cx);
    handle.update(cx, |this, w, cx| this.open_file("props.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    let has_card = handle
        .update(cx, |this, _, cx| {
            let index = this.active;
            this.set_view(index, View::Reading, cx);
            this.reading_card(index, cx).is_some()
        })
        .unwrap();
    assert!(has_card);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    handle.update(cx, |this, w, cx| this.follow_reading_link("lapis-wiki:notes/alpha", w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(active_path(handle, cx).as_deref(), Some("notes/alpha.md"));
}
