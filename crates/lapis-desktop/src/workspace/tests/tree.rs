use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `notes/` holds `a.md` and `deep/b.md`; `top.md` sits at the root.
struct TreeFixture {
    base: Fixture,
    listings: AtomicUsize,
}
fn dir(path: &str) -> FileEntry {
    FileEntry { path: path.into(), name: path.rsplit('/').next().unwrap().into(), directory: true }
}
fn file(path: &str) -> FileEntry {
    FileEntry { path: path.into(), name: path.rsplit('/').next().unwrap().into(), directory: false }
}
impl WorkspaceServices for TreeFixture {
    fn directory(&self, p: &str) -> Result<Vec<FileEntry>, String> {
        self.listings.fetch_add(1, Ordering::SeqCst);
        Ok(match p {
            "" => vec![dir("notes"), file("top.md")],
            "notes" => vec![dir("notes/deep"), file("notes/a.md")],
            "notes/deep" => vec![file("notes/deep/b.md")],
            _ => vec![],
        })
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
}
fn shown(handle: WindowHandle<Workspace>, cx: &TestAppContext) -> Vec<(String, usize)> {
    handle
        .read_with(cx, |this, _| this.tree.rows().iter().map(|r| (r.entry.path.clone(), r.depth)).collect())
        .unwrap()
}
fn row(path: &str, depth: usize) -> (String, usize) {
    (path.into(), depth)
}

#[gpui_kit::test]
fn folders_expand_in_place_and_an_opened_note_is_revealed(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let services = Arc::new(TreeFixture { base: Fixture::new(false), listings: 0.into() });
    handle
        .update(cx, |this, w, cx| {
            this.services = services.clone();
            this.directory(String::new(), w, cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(shown(handle, cx), [row("notes", 0), row("top.md", 0)]);

    handle.update(cx, |this, w, cx| this.toggle_folder("notes".into(), w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(
        shown(handle, cx),
        [row("notes", 0), row("notes/deep", 1), row("notes/a.md", 1), row("top.md", 0)]
    );
    assert_eq!(handle.read_with(cx, |this, _| this.folder.clone()).unwrap(), "notes");

    handle.update(cx, |this, _, cx| this.collapse_tree(cx)).unwrap();
    assert_eq!(shown(handle, cx), [row("notes", 0), row("top.md", 0)]);

    // Opening a nested note opens every folder above it and puts it first in Recent.
    handle.update(cx, |this, w, cx| this.open_file("notes/deep/b.md".into(), w, cx)).unwrap();
    cx.run_until_parked();
    assert!(shown(handle, cx).contains(&row("notes/deep/b.md", 2)));
    let mut visual = VisualTestContext::from_window(handle.into(), cx);
    visual.update(|w, cx| w.render_frame(cx));
    handle
        .read_with(cx, |this, _| {
            assert_eq!(this.recent.first().map(String::as_str), Some("notes/deep/b.md"));
            assert!(this.reveal.is_none(), "the revealed row was scrolled to");
        })
        .unwrap();

    // A refresh reloads the root and each open folder, not the whole vault.
    let before = services.listings.load(Ordering::SeqCst);
    handle.update(cx, |this, w, cx| this.refresh_tree(w, cx)).unwrap();
    cx.run_until_parked();
    assert_eq!(services.listings.load(Ordering::SeqCst) - before, 3);
}
