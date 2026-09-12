//! Adapter from the desktop's background requests to canonical product operations.

use crate::{hal, notes, ops, safe_file, write};
use lapis_desktop::services::{Document, FileEntry, FileKind, SearchPage, WorkspaceServices};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Finder launches a native app without CLI arguments. Only an actual macOS
/// application bundle defaults to the workspace; bare CLI invocations keep TUI.
pub fn default_surface(mut cli: crate::cli::Cli, exe: &Path) -> crate::cli::Cli {
    let bundled = cfg!(all(target_os = "macos", feature = "desktop"))
        && exe.parent().is_some_and(|p| p.ends_with("Contents/MacOS"))
        && exe.ancestors().nth(3).is_some_and(|p| p.extension().is_some_and(|e| e == "app"));
    if bundled && cli.subcommand.is_none() {
        cli.subcommand =
            Some(crate::cli::Command::Desktop(crate::cli::DesktopArgs { path: None, check: false }));
    }
    cli
}

pub fn service(ctx: &ops::Ctx) -> Arc<dyn WorkspaceServices> {
    Arc::new(Service { ctx: ctx.clone(), runtime: tokio::runtime::Handle::current() })
}

struct Service {
    ctx: ops::Ctx,
    runtime: tokio::runtime::Handle,
}

impl Service {
    fn content(document: &Document, text: &str) -> String {
        if document.kind == FileKind::Markdown {
            let head = hal::raw_parts(&document.original).0;
            let newline = if text.ends_with('\n') { "" } else { "\n" };
            write::set_frontmatter_key(&format!("{head}{text}{newline}"), "updated", &write::today())
        } else {
            text.into()
        }
    }
    fn contained(&self, rel: &str) -> Result<PathBuf, String> {
        let root = self.ctx.vault.root.canonicalize().map_err(|e| e.to_string())?;
        let path = if rel.is_empty() {
            root.clone()
        } else {
            root.join(notes::clean_rel(rel).map_err(|e| e.to_string())?)
                .canonicalize()
                .map_err(|e| format!("{rel}: {e}"))?
        };
        if !path.starts_with(&root) {
            return Err(format!("{rel}: target is outside the workspace"));
        }
        Ok(path)
    }
}

