//! Explicit editor saves and conflict copies.

use super::app::App;
use crate::{hal, write};

impl App {
    /// Frontmatter + marker are kept verbatim; only the body is written.
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
        let (head, _) = hal::raw_parts(&current);
        let mut body = t.body();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let next = write::set_frontmatter_key(&format!("{head}{body}"), "updated", &write::today());
        match crate::safe_file::replace(&abs, next.as_bytes(), Some(current.as_bytes())) {
            Ok(()) => {
                let p = hal::parse(&next);
                if let Some(t) = self.tab_mut() {
                    t.dirty = false;
                    t.hal = p.hal;
                    t.hal_valid = p.hal_valid;
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
        let head = t.saved_source.as_deref().map_or("", |s| hal::raw_parts(s).0);
        let mut body = t.body();
        if !body.ends_with('\n') {
            body.push('\n');
        }
        let next = write::set_frontmatter_key(&format!("{head}{body}"), "updated", &write::today());
        for number in 1..=1000 {
            let rel = parent.join(format!("{stem} (Lapis copy {number}).md")).to_string_lossy().into_owned();
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
