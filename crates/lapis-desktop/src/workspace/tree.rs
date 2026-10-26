//! Sidebar tree behaviour: lazy folder loads, expand in place, reveal the open note.
use super::*;
use crate::file_tree::ancestors;

impl Workspace {
    /// Load one folder's listing in the background. A newer request for it wins.
    pub(super) fn load_folder(&mut self, folder: String, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.tree.request(&folder);
        let service = self.services.clone();
        let path = folder.clone();
        let task = cx.background_executor().spawn(async move { service.directory(&path) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let applied = this.update_in(cx, |this, window, cx| {
                if let (true, Err(e)) = (folder.is_empty(), &result) {
                    this.status = e.clone();
                    this.error = true;
                }
                if this.tree.receive(&folder, generation, result) {
                    cx.notify();
                    window.refresh();
                }
            });
            if let Err(error) = applied {
                eprintln!("lapis workspace: folder listing dropped: {error}");
            }
        })
        .detach();
        cx.notify();
    }
    fn ensure_loaded(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tree.is_loaded(folder) && !self.tree.is_loading(folder) {
            self.load_folder(folder.to_string(), window, cx);
        }
    }
    /// Make `path` the current folder: open it and every folder above it.
    pub(super) fn directory(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_loaded("", window, cx);
        if !path.is_empty() {
            for folder in ancestors(&format!("{path}/")) {
                self.tree.expand(&folder);
                self.ensure_loaded(&folder, window, cx);
            }
        }
        self.folder = path;
    }
    /// A folder row was clicked: it becomes current and opens or closes in place.
    pub(super) fn toggle_folder(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.tree.toggle(&path) {
            self.ensure_loaded(&path, window, cx);
        }
        self.folder = path;
        cx.notify();
    }
    /// Reload the root and every open folder, keeping what shows until replies arrive.
    pub(super) fn refresh_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for folder in self.tree.visible_folders() {
            self.load_folder(folder, window, cx);
        }
    }
    pub(super) fn collapse_tree(&mut self, cx: &mut Context<Self>) {
        self.tree.collapse_all();
        cx.notify();
    }
    /// An activated note goes to the front of Recent and is revealed in the tree.
    pub(super) fn note_opened(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path.clone());
        self.recent.truncate(RECENT);
        for folder in ancestors(&path) {
            self.tree.expand(&folder);
            self.ensure_loaded(&folder, window, cx);
        }
        self.reveal = Some(path);
    }
}

/// Notes kept in the sidebar's Recent section.
pub(super) const RECENT: usize = 5;
