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
    Yaml,
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
        "yaml" | "yml" => Kind::Yaml,
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
/// (so `notes/SPEC` reads `notes/SPEC.md`).
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
    /// Content hash of the file bytes (`fnv1a64:<hex>`); pass back as `--if-hash`.
    pub hash: String,
    /// PDF only: page count of the extracted text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<usize>,
}

pub fn read(root: &Path, rel: &str) -> Result<Note> {
    read_impl(root, rel, false)
}

/// Read-only presentation for terminal references; CLI/MCP continue using `read`.
pub fn read_for_tui(root: &Path, rel: &str) -> Result<Note> {
    read_impl(root, rel, true)
}

fn read_impl(root: &Path, rel: &str, display: bool) -> Result<Note> {
    let (path, abs) = resolve(root, rel)?;
    let kind = kind_of(&path);
    let meta = std::fs::metadata(&abs)?;
    let size = meta.len();
    let updated_at =
        meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64);

    if kind == Kind::Pdf {
        // Read-through with pdf-extract (pure Rust, MIT). The lattice's
        // tome_indexer.py owns PDF *indexing*; this is the on-demand text
        // for `read` / MCP read_note, never a second crawler.
        let (body, pages) = if display {
            pdf_extract::extract_text_by_pages(&abs).map(|pages| (display_pdf_pages(&pages), pages.len()))
        } else {
            pdf_text(&abs)
        }
        .map_err(|e| LapisError::Usage(format!("{path}: pdf: {e}")))?;
        let hash = std::fs::read(&abs).map(|b| content_hash(&b)).unwrap_or_default();
        return Ok(Note {
            title: stem_of(&path),
            path,
            kind,
            hal: Map::new(),
            hal_valid: true,
            hal_error: None,
            tags: Vec::new(),
            body,
            size,
            updated_at,
            hash,
            pages: Some(pages),
        });
    }

    let bytes = std::fs::read(&abs)?;
    let hash = content_hash(&bytes);
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
    let body = if display && kind == Kind::Html {
        // Keep logical lines unwrapped; the reader owns terminal cell geometry.
        let text = lapis_desktop::html::to_text(&body, usize::MAX);
        if text.trim().is_empty() { "This HTML file has no readable static content.".into() } else { text }
    } else {
        body
    };

    Ok(Note { path, kind, title, hal, hal_valid, hal_error, tags, body, size, updated_at, hash, pages: None })
}

/// FNV-1a 64 over the raw bytes. Not cryptographic: a cheap stale-write guard
/// (`--if-hash`) that agents can round-trip from `read`.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}")
}

/// Millisecond mtime of a file, as `read` reports in `updatedAt`.
pub fn mtime_ms(abs: &Path) -> Option<u64> {
    std::fs::metadata(abs)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

/// One `#` section: heading text, level, and the body under it (up to the
/// next heading of the same or a higher level). Section 0 is the preamble
/// before the first heading, with an empty heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub level: u8,
    pub text: String,
}

fn heading_of(line: &str) -> Option<(u8, &str)> {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    if !rest.starts_with(' ') && !rest.is_empty() {
        return None;
    }
    Some((hashes as u8, rest.trim().trim_end_matches('#').trim()))
}

/// Split a Markdown body into heading sections (fenced code is not parsed as headings).
pub fn sections(body: &str) -> Vec<Section> {
    let mut out: Vec<Section> = vec![Section { heading: String::new(), level: 0, text: String::new() }];
    let mut in_fence = false;
    for line in body.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        }
        if !in_fence && let Some((level, h)) = heading_of(line) {
            out.push(Section { heading: h.to_string(), level, text: String::new() });
            continue;
        }
        let cur = out.last_mut().unwrap();
        cur.text.push_str(line);
        cur.text.push('\n');
    }
    for s in &mut out {
        s.text = s.text.trim().to_string();
    }
    if out.len() > 1 && out[0].text.is_empty() {
        out.remove(0);
    }
    out
}

/// Body under heading `h` (case-insensitive, `#` prefix optional) including
/// its nested sub-sections, or `None`.
pub fn section(body: &str, h: &str) -> Option<String> {
    let want = h.trim().trim_start_matches('#').trim().to_lowercase();
    let secs = sections(body);
    let i = secs.iter().position(|s| s.heading.to_lowercase() == want)?;
    let level = secs[i].level;
    let mut text = format!("{} {}\n\n{}", "#".repeat(level as usize), secs[i].heading, secs[i].text);
    for s in &secs[i + 1..] {
        if s.level <= level {
            break;
        }
        text.push_str(&format!("\n\n{} {}\n\n{}", "#".repeat(s.level as usize), s.heading, s.text));
    }
    Some(text.trim().to_string())
}

