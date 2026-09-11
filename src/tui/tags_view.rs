//! `Space #` tags browser: every tag the lattice knows (`GET /documents`,
//! the same rows `lapis documents` prints) plus inline `#tag`s from the task
//! scan, with the notes under each. Pure state; the overlay draws `rows()`.

use std::collections::{BTreeMap, BTreeSet};

use crate::http::Document;
use crate::tasks::Task;

#[derive(Debug, Default, Clone)]
pub struct TagsBrowser {
    /// tag → note paths carrying it (sorted, deduplicated).
    by_tag: BTreeMap<String, BTreeSet<String>>,
    /// Substring filter typed into the browser.
    pub filter: String,
    pub sel: usize,
    /// Tag drilled into; `None` while listing tags.
    pub open: Option<String>,
    pub note_sel: usize,
    pub loading: bool,
}

/// What Enter did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Drilled into a tag; keep the overlay.
    Tag(String),
    /// A note was chosen; open it.
    Note(String),
    Nothing,
}

fn norm(tag: &str) -> Option<String> {
    let t = tag.trim().trim_start_matches('#').trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

impl TagsBrowser {
    pub fn new() -> Self {
        Self { loading: true, ..Self::default() }
    }

    pub fn add<I, S>(&mut self, path: &str, tags: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for t in tags {
            if let Some(t) = norm(t.as_ref()) {
                self.by_tag.entry(t).or_default().insert(path.to_string());
            }
        }
    }

    pub fn add_documents(&mut self, docs: &[Document]) {
        for d in docs {
            self.add(&d.path, &d.tags);
        }
    }

    pub fn add_tasks(&mut self, tasks: &[Task]) {
        for t in tasks {
            self.add(&t.source_path, &t.tags);
        }
    }

    /// Tags matching the filter, most-used first, then alphabetical.
    pub fn tags(&self) -> Vec<(&str, usize)> {
        let f = self.filter.to_lowercase();
        let mut v: Vec<(&str, usize)> = self
            .by_tag
            .iter()
            .filter(|(t, _)| f.is_empty() || t.to_lowercase().contains(&f))
            .map(|(t, p)| (t.as_str(), p.len()))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        v
    }

    /// Note paths under the drilled-into tag.
    pub fn notes(&self) -> Vec<&str> {
        self.open
            .as_ref()
            .and_then(|t| self.by_tag.get(t))
            .map(|s| s.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    pub fn title(&self) -> String {
        match &self.open {
            Some(t) => format!(" #{t}  (Enter opens · Esc back) "),
            None if self.loading => " tags  (loading…) ".to_string(),
            None if self.filter.is_empty() => " tags  (type to filter · Enter drills in) ".to_string(),
            None => format!(" tags  /{} ", self.filter),
        }
    }

    pub fn rows(&self) -> Vec<String> {
        if self.open.is_some() {
            self.notes().into_iter().map(str::to_string).collect()
        } else {
            self.tags().into_iter().map(|(t, n)| format!("#{t:<28} {n:>4}")).collect()
        }
    }

    pub fn selected_index(&self) -> usize {
        if self.open.is_some() { self.note_sel } else { self.sel }
    }

    fn len(&self) -> usize {
        if self.open.is_some() { self.notes().len() } else { self.tags().len() }
    }

    pub fn down(&mut self) {
        let max = self.len().saturating_sub(1);
        if self.open.is_some() {
            self.note_sel = (self.note_sel + 1).min(max);
        } else {
            self.sel = (self.sel + 1).min(max);
        }
    }

    pub fn up(&mut self) {
        if self.open.is_some() {
            self.note_sel = self.note_sel.saturating_sub(1);
        } else {
            self.sel = self.sel.saturating_sub(1);
        }
    }

    pub fn type_char(&mut self, c: char) {
        if self.open.is_none() {
            self.filter.push(c);
            self.sel = 0;
        }
    }

    pub fn backspace(&mut self) {
        if self.open.is_none() {
            self.filter.pop();
            self.sel = 0;
        }
    }

    pub fn enter(&mut self) -> Pick {
        if self.open.is_some() {
            match self.notes().get(self.note_sel) {
                Some(p) => Pick::Note(p.to_string()),
                None => Pick::Nothing,
            }
        } else {
            match self.tags().get(self.sel) {
                Some((t, _)) => {
                    let t = t.to_string();
                    self.open = Some(t.clone());
                    self.note_sel = 0;
                    Pick::Tag(t)
                }
                None => Pick::Nothing,
            }
        }
    }

    /// Esc: leave the drilled-in tag. Returns `false` when already at the top
    /// (caller closes the overlay).
    pub fn back(&mut self) -> bool {
        if self.open.take().is_some() {
            self.note_sel = 0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, tags: &[&str]) -> Document {
        Document {
            path: path.into(),
            title: None,
            hash: None,
            domain: None,
            doc_type: None,
            status: None,
            priority: None,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            mtime: None,
        }
    }

    #[test]
    fn lists_tags_from_documents_and_tasks() {
        let mut b = TagsBrowser::new();
        b.add_documents(&[doc("foundry/spec.md", &["foundry", "spec"]), doc("notes/a.md", &["#spec"])]);
        let tasks = crate::tasks::parse("plans/p.md", "- [ ] ship it #lapis #spec\n");
        assert_eq!(tasks[0].tags, ["lapis", "spec"]);
        b.add_tasks(&tasks);
        b.loading = false;
        let tags: Vec<(&str, usize)> = b.tags();
        assert_eq!(tags, [("spec", 3), ("foundry", 1), ("lapis", 1)]);
        assert!(b.rows()[0].starts_with("#spec"));
    }

    #[test]
    fn filter_drill_in_and_back() {
        let mut b = TagsBrowser::new();
        b.add_documents(&[doc("x.md", &["alpha", "beta"]), doc("y.md", &["beta"])]);
        b.type_char('b');
        assert_eq!(b.tags(), [("beta", 2)]);
        assert_eq!(b.enter(), Pick::Tag("beta".into()));
        assert_eq!(b.notes(), ["x.md", "y.md"]);
        b.down();
        assert_eq!(b.enter(), Pick::Note("y.md".into()));
        assert!(b.back());
        assert!(!b.back());
        b.backspace();
        assert_eq!(b.tags().len(), 2);
    }
}
