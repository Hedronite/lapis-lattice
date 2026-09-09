//! Ctrl+P palette: lattice search by default, `>` prefix for commands.

use super::leader::{self, Cmd};
use crate::lattice::Hit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Note { path: String, title: String, snippet: Option<String> },
    Command { keys: String, cmd: Cmd },
}

pub struct Palette {
    pub input: String,
    pub items: Vec<Item>,
    pub sel: usize,
    pub seq: u64,
    pub pending: bool,
    /// Last query the lattice was asked; avoids duplicate requests.
    pub asked: String,
}

impl Palette {
    pub fn new(input: &str) -> Self {
        let mut p = Self {
            input: input.to_string(),
            items: vec![],
            sel: 0,
            seq: 0,
            pending: false,
            asked: String::new(),
        };
        p.refresh_commands();
        p
    }

    pub fn is_commands(&self) -> bool {
        self.input.starts_with('>')
    }

    pub fn query(&self) -> String {
        self.input.trim_start_matches('>').trim().to_string()
    }

    /// Filter commands locally (fuzzy-ish: every query word is a substring).
    pub fn refresh_commands(&mut self) {
        if !self.is_commands() {
            return;
        }
        let q = self.query().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        self.items = leader::all_commands()
            .into_iter()
            .filter(|(keys, cmd)| {
                let hay = format!("{} {}", keys.to_lowercase(), cmd.label().to_lowercase());
                words.iter().all(|w| hay.contains(w))
            })
            .map(|(keys, cmd)| Item::Command { keys, cmd })
            .collect();
        self.sel = 0;
    }

    pub fn set_hits(&mut self, hits: Vec<Hit>) {
        self.items = hits
            .into_iter()
            .map(|h| Item::Note { path: h.path, title: h.title, snippet: h.heading.or(h.snippet) })
            .collect();
        self.sel = 0;
        self.pending = false;
    }

    pub fn up(&mut self) {
        self.sel = self.sel.saturating_sub(1);
    }
    pub fn down(&mut self) {
        self.sel = (self.sel + 1).min(self.items.len().saturating_sub(1));
    }
    pub fn selected(&self) -> Option<&Item> {
        self.items.get(self.sel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_filter_and_mode() {
        let mut p = Palette::new(">kan");
        assert!(p.is_commands());
        assert_eq!(p.query(), "kan");
        assert!(p.items.iter().all(|i| matches!(i, Item::Command { cmd: Cmd::Kanban, .. })));
        p.input = ">".into();
        p.refresh_commands();
        assert!(p.items.len() > 10);
        let q = Palette::new("hedron");
        assert!(!q.is_commands());
        assert!(q.items.is_empty());
    }
}
