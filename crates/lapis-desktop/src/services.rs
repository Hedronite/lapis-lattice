//! Host operations injected by the binary; desktop never owns a second writer or ranker.

use serde_json::Value;

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

pub struct SearchPage {
    pub hits: Vec<lapis_lattice::Hit>,
    pub modalities: Vec<String>,
    pub indexed_documents: u64,
    pub can_build_index: bool,
}

/// Blocking operations. Call on a background executor, never during GPUI paint/input.
pub trait WorkspaceServices: Send + Sync {
    fn directory(&self, path: &str) -> Result<Vec<FileEntry>, String>;
    fn read(&self, path: &str) -> Result<Document, String>;
    fn save(&self, document: &Document, text: &str) -> Result<Document, String>;
    fn save_copy(&self, document: &Document, text: &str) -> Result<Document, String>;
    fn search(&self, query: &str) -> Result<SearchPage, String>;
    fn reindex(&self, path: &str) -> Result<(), String>;
    fn build_index(&self) -> Result<u64, String>;
}
