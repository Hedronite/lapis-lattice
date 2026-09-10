//! The write path: create, append, capture.
//!
//! Files are the write source of truth. Every function here writes exactly
//! one file under the vault root and returns its vault-relative path; the
//! caller kicks `POST /reindex`. Nothing here touches `lattice.db`.
//!
//! HAL rules (SPEC.md § HAL): create stamps the create-set; later edits only
//! touch allowlisted keys, line-wise, so unknown keys and comments survive.

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::{LapisError, Result};
use crate::hal;
use crate::notes;
use crate::taxonomy;
use crate::templates;

pub const HAL_VERSION: &str = "1.0";
pub const AUTHORITATIVE_MARKER: &str = "<!--hal:authoritative:yaml-->";
/// Types that get the authoritative marker after the closing `---`.
const CANON_TYPES: &[&str] =
    &["foundry-doc", "foundry-product", "status", "cross-reference", "maintenance-canon"];

#[derive(Debug, Clone, Default)]
pub struct CreateOpts {
    pub title: String,
    /// Vault-relative file or directory. Directory when it ends with `/` or exists as one.
    pub path: Option<String>,
    /// Name of `.lapis/templates/<name>.md`.
    pub template: Option<String>,
    pub doc_type: Option<String>,
    pub domain: Option<String>,
    pub tags: Vec<String>,
    pub body: Option<String>,
    pub operator: Option<String>,
    /// Default bucket when `path` is absent (overlay `buckets.inbox`).
    pub inbox: String,
    /// Optional `{{name}}` placeholder for templates (default: operator).
    pub director: Option<String>,
    /// Date the template placeholders refer to (`{{date}}`, `{{week}}`,
    /// `{{month}}`); default today. `created`/`updated` are always today.
    pub template_date: Option<jiff::civil::Date>,
    /// Compute everything, write nothing.
    pub dry_run: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Written {
    pub path: String,
    pub title: String,
    pub hal: Map<String, Value>,
    pub bytes: u64,
    pub taxonomy: &'static str,
    /// `true` when nothing touched the disk (`--dry-run`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    /// Full text that was (or would be) written; only on dry runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

pub fn today() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d").to_string()
}

pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "note".to_string() } else { out }
}

pub const DEFAULT_INIT_VAULT: &str = "~/Notes";
const VAULT_GITIGNORE: &str = ".lapis/\nlattice.sqlite*\n*.duckdb\n";
const WELCOME_MD: &str = "---\nname: Welcome\nstatus: draft\ntags: []\n---\n# Welcome\n\nThis is your vault. Notes are ordinary Markdown files.\n\n- Open the TUI: `lapis --vault .`\n- Search (needs a lattice): `lapis search --json welcome`\n- MCP: `lapis mcp`\n";

#[derive(Debug, Clone, serde::Serialize)]
pub struct InitResult {
    pub path: String,
    pub created: bool,
}

/// Create a vault directory. Idempotent: existing files are left alone.
pub fn init_vault(raw: Option<&str>) -> Result<InitResult> {
    let raw = raw.unwrap_or(DEFAULT_INIT_VAULT);
    let root = crate::config::expand_tilde(raw);
    if root.exists() && !root.is_dir() {
        return Err(LapisError::Usage(format!("{} exists and is not a directory", root.display())));
    }
    let created = !root.exists();
    std::fs::create_dir_all(root.join("Daily"))?;
    std::fs::create_dir_all(root.join("inbox"))?;
    std::fs::create_dir_all(root.join(".lapis"))?;
    let gi = root.join(".gitignore");
    if !gi.exists() {
        std::fs::write(&gi, VAULT_GITIGNORE)?;
    }
    let welcome = root.join("Welcome.md");
    if !welcome.exists() {
        std::fs::write(&welcome, WELCOME_MD)?;
    }
    Ok(InitResult { path: root.display().to_string(), created })
}

