use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct GraphFixture {
    base: Fixture,
    requests: AtomicUsize,
    fail: AtomicBool,
}
impl WorkspaceServices for GraphFixture {
    fn directory(&self, p: &str) -> Result<Vec<FileEntry>, String> {
        self.base.directory(p)
    }
    fn read(&self, p: &str) -> Result<Document, String> {
        self.base.read(p)
    }
    fn save(&self, d: &Document, t: &str) -> Result<Document, String> {
        self.base.save(d, t)
    }
    fn save_copy(&self, d: &Document, t: &str) -> Result<Document, String> {
        self.base.save_copy(d, t)
    }
    fn search(&self, q: &str) -> Result<SearchPage, String> {
        self.base.search(q)
    }
    fn reindex(&self, p: &str) -> Result<(), String> {
        self.base.reindex(p)
    }
    fn build_index(&self) -> Result<u64, String> {
        self.base.build_index()
    }
    fn graph_snapshot(&self) -> Result<lapis_lattice::GraphSnapshot, String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err("Fixture backend offline".into());
        }
        Ok(lapis_lattice::GraphSnapshot {
            generated_at: "fixture".into(),
            vault: "fixture".into(),
            truncated: false,
            nodes: ["fixture.md", "second.md", "dangling:Missing", "missing.md"]
                .into_iter()
                .map(|id| lapis_lattice::GraphNode {
                    id: id.into(),
                    path: (!id.starts_with("dangling:")).then(|| id.into()),
                    title: id.into(),
                    domain: None,
                    kind: Some("markdown".into()),
                    degree: 0,
                    dangling: id.starts_with("dangling:"),
                    x: None,
                    y: None,
                })
                .collect(),
            edges: vec![],
        })
    }
    fn graph_preview(&self, p: &str) -> Result<String, String> {
        Ok(format!("Preview of {p}"))
    }
}
#[gpui_kit::test]
fn graph_is_lazy_and_keyboard_navigation_reuses_dirty_buffers(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let services = Arc::new(GraphFixture {
        base: Fixture { reject_save: false, indexed: false.into() },
        requests: 0.into(),
        fail: false.into(),
    });
    handle.update(cx, |this, _, _| this.services = services.clone()).unwrap();
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.press("i", cx);
        cx.write_to_clipboard(ClipboardItem::new_string("unsaved ".into()));
        w.dispatch_action(Box::new(gpui_kit::base::input::Paste), cx);
        w.press("escape", cx);
    });
    assert_eq!(services.requests.load(Ordering::SeqCst), 0);
    handle.update(cx, |this, w, cx| this.show_graph(None, w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(services.requests.load(Ordering::SeqCst), 1);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.press("tab", cx);
        w.press("enter", cx);
    });
    cx.run_until_parked();
    handle
        .read_with(cx, |this, cx| {
            assert!(!this.graph_visible);
            assert_eq!(this.tabs[this.active].document.path, "second.md");
            assert_eq!(this.history.target(false).unwrap().1, history::Visit::Graph);
            assert!(Workspace::dirty(&this.tabs[0], cx));
        })
        .unwrap();
    handle.update(cx, |this, w, cx| this.navigate(false, w, cx)).unwrap();
    cx.run_until_parked();
    assert!(handle.read_with(cx, |this, _| this.graph_visible).unwrap());
    assert_eq!(services.requests.load(Ordering::SeqCst), 1, "Returning to the graph reuses its snapshot");
    handle.update(cx, |this, w, cx| this.navigate(false, w, cx)).unwrap();
    cx.run_until_parked();
    assert!(text(handle, cx).starts_with("unsaved "));
    handle.update(cx, |this, w, cx| this.navigate(true, w, cx)).unwrap();
    cx.run_until_parked();
    // A failed node open and a dangling link retain the graph/history and editor.
    handle
        .update(cx, |this, w, cx| {
            this.graph.as_ref().unwrap().update(cx, |g, cx| g.open_note("missing.md", w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    handle
        .read_with(cx, |this, _| {
            assert!(this.graph_visible);
            assert!(this.error);
        })
        .unwrap();
    handle
        .update(cx, |this, w, cx| {
            this.graph.as_ref().unwrap().update(cx, |g, cx| g.open_note("dangling:Missing", w, cx))
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(handle.read_with(cx, |this, _| this.tabs.len()).unwrap(), 2);
    assert!(text(handle, cx).starts_with("unsaved "));
}
#[gpui_kit::test]
fn graph_hidden_load_is_discarded_and_retry_reads_the_backend(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let services = Arc::new(GraphFixture {
        base: Fixture { reject_save: false, indexed: false.into() },
        requests: 0.into(),
        fail: true.into(),
    });
    handle
        .update(cx, |this, w, cx| {
            this.services = services.clone();
            this.show_graph(None, w, cx);
            this.toggle_graph(w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!handle.read_with(cx, |this, _| this.graph_visible).unwrap());
    services.fail.store(false, Ordering::SeqCst);
    handle.update(cx, |this, w, cx| this.show_graph(None, w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(services.requests.load(Ordering::SeqCst), 2);
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| {
        w.render_frame(cx);
        w.press("tab", cx);
        w.press("enter", cx);
    });
    cx.run_until_parked();
    assert_eq!(
        handle.read_with(cx, |this, _| this.tabs[this.active].document.path.clone()).unwrap(),
        "second.md"
    );
}
