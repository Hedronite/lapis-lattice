use super::*;
use crate::services::SearchPage;
use crate::workspace::palette::Arm;

fn page(paths: &[&str], modalities: &[&str]) -> Result<SearchPage, String> {
    let hits = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            serde_json::from_value(serde_json::json!({
                "path": p, "kind": "markdown", "title": p, "rank": i + 1, "score": 1.0,
                "domain": null, "doc_type": null,
            }))
            .unwrap()
        })
        .collect();
    Ok(SearchPage {
        hits,
        modalities: modalities.iter().map(|m| m.to_string()).collect(),
        indexed_documents: 10,
        can_build_index: false,
    })
}
fn shown(handle: WindowHandle<Workspace>, cx: &TestAppContext) -> (Vec<String>, usize, bool, bool) {
    handle
        .read_with(cx, |this, _| {
            let p = this.palette.as_ref().unwrap();
            (p.hits.iter().map(|h| h.path.clone()).collect(), p.selected, p.refining, p.vector_missed)
        })
        .unwrap()
}

#[gpui_kit::test]
fn keyword_hits_show_first_and_the_hybrid_ranking_replaces_them(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let epoch = handle
        .update(cx, |this, w, cx| {
            this.open_palette(w, cx);
            this.query_epoch
        })
        .unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.apply_search(epoch, Arm::Keyword, page(&["a.md", "b.md"], &["bm25"]), cx)
        })
        .unwrap();
    assert_eq!(shown(handle, cx), (vec!["a.md".into(), "b.md".into()], 0, true, false));
    // The reader moves to b.md while vectors are still on their way.
    handle.update(cx, |this, _, _| this.palette.as_mut().unwrap().selected = 1).unwrap();
    handle
        .update(cx, |this, _, cx| {
            this.apply_search(epoch, Arm::Hybrid, page(&["c.md", "a.md", "b.md"], &["vector", "bm25"]), cx)
        })
        .unwrap();
    assert_eq!(shown(handle, cx), (vec!["c.md".into(), "a.md".into(), "b.md".into()], 2, false, false));
    // A keyword reply that arrives after the ranking is stale.
    handle
        .update(cx, |this, _, cx| this.apply_search(epoch, Arm::Keyword, page(&["z.md"], &["bm25"]), cx))
        .unwrap();
    assert_eq!(shown(handle, cx).0, ["c.md", "a.md", "b.md"]);
}

#[gpui_kit::test]
fn keyword_hits_stay_when_the_vector_ranking_fails(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let epoch = handle
        .update(cx, |this, w, cx| {
            this.open_palette(w, cx);
            this.query_epoch
        })
        .unwrap();
    handle
        .update(cx, |this, _, cx| this.apply_search(epoch, Arm::Keyword, page(&["a.md"], &["bm25"]), cx))
        .unwrap();
    handle
        .update(cx, |this, _, cx| this.apply_search(epoch, Arm::Hybrid, Err("embedder offline".into()), cx))
        .unwrap();
    assert_eq!(shown(handle, cx), (vec!["a.md".into()], 0, false, true));
    // A stale epoch changes nothing.
    handle
        .update(cx, |this, _, cx| this.apply_search(epoch - 1, Arm::Hybrid, page(&["x.md"], &[]), cx))
        .unwrap();
    assert_eq!(shown(handle, cx).0, ["a.md"]);
}

/// A run of queries keeps one search per arm in flight: a query entered while
/// its arm is busy waits, a newer one replaces it, and one the reader has moved
/// past (or closed the palette on) never starts.
#[test]
fn each_arm_runs_one_search_and_only_the_newest_waits() {
    use crate::workspace::palette::Lanes;
    let mut lanes = Lanes::default();
    assert!(lanes.submit(Arm::Hybrid, 1, "tide"));
    assert!(lanes.submit(Arm::Keyword, 1, "tide"), "the arms are independent");
    assert!(!lanes.submit(Arm::Hybrid, 2, "tid"));
    assert!(!lanes.submit(Arm::Hybrid, 3, "tides"), "a newer query replaces the waiting one");
    assert_eq!(lanes.finish(Arm::Hybrid, Some(3)), Some((3, "tides".to_string())));
    assert!(!lanes.submit(Arm::Hybrid, 4, "moon"), "still busy with the query it handed back");
    assert_eq!(lanes.finish(Arm::Hybrid, Some(5)), None, "the reader moved on, so 4 never runs");
    assert!(lanes.submit(Arm::Hybrid, 5, "moon"), "and the arm is free again");
    assert!(!lanes.submit(Arm::Hybrid, 6, "sea"));
    assert_eq!(lanes.finish(Arm::Hybrid, None), None, "a closed palette starts nothing");
    assert_eq!(lanes.finish(Arm::Keyword, Some(6)), None);
    assert!(lanes.submit(Arm::Keyword, 6, "sea"));
}