/// Decide the vault-relative file path for a new note. Never overwrites.
fn target_path(root: &Path, opts: &CreateOpts) -> Result<String> {
    let file = format!("{}.md", slug(&opts.title));
    let rel = match opts.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        None => format!("{}/{}", opts.inbox.trim_matches('/'), file),
        Some(p) => {
            let is_dir = p.ends_with('/') || root.join(notes::clean_rel(p)?).is_dir();
            let clean = notes::clean_rel(p)?;
            if is_dir {
                format!("{clean}/{file}")
            } else if clean.ends_with(".md") {
                clean
            } else {
                format!("{clean}.md")
            }
        }
    };
    if !rel.contains('/') {
        // No Welcome.md-style root drops: a note needs a folder (SPEC D-BUCKETS).
        return Err(LapisError::Usage(format!(
            "refusing to create at the vault root: {rel} (use --path <folder>/)"
        )));
    }
    if root.join(&rel).exists() {
        return Err(LapisError::Usage(format!("already exists: {rel}")));
    }
    Ok(rel)
}

/// Build the create-set mapping in canonical key order.
fn create_set(rel: &str, opts: &CreateOpts, root: &Path) -> (Map<String, Value>, &'static str) {
    let class = taxonomy::classify(root, rel, opts.doc_type.as_deref());
    let doc_type = opts.doc_type.clone().or(class.doc_type.clone()).unwrap_or_else(|| "note".to_string());
    let domain = opts.domain.clone().or(class.domain.clone());
    let today = today();
    let mut m = Map::new();
    m.insert("name".into(), Value::String(opts.title.clone()));
    m.insert("type".into(), Value::String(doc_type.clone()));
    m.insert("doc_type".into(), Value::String(doc_type));
    if let Some(d) = domain {
        m.insert("domain".into(), Value::String(d));
    }
    m.insert("status".into(), Value::String("draft".into()));
    m.insert("created".into(), Value::String(today.clone()));
    m.insert("updated".into(), Value::String(today));
    m.insert("hal_version".into(), Value::String(HAL_VERSION.into()));
    if let Some(op) = opts.operator.as_deref().filter(|s| !s.trim().is_empty()) {
        m.insert("operator".into(), Value::String(op.to_string()));
    }
    m.insert("tags".into(), Value::Array(opts.tags.iter().map(|t| Value::String(t.clone())).collect()));
    (m, class.source)
}

/// Serialize a mapping as YAML frontmatter text (no delimiters).
pub fn to_yaml(m: &Map<String, Value>) -> Result<String> {
    let v = serde_json::Value::Object(m.clone());
    let y: yaml_serde::Value =
        yaml_serde::to_value(&v).map_err(|e| LapisError::Internal(format!("yaml: {e}")))?;
    let s = yaml_serde::to_string(&y).map_err(|e| LapisError::Internal(format!("yaml: {e}")))?;
    Ok(s.strip_prefix("---\n").unwrap_or(&s).trim_end().to_string())
}

/// Resolve a template (built-in or `.lapis/templates/<name>.md`), substitute
/// placeholders, split HAL.
fn load_template(
    root: &Path,
    name: &str,
    title: &str,
    director: Option<&str>,
    date: Option<jiff::civil::Date>,
) -> Result<hal::Parsed> {
    let t = templates::find(root, name)?;
    let vars = template_vars_at(title, director, date.unwrap_or_else(|| jiff::Zoned::now().date()));
    Ok(hal::parse(&templates::substitute(&t.text, &vars)))
}

pub fn template_vars_at(title: &str, director: Option<&str>, day: jiff::civil::Date) -> templates::Vars {
    templates::Vars {
        title: title.to_string(),
        date: day.strftime("%Y-%m-%d").to_string(),
        week: iso_week(&day),
        month: day.strftime("%Y-%m").to_string(),
        director: director.unwrap_or("").to_string(),
    }
}

/// ISO week label `YYYY-Www` for a date.
pub fn iso_week(d: &jiff::civil::Date) -> String {
    let w = d.iso_week_date();
    format!("{}-W{:02}", w.year(), w.week())
}

fn is_canon(doc_type: &str) -> bool {
    CANON_TYPES.contains(&doc_type)
}

