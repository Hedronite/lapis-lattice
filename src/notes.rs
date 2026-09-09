//! File IO for notes: vault-relative path resolution with escape rejection,
//! and building the `note` object (`schema/note.schema.json`).
//!
//! Never walks the tree. Never loads anything but the one requested file.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{LapisError, Result};
use crate::hal;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Markdown,
    Pdf,
    Html,
    Source,
}

pub fn kind_of(path: &str) -> Kind {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => Kind::Markdown,
        "pdf" => Kind::Pdf,
        "html" | "htm" => Kind::Html,
        _ => Kind::Source,
    }
}

/// Normalize a vault-relative path: POSIX separators, no leading `./`,
/// no `..`, not absolute. Returns the cleaned relative string.
pub fn clean_rel(rel: &str) -> Result<String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(LapisError::Usage("path is required".into()));
    }
    let p = Path::new(rel);
    if p.is_absolute() || rel.starts_with('/') || rel.starts_with('\\') {
        return Err(LapisError::Path(format!("path must be vault-relative, got absolute: {rel}")));
    }
    let mut parts: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => {
                let s = s.to_str().ok_or_else(|| LapisError::Path("non-UTF-8 path".into()))?;
                parts.push(s.to_string());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(LapisError::Path(format!("path escapes the vault: {rel}")));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(LapisError::Path(format!("path must be vault-relative: {rel}")));
            }
        }
    }
    if parts.is_empty() {
        return Err(LapisError::Path(format!("path resolves to the vault root: {rel}")));
    }
    Ok(parts.join("/"))
}

/// Resolve a vault-relative path to an absolute one inside `root`.
/// If the exact file is missing and the path has no extension, `.md` is tried
/// (so `foundry/lapis/SPEC` reads `foundry/lapis/SPEC.md`).
/// Errors with exit-3 semantics on escape or not-found.
pub fn resolve(root: &Path, rel: &str) -> Result<(String, PathBuf)> {
    let clean = clean_rel(rel)?;
    let abs = root.join(&clean);
    if abs.is_file() {
        return Ok((clean, abs));
    }
    if Path::new(&clean).extension().is_none() {
        let with_md = format!("{clean}.md");
        let abs_md = root.join(&with_md);
        if abs_md.is_file() {
            return Ok((with_md, abs_md));
        }
    }
    if abs.is_dir() {
        return Err(LapisError::Path(format!("{clean} is a directory, not a note")));
    }
    Err(LapisError::Path(format!("not found: {clean}")))
}

/// Note object per `schema/note.schema.json`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub path: String,
    pub kind: Kind,
    pub title: String,
    pub hal: Map<String, Value>,
    pub hal_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hal_error: Option<String>,
    pub tags: Vec<String>,
    pub body: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

pub fn read(root: &Path, rel: &str) -> Result<Note> {
    let (path, abs) = resolve(root, rel)?;
    let kind = kind_of(&path);
    let meta = std::fs::metadata(&abs)?;
    let size = meta.len();
    let updated_at =
        meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);

    match kind {
        Kind::Pdf => {
            // L7 wires pdf-extract / lattice chunks. Until then be explicit.
            return Err(LapisError::Usage(format!(
                "{path}: PDF read-through lands in a later slice; search already covers kind=pdf"
            )));
        }
        Kind::Markdown | Kind::Html | Kind::Source => {}
    }

    let bytes = std::fs::read(&abs)?;
    let text = String::from_utf8(bytes).map_err(|_| LapisError::Usage(format!("{path}: not UTF-8 text")))?;

    let (hal, hal_valid, hal_error, body) = if kind == Kind::Markdown {
        let p = hal::parse(&text);
        (p.hal, p.hal_valid, p.error, p.body)
    } else {
        (Map::new(), true, None, text)
    };

    let title = hal::title_from_hal(&hal)
        .or_else(|| if kind == Kind::Markdown { first_heading(&body) } else { None })
        .unwrap_or_else(|| stem_of(&path));
    let tags = hal::tags_from_hal(&hal);

    Ok(Note { path, kind, title, hal, hal_valid, hal_error, tags, body, size, updated_at })
}