/// `read --heading / --chunk / --max-chars` on an already-read note: the body
/// slice and whether it was clipped. Missing heading / chunk is exit 3.
pub fn excerpt(
    note: &Note,
    heading: Option<&str>,
    chunk: Option<usize>,
    max_chars: Option<usize>,
) -> Result<(String, bool)> {
    let body = if let Some(h) = heading {
        section(&note.body, h).ok_or_else(|| LapisError::Path(format!("{}: no heading {h:?}", note.path)))?
    } else if let Some(n) = chunk {
        let secs = sections(&note.body);
        let s = secs.get(n).ok_or_else(|| {
            LapisError::Path(format!("{}: no chunk #{n} ({} sections)", note.path, secs.len()))
        })?;
        if s.level == 0 {
            s.text.clone()
        } else {
            format!("{} {}\n\n{}", "#".repeat(s.level as usize), s.heading, s.text)
        }
    } else {
        note.body.clone()
    };
    Ok(match max_chars {
        Some(m) => clip(&body, m),
        None => (body, false),
    })
}

/// Clip to `max` chars on a char boundary. Returns `(text, truncated)`.
pub fn clip(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() <= max {
        return (s.to_string(), false);
    }
    (s.chars().take(max).collect(), true)
}

/// Extracted text and page count. Pages are joined with a blank line; runs
/// of whitespace inside a page are left alone so layout stays greppable.
pub fn pdf_text(abs: &Path) -> std::result::Result<(String, usize), pdf_extract::OutputError> {
    let pages = pdf_extract::extract_text_by_pages(abs)?;
    let n = pages.len();
    let body = pages.iter().map(|p| p.trim_end()).collect::<Vec<_>>().join("\n\n");
    Ok((body.trim().to_string(), n))
}