/// Stale-write guard and dry-run switch shared by append / toggle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guard {
    pub dry_run: bool,
    /// Refuse unless the file's mtime (ms, as `read` reports `updatedAt`) still matches.
    pub if_mtime: Option<u64>,
    /// Refuse unless the file's content hash (as `read` reports `hash`) still matches.
    pub if_hash: Option<String>,
}

impl Guard {
    /// Usage error (exit 1) when the note changed since the agent read it.
    pub fn check(&self, rel: &str, abs: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(want) = self.if_mtime {
            let have = notes::mtime_ms(abs).unwrap_or(0);
            if have != want {
                return Err(LapisError::Usage(format!("{rel}: stale: mtime {have} != expected {want}")));
            }
        }
        if let Some(want) = &self.if_hash {
            let have = notes::content_hash(bytes);
            if &have != want {
                return Err(LapisError::Usage(format!("{rel}: stale: hash {have} != expected {want}")));
            }
        }
        Ok(())
    }
}

fn write_atomic(abs: &Path, text: &str) -> Result<u64> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = abs.with_extension("md.lapis-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, abs)?;
    Ok(text.len() as u64)
}

pub fn create(root: &Path, opts: &CreateOpts) -> Result<Written> {
    if opts.title.trim().is_empty() {
        return Err(LapisError::Usage("--title is required".into()));
    }
    let rel = target_path(root, opts)?;
    let (mut hal, tax_source) = create_set(&rel, opts, root);

    let mut body = opts.body.clone().unwrap_or_default();
    if let Some(name) = &opts.template {
        let director = opts.director.clone().or_else(|| opts.operator.clone());
        let t = load_template(root, name, &opts.title, director.as_deref(), opts.template_date)?;
        if !t.hal_valid {
            return Err(LapisError::Usage(format!("template {name}: invalid YAML frontmatter")));
        }
        // Template keys lay over the create-set; unknown keys are kept.
        // `type` and `doc_type` are one fact: a template setting either sets both.
        let has_type = t.hal.contains_key("type");
        let has_doc_type = t.hal.contains_key("doc_type");
        for (k, v) in t.hal {
            hal.insert(k, v);
        }
        match (has_type, has_doc_type) {
            (true, false) => {
                let v = hal["type"].clone();
                hal.insert("doc_type".into(), v);
            }
            (false, true) => {
                let v = hal["doc_type"].clone();
                hal.insert("type".into(), v);
            }
            _ => {}
        }
        if body.is_empty() {
            body = t.body;
        } else {
            body = format!("{}\n\n{}", t.body.trim_end(), body);
        }
    }
    if body.trim().is_empty() {
        body = format!("# {}\n", opts.title);
    }
    let doc_type = hal.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let marker = if is_canon(&doc_type) { format!("{AUTHORITATIVE_MARKER}\n\n") } else { String::new() };
    let text = format!("---\n{}\n---\n{marker}{}\n", to_yaml(&hal)?, body.trim_end());
    if opts.dry_run {
        let bytes = text.len() as u64;
        return Ok(Written {
            path: rel,
            title: opts.title.clone(),
            hal,
            bytes,
            taxonomy: tax_source,
            dry_run: true,
            text: Some(text),
        });
    }
    let bytes = write_atomic(&root.join(&rel), &text)?;
    Ok(Written {
        path: rel,
        title: opts.title.clone(),
        hal,
        bytes,
        taxonomy: tax_source,
        dry_run: false,
        text: None,
    })
}

/// Append `text` to an existing note and bump `updated:` (allowlisted), with
/// a stale guard and dry-run switch (N13).
pub fn append_with(root: &Path, rel: &str, text: &str, guard: &Guard) -> Result<Written> {
    let (rel, abs) = notes::resolve(root, rel)?;
    if notes::kind_of(&rel) != notes::Kind::Markdown {
        return Err(LapisError::Usage(format!("{rel}: append only supports markdown notes")));
    }
    let raw = std::fs::read(&abs)?;
    guard.check(&rel, &abs, &raw)?;
    let mut current =
        String::from_utf8(raw).map_err(|_| LapisError::Usage(format!("{rel}: not UTF-8 text")))?;
    current = set_frontmatter_key(&current, "updated", &today());
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    if !current.trim_end().ends_with("\n---") && !current.ends_with("\n\n") {
        current.push('\n');
    }
    current.push_str(text.trim_end());
    current.push('\n');
    let bytes = if guard.dry_run { current.len() as u64 } else { write_atomic(&abs, &current)? };
    let p = hal::parse(&current);
    let title = hal::title_from_hal(&p.hal).unwrap_or_else(|| notes::stem_of(&rel));
    Ok(Written {
        path: rel,
        title,
        hal: p.hal,
        bytes,
        taxonomy: "unchanged",
        dry_run: guard.dry_run,
        text: if guard.dry_run { Some(current) } else { None },
    })
}

