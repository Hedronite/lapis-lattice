//! Saved document arrangement. A file is rendered by at most one pane.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    Across,
    Down,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panes {
    pub paths: Vec<Option<String>>,
    pub focused: usize,
    pub axis: Axis,
    pub sizes: Vec<f32>,
}
impl Default for Panes {
    fn default() -> Self {
        Self { paths: vec![None], focused: 0, axis: Axis::Across, sizes: vec![] }
    }
}
impl Panes {
    pub fn assign(&mut self, path: String) {
        if let Some(index) = self.paths.iter().position(|p| p.as_ref() == Some(&path)) {
            self.focused = index;
        } else {
            self.paths[self.focused] = Some(path);
        }
    }
    pub fn contains(&self, path: &str) -> bool {
        self.paths.iter().any(|p| p.as_deref() == Some(path))
    }
    pub fn split(&mut self, axis: Axis) -> bool {
        if self.paths.len() >= 4 {
            return false;
        }
        self.axis = axis;
        self.focused += 1;
        self.paths.insert(self.focused, None);
        self.sizes.clear();
        true
    }
    pub fn close(&mut self, index: usize) {
        if self.paths.len() == 1 || index >= self.paths.len() {
            return;
        }
        self.paths.remove(index);
        self.focused =
            if index < self.focused { self.focused - 1 } else { self.focused.min(self.paths.len() - 1) };
        self.sizes.clear();
    }
    pub fn remove_path(&mut self, path: &str) {
        for p in &mut self.paths {
            if p.as_deref() == Some(path) {
                *p = None;
            }
        }
    }
    pub fn validate(&self, tabs: &[crate::session::Tab]) -> Result<(), String> {
        if self.paths.is_empty() || self.paths.len() > 4 || self.focused >= self.paths.len() {
            return Err("Invalid saved document panes".into());
        }
        let mut seen = std::collections::HashSet::new();
        for path in self.paths.iter().flatten() {
            if !seen.insert(path) || !tabs.iter().any(|t| &t.path == path) {
                return Err("Missing or duplicate document in saved panes".into());
            }
        }
        if (!self.sizes.is_empty() && self.sizes.len() != self.paths.len())
            || self.sizes.iter().any(|v| !v.is_finite() || !(80. ..=10000.).contains(v))
        {
            return Err("Invalid saved document pane sizes".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_existing_documents_without_duplicating_or_moving_them() {
        let mut p = Panes::default();
        p.assign("a.md".into());
        p.split(Axis::Across);
        p.assign("b.md".into());
        p.assign("a.md".into());
        assert_eq!(p.focused, 0);
        assert_eq!(p.paths, vec![Some("a.md".into()), Some("b.md".into())]);
        p.close(0);
        assert_eq!(p.paths, vec![Some("b.md".into())]);
        p.remove_path("b.md");
        assert_eq!(p.paths, vec![None]);
    }
    #[test]
    fn validates_boundaries_and_unique_file_ownership() {
        let mut p = Panes::default();
        let tabs = vec![crate::session::Tab::new("a.md".into())];
        p.assign("a.md".into());
        assert!(p.validate(&tabs).is_ok());
        p.paths.push(Some("a.md".into()));
        assert!(p.validate(&tabs).is_err());
        p.paths.pop();
        p.sizes.push(f32::NAN);
        assert!(p.validate(&tabs).is_err());
    }
}
