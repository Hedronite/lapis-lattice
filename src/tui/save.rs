//! Explicit editor saves and conflict copies.

use super::app::App;
use crate::{hal, notes, write};

impl App {
    /// Preserve Markdown metadata and YAML source according to the shared editor policy.
    pub(crate) fn save(&mut self) {
        let Some(t) = self.tab() else { return };
        if t.readonly {
            self.set_status("read-only (PDF/HTML text)");
            return;
        }
        let rel = t.rel.clone();
        let abs = self.root().join(&rel);
        let current = match std::fs::read_to_string(&abs) {
            Ok(current) => current,
            Err(e) => {
                self.set_status(format!("save failed reading {rel}: {e}; buffer retained"));
                return;
            }
        };
        if t.saved_source.as_ref().is_some_and(|saved| saved != &current) {
            self.set_status(format!(
                "{rel} changed on disk; save cancelled; Space l S saves a copy to keep both versions"
            ));
            return;
        }
        let next = write::editor_content(&rel, &current, &t.body());
        match crate::safe_file::replace(&abs, next.as_bytes(), Some(current.as_bytes())) {
            Ok(()) => {
                if let Some(t) = self.tab_mut() {
                    t.dirty = false;
                    if notes::kind_of(&rel) == notes::Kind::Markdown {
                        let p = hal::parse(&next);
                        t.hal = p.hal;
                        t.hal_valid = p.hal_valid;
                    }
                    t.saved_source = Some(next.clone());
                }
                self.set_status(format!("saved {rel}"));
                self.kick(rel);
            }
            Err(e) => self.set_status(format!("save failed: {e}")),
        }
    }

    pub(crate) fn save_copy(&mut self) {
        let Some(t) = self.tab() else { return };
        if t.readonly {
            self.set_status("read-only reference; copy text instead");
            return;
        }
        let original = t.rel.clone();
        let path = std::path::Path::new(&original);
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Note");
        // Use the buffer's original metadata, not a conflicting agent edit.
        let next = write::editor_content(&original, t.saved_source.as_deref().unwrap_or(""), &t.body());
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("md");
        for number in 1..=1000 {
            let rel = parent
                .join(format!("{stem} (Lapis copy {number}).{extension}"))
                .to_string_lossy()
                .into_owned();
            match crate::safe_file::create(&self.root().join(&rel), next.as_bytes()) {
                Ok(()) => {
                    self.tree.reload(&self.root(), &parent.to_string_lossy());
                    self.kick(rel.clone());
                    self.open_note(&rel);
                    self.set_status(format!("saved {rel}; original buffer retained"));
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    self.set_status(format!("save copy failed: {e}; buffer retained"));
                    return;
                }
            }
        }
        self.set_status("save copy failed: all copy names are in use; buffer retained");
    }
}

#[cfg(test)]
#[path = "save_tests.rs"]
mod tests;