/// Line-wise edit of one top-level scalar key inside the frontmatter block.
/// Preserves every other line byte-for-byte (unknown keys, comments).
/// No frontmatter → unchanged. Key absent → inserted before the closing `---`.
pub fn set_frontmatter_key(text: &str, key: &str, value: &str) -> String {
    let Some(rest) = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n")) else {
        return text.to_string();
    };
    let Some(end) = rest.find("\n---").map(|i| i + 1) else {
        return text.to_string();
    };
    let (matter, tail) = rest.split_at(end);
    let prefix = format!("{key}:");
    let mut lines: Vec<String> = matter.lines().map(str::to_string).collect();
    if let Some(l) = lines.iter_mut().find(|l| l.starts_with(&prefix)) {
        *l = format!("{key}: {value}");
    } else {
        lines.push(format!("{key}: {value}"));
    }
    format!("---\n{}\n{}", lines.join("\n"), tail)
}

pub fn capture(root: &Path, text: &str, inbox: &str, operator: Option<String>) -> Result<Written> {
    let text = text.trim();
    if text.is_empty() {
        return Err(LapisError::Usage("capture needs some text".into()));
    }
    let first = text.lines().next().unwrap_or(text).trim_start_matches('#').trim();
    let title: String = first.chars().take(80).collect();
    let stamp = jiff::Zoned::now().strftime("%Y-%m-%d-%H%M").to_string();
    let file = format!("{}/{stamp}-{}.md", inbox.trim_matches('/'), slug(&title));
    let opts = CreateOpts {
        title,
        path: Some(file),
        template: None,
        doc_type: Some("capture".into()),
        domain: None,
        tags: vec![],
        body: Some(text.to_string()),
        operator,
        inbox: inbox.to_string(),
        director: None,
        template_date: None,
        dry_run: false,
    };
    create(root, &opts)
}

/// Result of `daily`: the note path and whether this call created it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Periodic {
    pub path: String,
    pub created: bool,
    pub date: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
}

/// Open-or-create the periodic note for `date` (default today) using the
/// matching built-in template: `Daily/YYYY-MM-DD.md`, `Weekly/YYYY-Www.md`,
/// `Monthly/YYYY-MM.md`. HAL create-set, no authoritative marker.
pub fn periodic(
    root: &Path,
    period: Period,
    date: Option<&str>,
    operator: Option<String>,
) -> Result<Periodic> {
    let day = match date {
        Some(d) => jiff::civil::Date::strptime("%Y-%m-%d", d)
            .map_err(|_| LapisError::Usage(format!("date must be YYYY-MM-DD: {d}")))?,
        None => jiff::Zoned::now().date(),
    };
    let (label, rel, template) = match period {
        Period::Daily => {
            let d = day.strftime("%Y-%m-%d").to_string();
            (d.clone(), format!("Daily/{d}.md"), "builtin.daily")
        }
        Period::Weekly => {
            let w = iso_week(&day);
            (w.clone(), format!("Weekly/{w}.md"), "builtin.weekly")
        }
        Period::Monthly => {
            let m = day.strftime("%Y-%m").to_string();
            (m.clone(), format!("Monthly/{m}.md"), "builtin.monthly")
        }
    };
    if root.join(&rel).is_file() {
        return Ok(Periodic { path: rel, created: false, date: label });
    }
    let opts = CreateOpts {
        title: label.clone(),
        path: Some(rel),
        template: Some(template.into()),
        doc_type: None,
        domain: None,
        tags: vec![],
        body: None,
        operator,
        inbox: "inbox".into(),
        director: None,
        template_date: Some(day),
        dry_run: false,
    };
    let w = create(root, &opts)?;
    Ok(Periodic { path: w.path, created: true, date: label })
}

