//! Host operations injected by the binary; desktop never owns a second writer or ranker.

use serde_json::Value;
pub type ArcCancel = std::sync::Arc<std::sync::atomic::AtomicBool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Yaml,
    Html,
    Pdf,
    Source,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub directory: bool,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub path: String,
    pub kind: FileKind,
    pub title: String,
    pub text: String,
    /// Exact original file, retained for optimistic concurrency and opaque HAL fields.
    pub original: String,
    pub properties: Value,
    pub readonly: bool,
}

#[derive(Debug, Clone)]
pub struct ContextLink {
    pub path: Option<String>,
    pub label: String,
    pub direction: String,
}
#[derive(Debug, Clone)]
pub struct ContextTree {
    pub seed: String,
    pub nodes: Vec<(String, u32)>,
    pub truncated: bool,
}

pub struct SearchPage {
    pub hits: Vec<lapis_lattice::Hit>,
    pub modalities: Vec<String>,
    pub indexed_documents: u64,
    pub can_build_index: bool,
}

#[derive(Debug, Clone)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: String,
    pub path: String,
    pub line: Option<usize>,
    pub content: String,
    pub checked: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
}

/// Blocking operations. Call on a background executor, never during GPUI paint/input.
pub trait WorkspaceServices: Send + Sync {
    fn load_session(&self) -> Result<Option<crate::session::Session>, String> {
        Ok(None)
    }
    fn save_session(&self, _session: &crate::session::Session) -> Result<(), String> {
        Ok(())
    }

    fn directory(&self, path: &str) -> Result<Vec<FileEntry>, String>;
    fn read(&self, path: &str) -> Result<Document, String>;
    fn save(&self, document: &Document, text: &str) -> Result<Document, String>;
    fn save_copy(&self, document: &Document, text: &str) -> Result<Document, String>;
    fn search(&self, query: &str) -> Result<SearchPage, String>;
    fn reindex(&self, path: &str) -> Result<(), String>;
    fn build_index(&self) -> Result<u64, String>;
    fn links(&self, _path: &str) -> Result<Vec<ContextLink>, String> {
        Err("Link context is unavailable in this service".into())
    }
    fn graph_snapshot(&self) -> Result<lapis_lattice::GraphSnapshot, String> {
        Err("Graph snapshots are unavailable in this service".into())
    }
    fn graph_preview(&self, _path: &str) -> Result<String, String> {
        Err("Graph previews are unavailable in this service".into())
    }
    fn tree(&self, _path: &str) -> Result<ContextTree, String> {
        Err("Tree retrieval is unavailable in this service".into())
    }
    fn pdf_page(&self, _path: &str, _page: u32, _width: u32, _cancel: ArcCancel) -> Result<PdfPage, String> {
        Err("PDF page rendering is unavailable in this service".into())
    }

    // Notes workflows. Each returns the vault-relative path it produced or acted on.
    fn templates(&self) -> Result<Vec<TemplateInfo>, String> {
        Err("Templates are unavailable in this service".into())
    }
    fn create_note(&self, _title: &str, _folder: &str, _template: Option<&str>) -> Result<String, String> {
        Err("Creating notes is unavailable in this service".into())
    }
    fn periodic(&self, _period: Period) -> Result<String, String> {
        Err("Periodic notes are unavailable in this service".into())
    }
    fn capture(&self, _text: &str) -> Result<String, String> {
        Err("Quick capture is unavailable in this service".into())
    }
    fn tags(&self) -> Result<Vec<(String, u64)>, String> {
        Err("Tags are unavailable in this service".into())
    }
    fn tagged(&self, _tag: &str) -> Result<Vec<String>, String> {
        Err("Tags are unavailable in this service".into())
    }
    fn tasks(&self) -> Result<Vec<TaskRow>, String> {
        Err("Tasks are unavailable in this service".into())
    }
    /// Returns the task's new checked state.
    fn toggle_task(&self, _id: &str) -> Result<bool, String> {
        Err("Tasks are unavailable in this service".into())
    }
    /// Returns where the note went.
    fn trash(&self, _path: &str) -> Result<String, String> {
        Err("Trash is unavailable in this service".into())
    }
    fn trash_list(&self) -> Result<Vec<String>, String> {
        Err("Trash is unavailable in this service".into())
    }
    /// Returns the restored path.
    fn restore(&self, _trashed: &str) -> Result<String, String> {
        Err("Trash is unavailable in this service".into())
    }
    /// Vault-relative paths changed outside the workspace since the last call. A
    /// service without a watcher returns nothing.
    fn changed_paths(&self) -> Vec<String> {
        vec![]
    }
}

/// Pixel dimensions are bounded independently of the document's page count.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct PdfPageInfo {
    pub page: u32,
    pub pages: u32,
    pub width: u32,
    pub height: u32,
    pub text: String,
}
#[derive(Debug)]
pub struct PdfPage {
    pub info: PdfPageInfo,
    pub bgra: Vec<u8>,
    pub revision: String,
}
