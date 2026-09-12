//! Bounded workspace metadata. Note content and recovery buffers are not session state.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    #[default]
    Live,
    Source,
    Reading,
    Split,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub path: String,
    pub title: String,
    pub view: View,
    pub selection: [usize; 2],
    pub source_scroll: [f32; 2],
    pub reading_scroll: [f32; 2],
    pub live_scroll: f32,
    pub split_width: Option<f32>,
    pub pdf_page: u32,
    pub pdf_zoom: f32,
}
impl Tab {
    pub fn new(path: String) -> Self {
        Self {
            title: path.rsplit('/').next().unwrap_or(&path).into(),
            path,
            view: View::Live,
            selection: [0, 0],
            source_scroll: [0., 0.],
            reading_scroll: [0., 0.],
            live_scroll: 0.,
            split_width: None,
            pdf_page: 0,
            pdf_zoom: 1.,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub version: u32,
    pub tabs: Vec<Tab>,
    pub active: Option<String>,
    pub folder: String,
    pub context: bool,
    pub sidebar_width: f32,
    pub context_width: f32,
}
impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            tabs: vec![],
            active: None,
            folder: String::new(),
            context: false,
            sidebar_width: 232.,
            context_width: 280.,
        }
    }
}
fn path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', '\0'])
        && path.split('/').all(|p| !matches!(p, "" | "." | ".."))
}
impl Session {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("Unsupported workspace session version {}", self.version));
        }
        if self.tabs.len() > 128 {
            return Err("Workspace session exceeds 128 tabs".into());
        }
        if !self.folder.is_empty() && !path_ok(&self.folder) {
            return Err("Invalid saved folder".into());
        }
        let mut paths = std::collections::HashSet::new();
        for tab in &self.tabs {
            if !path_ok(&tab.path) || tab.title.len() > 4096 || !paths.insert(&tab.path) {
                return Err("Invalid or duplicate saved tab".into());
            }
            if tab.selection[0] > tab.selection[1] || tab.selection[1] > 32 * 1024 * 1024 {
                return Err("Invalid saved text position".into());
            }
            if tab
                .source_scroll
                .into_iter()
                .chain(tab.reading_scroll)
                .any(|v| !v.is_finite() || !(-1e9..=0.).contains(&v))
                || tab.split_width.is_some_and(|v| !v.is_finite() || !(100. ..=10000.).contains(&v))
                || !tab.live_scroll.is_finite()
                || !(0. ..=1e9).contains(&tab.live_scroll)
                || !tab.pdf_zoom.is_finite()
                || !(0.25..=4.).contains(&tab.pdf_zoom)
            {
                return Err("Invalid saved viewport".into());
            }
        }
        if self.active.as_ref().is_some_and(|p| !paths.contains(p)) {
            return Err("Active saved tab is missing".into());
        }
        if !self.sidebar_width.is_finite()
            || !(140. ..=480.).contains(&self.sidebar_width)
            || !self.context_width.is_finite()
            || !(180. ..=520.).contains(&self.context_width)
        {
            return Err("Invalid saved pane width".into());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_paths_versions_duplicates_and_unbounded_positions() {
        for path in ["../note.md", "/note.md", "folder/../note.md", "a\\b.md"] {
            let mut s = Session::default();
            s.tabs.push(Tab::new(path.into()));
            assert!(s.validate().is_err());
        }
        let mut s = Session { version: 2, ..Default::default() };
        assert!(s.validate().is_err());
        s.version = 1;
        s.tabs = vec![Tab::new("note.md".into()); 2];
        assert!(s.validate().is_err());
        s.tabs.pop();
        s.active = Some("note.md".into());
        assert!(s.validate().is_ok());
        s.tabs[0].selection = [0, usize::MAX];
        assert!(s.validate().is_err());
    }
}