impl WorkspaceServices for Service {
    fn directory(&self, rel: &str) -> Result<Vec<FileEntry>, String> {
        let directory = self.contained(rel)?;
        let mut entries = vec![];
        for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules") {
                continue;
            }
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let Ok(abs) = self.contained(&path) else { continue };
            let directory = abs.is_dir();
            entries.push(FileEntry { path, name, directory });
        }
        entries.sort_by(|a, b| {
            b.directory.cmp(&a.directory).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn read(&self, rel: &str) -> Result<Document, String> {
        let abs = self.contained(rel)?;
        let kind = match notes::kind_of(rel) {
            notes::Kind::Markdown => FileKind::Markdown,
            notes::Kind::Yaml => FileKind::Yaml,
            notes::Kind::Html => FileKind::Html,
            notes::Kind::Pdf => FileKind::Pdf,
            notes::Kind::Source => FileKind::Source,
        };
        if std::fs::metadata(&abs).map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
            return Err(format!("{rel}: text exceeds the 32 MiB editor limit; open externally"));
        }
        if kind == FileKind::Pdf {
            return Err(format!("{rel}: PDF page reader is not available yet"));
        }
        let original = std::fs::read_to_string(&abs).map_err(|e| format!("{rel}: {e}"))?;
        let (text, properties, title) = if kind == FileKind::Markdown {
            let p = hal::parse(&original);
            let title = hal::title_from_hal(&p.hal);
            (hal::raw_parts(&original).1.to_string(), serde_json::Value::Object(p.hal), title)
        } else {
            (original.clone(), serde_json::json!({}), None)
        };
        Ok(Document {
            path: rel.into(),
            kind,
            text,
            original,
            properties,
            title: title.unwrap_or_else(|| {
                Path::new(rel).file_stem().unwrap_or_default().to_string_lossy().into_owned()
            }),
            readonly: kind == FileKind::Html
                || std::fs::metadata(abs).map_err(|e| e.to_string())?.permissions().readonly(),
        })
    }

    fn save(&self, document: &Document, text: &str) -> Result<Document, String> {
        if document.readonly {
            return Err("This reference is read-only; buffer retained".into());
        }
        self.contained(&document.path)?;
        // Keep the lexical path so the common writer can reject replacing a symlink.
        let abs = self.ctx.vault.root.join(notes::clean_rel(&document.path).map_err(|e| e.to_string())?);
        let next = Self::content(document, text);
        safe_file::replace(&abs, next.as_bytes(), Some(document.original.as_bytes()))
            .map_err(|e| format!("{}: {e}; buffer retained", document.path))?;
        let mut saved = document.clone();
        saved.original = next;
        // Keep the editor's exact text/undo history; the explicit Markdown newline policy is on disk.
        saved.text = text.into();
        Ok(saved)
    }

    fn save_copy(&self, document: &Document, text: &str) -> Result<Document, String> {
        if document.readonly {
            return Err("Read-only reference; copy selected text instead".into());
        }
        let clean = notes::clean_rel(&document.path).map_err(|e| e.to_string())?;
        let path = Path::new(&clean);
        let parent = path.parent().unwrap_or(Path::new(""));
        self.contained(&parent.to_string_lossy())?;
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let extension = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let next = Self::content(document, text);
        for number in 1..=1000 {
            let path = parent
                .join(format!("{stem} (Lapis copy {number}){extension}"))
                .to_string_lossy()
                .into_owned();
            match safe_file::create(&self.ctx.vault.root.join(&path), next.as_bytes()) {
                Ok(()) => {
                    let mut saved = document.clone();
                    saved.path = path;
                    saved.text = text.into();
                    saved.original = next;
                    return Ok(saved);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("Save copy failed: {e}; buffer retained")),
            }
        }
        Err("All copy names are in use; buffer retained".into())
    }

    fn search(&self, query: &str) -> Result<SearchPage, String> {
        self.runtime.block_on(async {
            let backend = self.ctx.backend().map_err(|e| e.to_string())?;
            let health = backend.health().await.map_err(|e| e.to_string())?;
            let page = ops::search(
                &self.ctx,
                ops::SearchQuery { query: query.into(), limit: 30, ..Default::default() },
            )
            .await
            .map_err(|e| e.to_string())?;
            Ok(SearchPage {
                hits: page.result.hits,
                modalities: page.result.modalities,
                indexed_documents: health.documents_indexed,
                can_build_index: backend.mode() == "embedded",
            })
        })
    }

    fn reindex(&self, path: &str) -> Result<(), String> {
        self.runtime.block_on(ops::reindex(&self.ctx, path)).map(|_| ()).map_err(|e| e.to_string())
    }

    fn build_index(&self) -> Result<u64, String> {
        self.runtime.block_on(async {
            let backend = self.ctx.backend().map_err(|e| e.to_string())?;
            backend.reindex_all().await.map_err(|e| e.to_string())?;
            backend.health().await.map(|h| h.documents_indexed).map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn bundle_defaults_preserve_explicit_cli_commands() {
        let app = Path::new("/Applications/Lapis.app/Contents/MacOS/lapis");
        let explicit = default_surface(crate::cli::Cli::try_parse_from(["lapis", "tui"]).unwrap(), app);
        assert!(matches!(explicit.command(), crate::cli::Command::Tui));
        let ordinary = default_surface(
            crate::cli::Cli::try_parse_from(["lapis"]).unwrap(),
            Path::new("/usr/local/bin/lapis"),
        );
        assert!(matches!(ordinary.command(), crate::cli::Command::Tui));
        let bundled = default_surface(crate::cli::Cli::try_parse_from(["lapis"]).unwrap(), app);
        assert_eq!(
            matches!(bundled.command(), crate::cli::Command::Desktop(_)),
            cfg!(all(target_os = "macos", feature = "desktop"))
        );
    }
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn fixture() -> (PathBuf, Service) {
        let root = std::env::temp_dir().join(format!(
            "lapis-desktop-io-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ops::Ctx {
            json: false,
            vault: crate::vault::Vault { root: root.clone(), source: "test" },
            cfg: crate::config::Config::default(),
            lattice_url: "http://127.0.0.1:9".into(),
            force_http: true,
        };
        let service = Service { ctx, runtime: tokio::runtime::Handle::current() };
        (root, service)
    }

    #[tokio::test]
    async fn empty_index_is_distinct_and_explicit_build_enables_canonical_search() {
        let (root, mut service) = fixture();
        service.ctx.force_http = false;
        std::fs::write(root.join("Needle.md"), "---\ntitle: Needle\n---\nA unique fixture note.\n").unwrap();
        let result = tokio::task::spawn_blocking(move || {
            let before = service.search("Needle").unwrap();
            assert_eq!(before.indexed_documents, 0);
            assert!(before.can_build_index);
            assert!(before.hits.is_empty());
            assert_eq!(service.build_index().unwrap(), 1);
            let after = service.search("Needle").unwrap();
            assert_eq!(after.indexed_documents, 1);
            assert_eq!(after.hits[0].path, "Needle.md");
        })
        .await;
        std::fs::remove_dir_all(root).unwrap();
        result.unwrap();
    }
    #[tokio::test]
    async fn yaml_remains_text_and_stale_save_keeps_agent_version() {
        let (root, service) = fixture();
        let path = root.join("settings.yaml");
        let original = "# comment\nunknown: [a, b]\ninvalid: [\n";
        std::fs::write(&path, original).unwrap();
        let document = service.read("settings.yaml").unwrap();
        let edited = "# comment\nunknown: [a, b]\ninvalid: [\n  # retained\n";
        let saved = service.save(&document, edited).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        std::fs::write(&path, "external agent version\n").unwrap();
        assert!(service.save(&saved, "local version").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external agent version\n");
        let copied = service.save_copy(&saved, "local version").unwrap();
        assert!(copied.path.ends_with("(Lapis copy 1).yaml"));
        assert_eq!(std::fs::read_to_string(root.join(&copied.path)).unwrap(), "local version");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external agent version\n");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn markdown_keeps_opaque_metadata_and_exact_body_boundary() {
        let (root, service) = fixture();
        let original = "---\ntitle: Test\ncustom: {keep: true}\n---\n\n# Body\n";
        std::fs::write(root.join("Note.md"), original).unwrap();
        let document = service.read("Note.md").unwrap();
        assert_eq!(document.text, "\n# Body\n");
        service.save(&document, "\n# Changed\n漢字").unwrap();
        let saved = std::fs::read_to_string(root.join("Note.md")).unwrap();
        assert!(saved.contains("custom: {keep: true}\n"));
        assert!(saved.ends_with("\n# Changed\n漢字\n"));
        assert!(service.read("../outside.md").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn directory_does_not_enter_links_outside_workspace() {
        let (root, service) = fixture();
        std::os::unix::fs::symlink(std::env::temp_dir(), root.join("outside")).unwrap();
        assert!(service.directory("outside").is_err());
        assert!(service.directory("").unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