/// First H1 text in a Markdown body, via pulldown-cmark's event stream.
pub fn first_heading(body: &str) -> Option<String> {
    let parser = Parser::new_ext(body, Options::empty());
    let mut in_h1 = false;
    let mut buf = String::new();
    for ev in parser {
        match ev {
            Event::Start(Tag::Heading { level: HeadingLevel::H1, .. }) => in_h1 = true,
            Event::End(TagEnd::Heading(HeadingLevel::H1)) if in_h1 => {
                let t = buf.trim().to_string();
                return if t.is_empty() { None } else { Some(t) };
            }
            Event::Text(t) | Event::Code(t) if in_h1 => buf.push_str(&t),
            _ => {}
        }
    }
    None
}

pub fn stem_of(path: &str) -> String {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_vault() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lapis-notes-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(dir.join("foundry/lapis")).unwrap();
        std::fs::write(
            dir.join("foundry/lapis/SPEC.md"),
            "---\nname: Lapis · SPEC\ntags: [a]\n---\n<!--hal:authoritative:yaml-->\n\n# SPEC.md\n\nbody\n",
        )
        .unwrap();
        std::fs::write(dir.join("foundry/lapis/plain.md"), "# Plain Title\n\ntext\n").unwrap();
        std::fs::write(dir.join("foundry/lapis/bad.md"), "---\nname: [oops\n---\nstill here\n").unwrap();
        std::fs::write(dir.join("foundry/lapis/notes.txt"), "just text").unwrap();
        std::fs::write(dir.join("foundry/lapis/tome.pdf"), b"%PDF-1.4").unwrap();
        dir
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn clean_rel_rejects_escape_and_absolute() {
        assert!(matches!(clean_rel("../etc/passwd"), Err(LapisError::Path(_))));
        assert!(matches!(clean_rel("a/../../b"), Err(LapisError::Path(_))));
        assert!(matches!(clean_rel("/etc/passwd"), Err(LapisError::Path(_))));
        assert!(matches!(clean_rel(""), Err(LapisError::Usage(_))));
        assert!(matches!(clean_rel("."), Err(LapisError::Path(_))));
        assert_eq!(clean_rel("./a/./b.md").unwrap(), "a/b.md");
        assert_eq!(clean_rel("a/b.md").unwrap(), "a/b.md");
    }

    #[test]
    fn exit_codes_for_path_errors_are_3() {
        assert_eq!(clean_rel("../x").unwrap_err().exit_code(), 3);
        let v = tmp_vault();
        assert_eq!(resolve(&v, "missing.md").unwrap_err().exit_code(), 3);
        assert_eq!(resolve(&v, "foundry").unwrap_err().exit_code(), 3);
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn resolve_adds_md_when_missing_extension() {
        let v = tmp_vault();
        let (rel, abs) = resolve(&v, "foundry/lapis/SPEC").unwrap();
        assert_eq!(rel, "foundry/lapis/SPEC.md");
        assert!(abs.ends_with("foundry/lapis/SPEC.md"));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_returns_hal_and_body() {
        let v = tmp_vault();
        let n = read(&v, "foundry/lapis/SPEC.md").unwrap();
        assert_eq!(n.title, "Lapis · SPEC");
        assert!(n.hal_valid);
        assert_eq!(n.hal["name"], "Lapis · SPEC");
        assert_eq!(n.tags, ["a"]);
        assert!(n.body.starts_with("<!--hal:authoritative:yaml-->"));
        assert_eq!(n.kind, Kind::Markdown);
        assert!(n.size > 0);
        assert!(n.updated_at.is_some());
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_falls_back_to_h1_then_stem() {
        let v = tmp_vault();
        assert_eq!(read(&v, "foundry/lapis/plain.md").unwrap().title, "Plain Title");
        let n = read(&v, "foundry/lapis/notes.txt").unwrap();
        assert_eq!(n.title, "notes");
        assert_eq!(n.kind, Kind::Source);
        assert_eq!(n.body, "just text");
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_bad_yaml_is_hal_invalid_with_body() {
        let v = tmp_vault();
        let n = read(&v, "foundry/lapis/bad.md").unwrap();
        assert!(!n.hal_valid);
        assert!(n.hal_error.is_some());
        assert_eq!(n.body, "still here");
        assert_eq!(n.title, "bad");
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_pdf_is_deferred_not_crash() {
        let v = tmp_vault();
        let e = read(&v, "foundry/lapis/tome.pdf").unwrap_err();
        assert_eq!(e.exit_code(), 1);
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a/b.md"), Kind::Markdown);
        assert_eq!(kind_of("a/b.PDF"), Kind::Pdf);
        assert_eq!(kind_of("a/b.html"), Kind::Html);
        assert_eq!(kind_of("a/b.rs"), Kind::Source);
    }
}