/// Move a trashed note back to its original vault-relative path.
/// `trashed_rel` is the path under the trash bucket (as `trash` reported).
pub fn restore(root: &Path, trashed_rel: &str, trash_bucket: &str) -> Result<Trashed> {
    let (trashed_rel, abs) = notes::resolve(root, trashed_rel)?;
    let bucket = trash_bucket.trim_matches('/');
    let Some(orig) = trashed_rel.strip_prefix(&format!("{bucket}/")) else {
        return Err(LapisError::Usage(format!("not in trash ({bucket}/): {trashed_rel}")));
    };
    // Strip a `.YYYYMMDD-HHMMSS` collision stamp if trash added one.
    let orig = match orig.rsplit_once('.') {
        Some((head, "md")) => match head.rsplit_once('.') {
            Some((h, stamp))
                if stamp.len() == 15 && stamp.chars().all(|c| c.is_ascii_digit() || c == '-') =>
            {
                format!("{h}.md")
            }
            _ => orig.to_string(),
        },
        _ => orig.to_string(),
    };
    let dest = root.join(&orig);
    if dest.exists() {
        return Err(LapisError::Usage(format!("cannot restore, target exists: {orig}")));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&abs, &dest)?;
    Ok(Trashed { path: orig, trashed_to: trashed_rel })
}