fn display_pdf_pages(pages: &[String]) -> String {
    if pages.is_empty() {
        return "This PDF has no readable pages.".into();
    }
    pages
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let text = text.trim();
            let content = if text.is_empty() {
                "No extractable text on this page. Read the page image in the desktop workspace."
            } else {
                text
            };
            format!("Page {} of {}\n\n{content}", index + 1, pages.len())
        })
        .collect::<Vec<_>>()
        .join("\n\n────────────────────\n\n")
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
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(
            dir.join("notes/SPEC.md"),
            "---\nname: Lapis · SPEC\ntags: [a]\n---\n<!--hal:authoritative:yaml-->\n\n# SPEC.md\n\nbody\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes/plain.md"), "# Plain Title\n\ntext\n").unwrap();
        std::fs::write(dir.join("notes/bad.md"), "---\nname: [oops\n---\nstill here\n").unwrap();
        std::fs::write(dir.join("notes/notes.txt"), "just text").unwrap();
        std::fs::write(dir.join("notes/tome.pdf"), b"%PDF-1.4").unwrap();
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
        let (rel, abs) = resolve(&v, "notes/SPEC").unwrap();
        assert_eq!(rel, "notes/SPEC.md");
        assert!(abs.ends_with("notes/SPEC.md"));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_returns_hal_and_body() {
        let v = tmp_vault();
        let n = read(&v, "notes/SPEC.md").unwrap();
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
        assert_eq!(read(&v, "notes/plain.md").unwrap().title, "Plain Title");
        let n = read(&v, "notes/notes.txt").unwrap();
        assert_eq!(n.title, "notes");
        assert_eq!(n.kind, Kind::Source);
        assert_eq!(n.body, "just text");
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_bad_yaml_is_hal_invalid_with_body() {
        let v = tmp_vault();
        let n = read(&v, "notes/bad.md").unwrap();
        assert!(!n.hal_valid);
        assert!(n.hal_error.is_some());
        assert_eq!(n.body, "still here");
        assert_eq!(n.title, "bad");
        let _ = std::fs::remove_dir_all(&v);
    }

    /// A minimal single-page PDF with a Helvetica text object and a correct xref.
    fn tiny_pdf(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({text}) Tj ET");
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
        }
        let xref = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
        for off in offsets {
            out.push_str(&format!("{off:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        ));
        out.into_bytes()
    }

    #[test]
    fn terminal_references_show_pages_without_changing_cli_content() {
        let root = tmp_vault();
        for (name, content) in [("text.pdf", "Literal **stars**"), ("empty.pdf", "")] {
            std::fs::write(root.join(name), tiny_pdf(content)).unwrap();
            let cli = read(&root, name).unwrap();
            let tui = read_for_tui(&root, name).unwrap();
            assert_eq!(cli.body, content);
            assert_eq!(tui.hash, cli.hash);
            assert_eq!(tui.pages, Some(1));
            assert!(tui.body.starts_with("Page 1 of 1\n\n"));
            if content.is_empty() {
                assert!(tui.body.contains("No extractable text"));
            } else {
                assert!(tui.body.ends_with(content));
            }
        }
        let html = "<h1>Read &amp; retain</h1><p>A <b>reference</b>.</p><script>privateScript()</script><pre>  x: 1\n  y: 2</pre>";
        std::fs::write(root.join("reference.html"), html).unwrap();
        assert_eq!(read(&root, "reference.html").unwrap().body, html);
        let body = read_for_tui(&root, "reference.html").unwrap().body;
        assert!(body.contains("Read & retain"));
        assert!(body.contains("  x: 1\n  y: 2"));
        assert!(!body.contains("privateScript"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_pdf_extracts_text() {
        let v = tmp_vault();
        std::fs::write(v.join("notes/real.pdf"), tiny_pdf("Hello Lapis")).unwrap();
        let n = read(&v, "notes/real.pdf").unwrap();
        assert_eq!(n.kind, Kind::Pdf);
        assert_eq!(n.title, "real");
        assert_eq!(n.pages, Some(1));
        assert!(n.hal_valid && n.hal.is_empty());
        assert!(n.body.contains("Hello Lapis"), "body was: {:?}", n.body);
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn read_garbage_pdf_is_a_clean_error() {
        let v = tmp_vault();
        let e = read(&v, "notes/tome.pdf").unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("pdf"));
        let _ = std::fs::remove_dir_all(&v);
    }

    /// N11: heading / chunk slicing and clipping are pure on the body.
    #[test]
    fn sections_heading_chunk_and_clip() {
        let body = "intro line\n\n# One\n\ntext one\n\n## One.a\n\nnested\n\n```\n# not a heading\n```\n\n# Two\n\ntext two\n";
        let s = sections(body);
        let heads: Vec<(&str, u8)> = s.iter().map(|x| (x.heading.as_str(), x.level)).collect();
        assert_eq!(heads, [("", 0), ("One", 1), ("One.a", 2), ("Two", 1)]);
        assert_eq!(s[0].text, "intro line");
        assert!(s[2].text.contains("# not a heading"), "fenced hash stays in the section");
        assert_eq!(s[3].text, "text two");
        let one = section(body, "one").unwrap();
        assert!(one.starts_with("# One"));
        assert!(one.contains("## One.a") && one.contains("nested"));
        assert!(!one.contains("text two"));
        assert_eq!(section(body, "## Two").unwrap(), "# Two\n\ntext two");
        assert!(section(body, "Three").is_none());
        let note = Note {
            path: "n.md".into(),
            kind: Kind::Markdown,
            title: "n".into(),
            hal: Map::new(),
            hal_valid: true,
            hal_error: None,
            tags: vec![],
            body: body.to_string(),
            size: 0,
            updated_at: None,
            hash: String::new(),
            pages: None,
        };
        assert_eq!(excerpt(&note, None, Some(0), None).unwrap(), ("intro line".to_string(), false));
        assert!(excerpt(&note, None, Some(3), None).unwrap().0.starts_with("# Two"));
        assert_eq!(excerpt(&note, None, Some(9), None).unwrap_err().exit_code(), 3);
        assert_eq!(excerpt(&note, Some("nope"), None, None).unwrap_err().exit_code(), 3);
        let (t, trunc) = excerpt(&note, Some("Two"), None, Some(5)).unwrap();
        assert_eq!((t.as_str(), trunc), ("# Two", true));
        assert_eq!(clip("héllo", 3), ("hél".to_string(), true));
        assert_eq!(clip("hi", 3), ("hi".to_string(), false));
        assert!(content_hash(b"a") != content_hash(b"b"));
        assert_eq!(content_hash(b"").len(), "fnv1a64:".len() + 16);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a/b.md"), Kind::Markdown);
        assert_eq!(kind_of("a/b.PDF"), Kind::Pdf);
        assert_eq!(kind_of("a/b.html"), Kind::Html);
        assert_eq!(kind_of("a/b.rs"), Kind::Source);
    }
}