/// Every note currently in the trash bucket (vault-relative, sorted).
pub fn trash_list(root: &Path, trash_bucket: &str) -> Vec<String> {
    let base = root.join(trash_bucket.trim_matches('/'));
    let mut out = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "md")
                && let Ok(rel) = p.strip_prefix(root)
            {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trashed {
    pub path: String,
    pub trashed_to: String,
}

/// Move a note into the trash bucket (default `.lapis/trash/`), keeping its
/// vault-relative path underneath so it can be restored. Never deletes.
pub fn trash(root: &Path, rel: &str, trash_bucket: &str) -> Result<Trashed> {
    let (rel, abs) = notes::resolve(root, rel)?;
    let bucket = trash_bucket.trim_matches('/');
    if rel.starts_with(&format!("{bucket}/")) {
        return Err(LapisError::Usage(format!("already in trash: {rel}")));
    }
    let mut dest_rel = format!("{bucket}/{rel}");
    if root.join(&dest_rel).exists() {
        let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
        dest_rel = format!("{bucket}/{}.{stamp}.md", rel.trim_end_matches(".md"));
    }
    let dest = root.join(&dest_rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&abs, &dest)?;
    Ok(Trashed { path: rel, trashed_to: dest_rel })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn weekly_monthly_and_restore() {
        let v = vault();
        let w = periodic(&v, Period::Weekly, Some("2026-09-09"), None).unwrap();
        assert_eq!(w.path, "Weekly/2026-W37.md");
        let n = notes::read(&v, &w.path).unwrap();
        assert_eq!(n.hal["doc_type"], "weekly-note");
        assert!(n.body.contains("# Week 2026-W37"));
        let m = periodic(&v, Period::Monthly, Some("2026-09-09"), None).unwrap();
        assert_eq!(m.path, "Monthly/2026-09.md");
        assert_eq!(notes::read(&v, &m.path).unwrap().hal["doc_type"], "monthly-note");
        // trash then restore round-trips the original path
        let t = trash(&v, &m.path, ".lapis/trash").unwrap();
        assert!(trash_list(&v, ".lapis/trash").contains(&t.trashed_to));
        let r = restore(&v, &t.trashed_to, ".lapis/trash").unwrap();
        assert_eq!(r.path, "Monthly/2026-09.md");
        assert!(v.join("Monthly/2026-09.md").is_file());
        assert!(trash_list(&v, ".lapis/trash").is_empty());
        assert!(matches!(restore(&v, "Weekly/2026-W37.md", ".lapis/trash"), Err(LapisError::Usage(_))));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn adr_template_wraps_hal() {
        let v = vault();
        let mut o = opts("Use SQLite");
        o.template = Some("builtin.adr".into());
        let w = create(&v, &o).unwrap();
        let n = notes::read(&v, &w.path).unwrap();
        assert_eq!(n.hal["type"], "adr");
        assert!(n.body.contains("# ADR: Use SQLite"));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn daily_creates_once_then_opens() {
        let v = vault();
        let d = periodic(&v, Period::Daily, Some("2026-09-09"), Some("Halo".into())).unwrap();
        assert_eq!(d.path, "Daily/2026-09-09.md");
        assert!(d.created);
        let n = notes::read(&v, &d.path).unwrap();
        assert_eq!(n.hal["doc_type"], "daily-note");
        assert_eq!(n.hal["domain"], "vault-operation");
        assert_eq!(n.hal["name"], "2026-09-09");
        assert!(!std::fs::read_to_string(v.join(&d.path)).unwrap().contains(AUTHORITATIVE_MARKER));
        let again = periodic(&v, Period::Daily, Some("2026-09-09"), None).unwrap();
        assert!(!again.created);
        assert!(matches!(periodic(&v, Period::Daily, Some("nope"), None), Err(LapisError::Usage(_))));
        assert_eq!(periodic(&v, Period::Daily, None, None).unwrap().date, today());
        let _ = std::fs::remove_dir_all(&v);
    }

    /// N13: dry runs write nothing; stale guards refuse with exit 1.
    #[test]
    fn dry_run_and_stale_guards() {
        let v = vault();
        std::fs::write(v.join("notes/g.md"), "---\nname: G\n---\nbody\n").unwrap();
        let before = std::fs::read_to_string(v.join("notes/g.md")).unwrap();
        let w =
            append_with(&v, "notes/g.md", "more", &Guard { dry_run: true, ..Default::default() }).unwrap();
        assert!(w.dry_run);
        assert!(w.text.as_deref().unwrap().ends_with("more\n"));
        assert_eq!(std::fs::read_to_string(v.join("notes/g.md")).unwrap(), before, "dry run must not write");
        let j = serde_json::to_value(&w).unwrap();
        assert_eq!(j["dryRun"], true);

        let n = notes::read(&v, "notes/g.md").unwrap();
        let ok = Guard { if_hash: Some(n.hash.clone()), if_mtime: n.updated_at, dry_run: false };
        append_with(&v, "notes/g.md", "real", &ok).unwrap();
        assert!(std::fs::read_to_string(v.join("notes/g.md")).unwrap().contains("real"));
        // the file changed: the old hash is now stale
        let e = append_with(&v, "notes/g.md", "again", &ok).unwrap_err();
        assert_eq!(e.exit_code(), 1);
        assert!(e.to_string().contains("stale"));
        let e = append_with(&v, "notes/g.md", "x", &Guard { if_mtime: Some(1), ..Default::default() })
            .unwrap_err();
        assert!(e.to_string().contains("mtime"));
        // wire shape without a dry run has neither key
        let w = append_with(&v, "notes/g.md", "z", &Guard::default()).unwrap();
        let j = serde_json::to_value(&w).unwrap();
        assert!(j.get("dryRun").is_none() && j.get("text").is_none());

        let mut o = CreateOpts {
            title: "Dry".into(),
            path: Some("notes/".into()),
            template: None,
            doc_type: None,
            domain: None,
            tags: vec![],
            body: None,
            operator: None,
            inbox: "inbox".into(),
            director: None,
            template_date: None,
            dry_run: true,
        };
        let w = create(&v, &o).unwrap();
        assert_eq!(w.path, "notes/dry.md");
        assert!(w.dry_run && !v.join("notes/dry.md").exists());
        assert!(w.text.as_deref().unwrap().starts_with("---\n"));
        o.dry_run = false;
        assert!(v.join(create(&v, &o).unwrap().path).exists());
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn trash_moves_under_bucket_and_never_overwrites() {
        let v = vault();
        std::fs::write(v.join("notes/x.md"), "one").unwrap();
        let t = trash(&v, "notes/x.md", ".lapis/trash").unwrap();
        assert_eq!(t.trashed_to, ".lapis/trash/notes/x.md");
        assert!(!v.join("notes/x.md").exists());
        assert_eq!(std::fs::read_to_string(v.join(&t.trashed_to)).unwrap(), "one");
        std::fs::write(v.join("notes/x.md"), "two").unwrap();
        let t2 = trash(&v, "notes/x.md", ".lapis/trash").unwrap();
        assert_ne!(t2.trashed_to, t.trashed_to);
        assert!(t2.trashed_to.starts_with(".lapis/trash/notes/x."));
        assert_eq!(trash(&v, "notes/missing.md", ".lapis/trash").unwrap_err().exit_code(), 3);
        assert!(matches!(trash(&v, &t.trashed_to, ".lapis/trash"), Err(LapisError::Usage(_))));
        let _ = std::fs::remove_dir_all(&v);
    }

    fn vault() -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-write-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(d.join("notes")).unwrap();
        d
    }
    fn opts(title: &str) -> CreateOpts {
        CreateOpts { title: title.into(), inbox: "inbox".into(), ..Default::default() }
    }

    #[test]
    fn init_vault_writes_welcome_and_gitignore() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-init-{}-{n}-{seq}", std::process::id()));
        let w = init_vault(Some(d.to_str().unwrap())).unwrap();
        assert!(w.created);
        assert!(d.join("Welcome.md").is_file());
        assert!(d.join(".gitignore").is_file());
        assert!(d.join("Daily").is_dir());
        let gi = std::fs::read_to_string(d.join(".gitignore")).unwrap();
        assert!(gi.contains(".lapis/"));
        let again = init_vault(Some(d.to_str().unwrap())).unwrap();
        assert!(!again.created);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Ship Smoke!  Test"), "ship-smoke-test");
        assert_eq!(slug("  ---  "), "note");
        assert_eq!(slug("Lapis · SPEC"), "lapis-spec");
    }

    #[test]
    fn create_defaults_to_inbox_with_create_set() {
        let v = vault();
        let w = create(&v, &opts("ship-smoke")).unwrap();
        assert_eq!(w.path, "inbox/ship-smoke.md");
        assert_eq!(w.taxonomy, "fallback");
        let n = notes::read(&v, &w.path).unwrap();
        assert!(n.hal_valid);
        assert_eq!(n.hal["name"], "ship-smoke");
        assert_eq!(n.hal["status"], "draft");
        assert_eq!(n.hal["hal_version"], "1.0");
        assert_eq!(n.hal["domain"], "inbox");
        assert_eq!(n.hal["created"], today());
        assert_eq!(n.hal["tags"], serde_json::json!([]));
        assert!(n.hal.get("operator").is_none());
        assert_eq!(n.body.trim(), "# ship-smoke");
        assert!(!std::fs::read_to_string(v.join(&w.path)).unwrap().contains(AUTHORITATIVE_MARKER));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn create_canon_type_gets_marker_and_refuses_overwrite() {
        let v = vault();
        let mut o = opts("Lapis status");
        o.path = Some("notes/".into());
        o.doc_type = Some("status".into());
        o.operator = Some("Halo".into());
        o.tags = vec!["lapis".into()];
        let w = create(&v, &o).unwrap();
        assert_eq!(w.path, "notes/lapis-status.md");
        let raw = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert!(
            raw.starts_with("---\nname: Lapis status\ntype: status\ndoc_type: status\ndomain: notes\n"),
            "{raw}"
        );
        assert!(raw.contains("hal_version: '1.0'") || raw.contains("hal_version: \"1.0\""), "{raw}");
        assert!(raw.contains("\n---\n<!--hal:authoritative:yaml-->\n\n# Lapis status\n"), "{raw}");
        assert!(raw.contains("operator: Halo"));
        assert!(matches!(create(&v, &o), Err(LapisError::Usage(_))));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn create_rejects_root_and_escape() {
        let v = vault();
        let mut o = opts("Welcome");
        o.path = Some("Welcome.md".into());
        assert!(matches!(create(&v, &o), Err(LapisError::Usage(_))));
        o.path = Some("../x/".into());
        assert_eq!(create(&v, &o).unwrap_err().exit_code(), 3);
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn create_with_template_overlays_hal_and_substitutes() {
        let v = vault();
        std::fs::create_dir_all(v.join(".lapis/templates")).unwrap();
        std::fs::write(
            v.join(".lapis/templates/adr.md"),
            "---\ntype: adr\nstatus: proposed\nborn_from: template\n---\n# ADR: {{title}}\n\nDate: {{date}}\n{{cursor}}\n",
        )
        .unwrap();
        let mut o = opts("Use Rust");
        o.path = Some("notes/".into());
        o.template = Some("adr".into());
        let w = create(&v, &o).unwrap();
        let n = notes::read(&v, &w.path).unwrap();
        assert_eq!(n.hal["type"], "adr");
        assert_eq!(n.hal["doc_type"], "adr", "template `type` must also set doc_type");
        assert_eq!(n.hal["status"], "proposed");
        assert_eq!(n.hal["born_from"], "template");
        assert_eq!(n.hal["name"], "Use Rust");
        assert!(n.body.contains("# ADR: Use Rust"));
        assert!(n.body.contains(&format!("Date: {}", today())));
        assert!(!n.body.contains("{{"));
        assert!(matches!(load_template(&v, "nope", "t", None, None), Err(LapisError::Usage(_))));
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn append_bumps_updated_and_preserves_unknown_keys() {
        let v = vault();
        let raw = "---\nname: X\n# a comment\nweird-key: [1, 2]\nupdated: 2020-01-01\n---\n<!--hal:authoritative:yaml-->\n\nbody\n";
        std::fs::write(v.join("notes/x.md"), raw).unwrap();
        let w = append_with(&v, "notes/x", "more text", &Guard::default()).unwrap();
        assert_eq!(w.path, "notes/x.md");
        let out = std::fs::read_to_string(v.join("notes/x.md")).unwrap();
        assert!(out.contains("# a comment\nweird-key: [1, 2]\n"));
        assert!(out.contains(&format!("updated: {}\n---\n<!--hal:authoritative:yaml-->", today())), "{out}");
        assert!(out.ends_with("body\n\nmore text\n"), "{out}");
        assert_eq!(w.hal["weird-key"], serde_json::json!([1, 2]));
        assert_eq!(append_with(&v, "notes/missing.md", "x", &Guard::default()).unwrap_err().exit_code(), 3);
        let _ = std::fs::remove_dir_all(&v);
    }

    #[test]
    fn set_key_inserts_when_absent_and_ignores_no_frontmatter() {
        let s = set_frontmatter_key("---\nname: X\n---\nbody\n", "updated", "2026-09-09");
        assert_eq!(s, "---\nname: X\nupdated: 2026-09-09\n---\nbody\n");
        assert_eq!(set_frontmatter_key("plain\n", "updated", "x"), "plain\n");
        // a `---` rule in the body is not the closing delimiter of a missing block
        assert_eq!(set_frontmatter_key("text\n---\nmore\n", "updated", "x"), "text\n---\nmore\n");
    }

    #[test]
    fn capture_writes_timestamped_inbox_note() {
        let v = vault();
        let w = capture(&v, "  Chinese quant trader built a bot\nsecond line", "inbox", None).unwrap();
        assert!(w.path.starts_with("inbox/"), "{}", w.path);
        assert!(w.path.ends_with("-chinese-quant-trader-built-a-bot.md"), "{}", w.path);
        let n = notes::read(&v, &w.path).unwrap();
        assert_eq!(n.hal["doc_type"], "capture");
        assert_eq!(n.title, "Chinese quant trader built a bot");
        assert!(n.body.contains("second line"));
        assert!(matches!(capture(&v, "  ", "inbox", None), Err(LapisError::Usage(_))));
        let _ = std::fs::remove_dir_all(&v);
    }
}
