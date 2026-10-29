//! The create-set and the write of a new note, moved verbatim from the CLI.
//!
//! The bytes are the contract: `create_new` writes exactly what
//! `lapis create` writes for the same request — create-set keys in canonical
//! order, YAML quoting from `yaml_serde`, the `# <title>` default body, the
//! authoritative marker for canon types, a `-`-separated Unicode slug, the
//! overlay's inbox bucket, and containment before the staged write.
//!
//! Templates and the in-scope RAG rewrite gate stay CLI-side on purpose: they
//! patch the text between [`compose_text`] and [`write_composed`], so the CLI
//! must orchestrate them itself and cannot drift from these bytes.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    Error, Result,
    contain::{clean_rel, contained_new},
    overlay::inbox_bucket,
    taxonomy::{self, Classified},
};
use std::path::{Path, PathBuf};

/// HAL frontmatter version stamped into every create-set.
pub const HAL_VERSION: &str = "1.0";
/// Marker appended for canon document types, after the closing `---`.
pub const AUTHORITATIVE_MARKER: &str = "<!--hal:authoritative:yaml-->";
/// Types that get the authoritative marker after the closing `---`.
pub const CANON_TYPES: &[&str] =
    &["foundry-doc", "foundry-product", "status", "cross-reference", "maintenance-canon"];

/// One note creation request. Mirrors the CLI's `CreateOpts` minus the CLI's
/// own policies (`--template`, dry runs); `created` is caller-owned time.
#[derive(Debug, Clone, Default)]
pub struct CreateRequest {
    /// Display title; becomes `name`/`title` and the `--title` gate.
    pub title: String,
    /// Vault-relative file or directory. A directory when it ends with `/` or
    /// already exists as one; without one the note lands in the inbox bucket.
    pub path: Option<String>,
    /// Requested document type, else the create-set classifies.
    pub doc_type: Option<String>,
    /// Overrides the classified domain.
    pub domain: Option<String>,
    pub tags: Vec<String>,
    /// Body; empty means the `# <title>` default.
    pub body: Option<String>,
    /// Stamps `operator:` only when present and not blank.
    pub operator: Option<String>,
    /// The inbox bucket to use when `path` is absent. `None` reads the
    /// vault's `.lapis/vault.json`, defaulting to `inbox` when absent.
    pub inbox: Option<String>,
    /// ISO date (`YYYY-MM-DD`) stamped into `created:` and `updated:`.
    pub created: String,
}

/// What `create_new` did: the written path, its title and exact byte count.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenNote {
    pub path: String,
    pub title: String,
    pub bytes: u64,
    /// `"taxonomy.py"` or `"fallback"`, reported in `--json` so an agent knows.
    pub taxonomy: &'static str,
}

/// The vault-relative path [`create_new`] would write, without writing it:
/// inbox-bucket default, directory vs file, slug, then the two refusals —
/// the vault root and an existing target — with the CLI's messages.
///
/// The CLI needs this for its create flow (compose → RAG gate → write)
/// and for dry runs, which must produce the same path a real write would.
pub fn target_path(root: &Path, req: &CreateRequest, inbox: &str) -> Result<String> {
    let file = format!("{}.md", slug(&req.title));
    let rel = match req.path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        None => format!("{}/{}", inbox.trim_matches('/'), file),
        Some(p) => {
            let is_dir = p.ends_with('/') || root.join(clean_rel(p)?).is_dir();
            let clean = clean_rel(p)?;
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
        return Err(Error::Usage(format!(
            "refusing to create at the vault root: {rel} (use --path <folder>/)"
        )));
    }
    if root.join(&rel).exists() {
        return Err(Error::Usage(format!("already exists: {rel}")));
    }
    Ok(rel)
}

/// Build the create-set mapping in canonical key order.
///
/// `classify` is the caller-supplied route to the canon, e.g. the CLI's
/// `taxonomy.py` child-process lookup wrapped in a closure taking the
/// vault-relative path. `None` — or a hook answering `None` — falls back
/// to the first path segment; the crate never spawns a process.
pub fn create_set(
    rel: &str,
    req: &CreateRequest,
    classify: Option<&Classify>,
) -> (Map<String, Value>, &'static str) {
    let class = match classify.and_then(|f| f(rel)) {
        Some(c) => c,
        None => taxonomy::fallback(rel, req.doc_type.as_deref()),
    };
    let doc_type = req.doc_type.clone().or(class.doc_type.clone()).unwrap_or_else(|| "note".to_string());
    let domain = req.domain.clone().or(class.domain.clone());
    let today = &req.created;
    let mut m = Map::new();
    m.insert("name".into(), Value::String(req.title.clone()));
    // The RAG gate requires `title`; `name` stays for readers that key on it.
    m.insert("title".into(), Value::String(req.title.clone()));
    m.insert("type".into(), Value::String(doc_type.clone()));
    m.insert("doc_type".into(), Value::String(doc_type));
    if let Some(d) = domain {
        m.insert("domain".into(), Value::String(d));
    }
    m.insert("status".into(), Value::String("draft".into()));
    m.insert("created".into(), Value::String(today.clone()));
    m.insert("updated".into(), Value::String(today.clone()));
    m.insert("hal_version".into(), Value::String(HAL_VERSION.into()));
    if let Some(op) = req.operator.as_deref().filter(|s| !s.trim().is_empty()) {
        m.insert("operator".into(), Value::String(op.to_string()));
    }
    m.insert("tags".into(), Value::Array(req.tags.iter().map(|t| Value::String(t.clone())).collect()));
    (m, class.source)
}

/// Serialize a mapping as YAML frontmatter text (no delimiters).
pub fn to_yaml(m: &Map<String, Value>) -> Result<String> {
    let v = serde_json::Value::Object(m.clone());
    let y: yaml_serde::Value = yaml_serde::to_value(&v).map_err(|e| Error::Internal(format!("yaml: {e}")))?;
    let s = yaml_serde::to_string(&y).map_err(|e| Error::Internal(format!("yaml: {e}")))?;
    Ok(s.strip_prefix("---\n").unwrap_or(&s).trim_end().to_string())
}

/// The final bytes of a new note: frontmatter, the canon marker when the
/// type is canon, and the body (trimmed of its trailing newlines).
///
/// The body must already be final — templates merged, the `# <title>` default
/// applied. [`create_new`] journals that order; the CLI does the same around
/// its gate and template policies.
pub fn compose_text(hal: &Map<String, Value>, body: &str) -> Result<String> {
    let doc_type = hal.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let marker = if is_canon(&doc_type) { format!("{AUTHORITATIVE_MARKER}\n\n") } else { String::new() };
    Ok(format!("---\n{}\n---\n{marker}{}\n", to_yaml(hal)?, body.trim_end()))
}

/// A caller-supplied route to the canon: the vault-relative path maps to its
/// class, or `None`. The CLI wraps its `taxonomy.py` child-process lookup in
/// one; a phone passes `Option::None` for the whole hook.
pub type Classify<'a> = dyn Fn(&str) -> Option<Classified> + 'a;

fn is_canon(doc_type: &str) -> bool {
    CANON_TYPES.contains(&doc_type)
}

/// Write a composed text to its `contained_new` location: missing folders
/// become real directories, the write is staged and renamed, and a folder
/// symlinked out of the vault cannot carry the bytes outside it. Returns the
/// byte count.
pub fn write_composed(vault: &Path, rel: &str, text: &str) -> Result<u64> {
    let abs: PathBuf = contained_new(vault, rel)?;
    Ok(crate::fsutil::write_atomic(&abs, text)?)
}

/// Create a note: decide the path, build the create-set, default the body,
/// compose the bytes, and write them under [`contained_new`] containment.
///
/// The same result `lapis create --no-reindex` produces, minus the CLI's
/// template and rewrite-gate policies. `classify` is the caller's route to
/// the canon (the CLI wires its `taxonomy.py` lookup here); `None` uses the
/// pure fallback. The crate never spawns a process.
pub fn create_new<'a>(
    vault: &Path,
    req: &CreateRequest,
    classify: Option<&'a Classify<'a>>,
) -> Result<WrittenNote> {
    if req.title.trim().is_empty() {
        return Err(Error::Usage("--title is required".into()));
    }
    let inbox = if let Some(b) = req.inbox.as_deref().filter(|s| !s.is_empty()) {
        b.to_string()
    } else {
        inbox_bucket(vault)?
    };
    let rel = target_path(vault, req, &inbox)?;
    let (hal, tax_source) = create_set(&rel, req, classify);
    let mut body = req.body.clone().unwrap_or_default();
    if body.trim().is_empty() {
        body = format!("# {}\n", req.title);
    }
    let text = compose_text(&hal, &body)?;
    let bytes = write_composed(vault, &rel, &text)?;
    Ok(WrittenNote { path: rel, title: req.title.clone(), bytes, taxonomy: tax_source })
}

fn slug(s: &str) -> String {
    crate::slug(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("lapis-notes-{name}-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn req(title: &str, day: &str) -> CreateRequest {
        CreateRequest { title: title.into(), created: day.into(), ..Default::default() }
    }

    /// The golden CREATE case (fbf2c0a), at byte level, through the library:
    /// explicit path, tags, body, config operator.
    #[test]
    fn create_with_path_writes_the_golden_bytes() {
        let v = dir("gold-create");
        let r = CreateRequest {
            title: "Ship the Golden Note".into(),
            path: Some("projects/".into()),
            tags: vec!["lapis".into(), "golden".into()],
            body: Some("First line.\n\nSecond line.".into()),
            operator: Some("Golden Operator".into()),
            inbox: Some("inbox".into()),
            created: "2026-10-29".into(),
            ..Default::default()
        };
        let w = create_new(&v, &r, None).unwrap();
        assert_eq!(w.path, "projects/ship-the-golden-note.md");
        assert_eq!(w.taxonomy, "fallback");
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert_eq!(
            text,
            "---\nname: Ship the Golden Note\ntitle: Ship the Golden Note\ntype: note\ndoc_type: note\ndomain: projects\nstatus: draft\ncreated: 2026-10-29\nupdated: 2026-10-29\nhal_version: '1.0'\noperator: Golden Operator\ntags:\n- lapis\n- golden\n---\nFirst line.\n\nSecond line.\n"
        );
        let _ = std::fs::remove_dir_all(v);
    }

    /// The golden INBOX case: overlay bucket `Inbox/Drop`, default body, a
    /// title that needs YAML quoting and a Unicode slug.
    #[test]
    fn inbox_create_reads_the_overlay_bucket_and_writes_the_golden_bytes() {
        let v = dir("gold-inbox");
        std::fs::create_dir_all(v.join(".lapis")).unwrap();
        let overlay = "{\"buckets\":{\"inbox\":\"Inbox/Drop\"}}\n";
        std::fs::write(v.join(".lapis/vault.json"), overlay).unwrap();
        let w = create_new(&v, &req("Inbox drop: Café & Ünïcode!", "2026-10-29"), None).unwrap();
        assert_eq!(w.path, "Inbox/Drop/inbox-drop-café-ünïcode.md");
        assert_eq!(w.taxonomy, "fallback");
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert_eq!(
            text,
            "---\nname: 'Inbox drop: Café & Ünïcode!'\ntitle: 'Inbox drop: Café & Ünïcode!'\ntype: note\ndoc_type: note\ndomain: inbox\nstatus: draft\ncreated: 2026-10-29\nupdated: 2026-10-29\nhal_version: '1.0'\ntags: []\n---\n# Inbox drop: Café & Ünïcode!\n"
        );
        // The overlay file is untouched by a create.
        assert_eq!(std::fs::read_to_string(v.join(".lapis/vault.json")).unwrap(), overlay);
        let _ = std::fs::remove_dir_all(v);
    }

    /// The golden SAME case with no overlay file: default bucket `inbox`;
    /// the second create is refused and the first bytes stay exact.
    #[test]
    fn collision_is_refused_and_keeps_the_first_bytes() {
        let v = dir("gold-collision");
        let w = create_new(&v, &req("Same Title", "2026-10-29"), None).unwrap();
        assert_eq!(w.path, "inbox/same-title.md");
        let first = std::fs::read(v.join("inbox/same-title.md")).unwrap();
        match create_new(&v, &req("Same Title", "2026-10-30"), None) {
            Err(Error::Usage(m)) => assert_eq!(m, "already exists: inbox/same-title.md"),
            Err(e) => panic!("expected a usage refusal, got {e:?}"),
            Ok(w) => panic!("the second create landed: {:?}", w.path),
        }
        assert_eq!(std::fs::read(v.join("inbox/same-title.md")).unwrap(), first, "first bytes intact");
        assert_eq!(std::fs::read_dir(v.join("inbox")).unwrap().count(), 1, "no abandoned files");
        let _ = std::fs::remove_dir_all(v);
    }

    /// Root drops, escapes and missing titles refuse exactly as the CLI does.
    #[test]
    fn refusals_match_the_cli() {
        let v = dir("gold-refuse");
        match create_new(&v, &req("", "2026-10-29"), None) {
            Err(Error::Usage(m)) => assert_eq!(m, "--title is required"),
            other => panic!("expected --title refusal, got {other:?}"),
        }
        let r = CreateRequest { path: Some("Welcome.md".into()), ..req("Welcome", "2026-10-29") };
        match create_new(&v, &r, None) {
            Err(Error::Usage(m)) => {
                assert_eq!(m, "refusing to create at the vault root: Welcome.md (use --path <folder>/)")
            }
            other => panic!("expected root refusal, got {other:?}"),
        }
        let r = CreateRequest { path: Some("../away/x.md".into()), ..req("X", "2026-10-29") };
        match create_new(&v, &r, None) {
            Err(Error::Path(m)) => assert_eq!(m, "path escapes the vault: ../away/x.md"),
            other => panic!("expected escape refusal, got {other:?}"),
        }
        let r = CreateRequest { path: Some("OneName.md".into()), ..req("Plain", "2026-10-29") };
        match create_new(&v, &r, None) {
            Err(Error::Usage(m)) => {
                assert_eq!(m, "refusing to create at the vault root: OneName.md (use --path <folder>/)")
            }
            other => panic!("expected root refusal for a single-segment path, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(v);
    }

    /// A canon type gets the marker; containment rules a folder link out.
    #[cfg(unix)]
    #[test]
    fn canon_marker_and_containment() {
        let v = dir("gold-canon");
        let r = CreateRequest {
            path: Some("foundry/".into()),
            doc_type: Some("foundry-doc".into()),
            ..req("Canon Doc", "2026-10-29")
        };
        let w = create_new(&v, &r, None).unwrap();
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        eprintln!("CANON TEXT: {text:?}");
        assert!(
            text.starts_with("---\nname: Canon Doc\n")
                && text.contains("---\n<!--hal:authoritative:yaml-->\n\n# Canon Doc\n")
        );
        assert_eq!(w.taxonomy, "fallback");
        let _ = std::fs::remove_dir_all(v);

        // The escape behind a linked folder never carries a create out.
        let v = dir("gold-link");
        let outside = dir("gold-link-outside");
        std::fs::create_dir_all(v.join("away")).unwrap();
        std::os::unix::fs::symlink(&outside, v.join("away/leak")).unwrap();
        let r = CreateRequest { path: Some("away/leak/".into()), ..req("Leak", "2026-10-29") };
        match create_new(&v, &r, None) {
            Err(Error::Path(m)) => assert_eq!(m, "away/leak/leak.md: a folder on the way escapes the vault"),
            other => panic!("expected containment refusal, got {other:?}"),
        }
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing written outside the vault");
        let _ = std::fs::remove_dir_all(outside);
        let _ = std::fs::remove_dir_all(v);
    }

    /// A caller-supplied classify hook routes through, hook answers winning;
    /// a hook that answers None drops to the fallback. The source string the
    /// caller carries is what `--json` reports.
    #[test]
    fn a_classify_hook_replaces_the_fallback() {
        let v = dir("gold-hook");
        let canon = |rel: &str| {
            if rel.starts_with("foundry/") {
                Some(Classified {
                    domain: Some("foundry".into()),
                    doc_type: Some("foundry-row".into()),
                    source: "taxonomy.py",
                })
            } else {
                None
            }
        };
        let mut r = CreateRequest { path: Some("foundry/".into()), ..req("Routed", "2026-10-29") };
        r.inbox = Some("inbox".into());
        let w = create_new(&v, &r, Some(&canon)).unwrap();
        assert_eq!(w.taxonomy, "taxonomy.py", "the hook's own source is reported");
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert!(text.contains("type: foundry-row\n") && text.contains("domain: foundry\n"));

        let mut r = CreateRequest { path: Some("Notes/".into()), ..req("Miss", "2026-10-29") };
        r.inbox = Some("inbox".into());
        let w = create_new(&v, &r, Some(&canon)).unwrap();
        assert_eq!(w.taxonomy, "fallback", "a hook that answers None falls back");
        let _ = std::fs::remove_dir_all(v);
    }

    /// A capturing closure hooks in: the alias is bounded by its lifetime, so
    /// local state does not need to be `'static` (E0521 regression).
    #[test]
    fn a_capturing_closure_hooks_in() {
        let v = dir("gold-cap");
        let outside_hint = format!("notes-{}", 7);
        let hook = move |rel: &str| {
            // Captures `outside_hint`, a local `String`.
            let domain = rel.split('/').next().map(str::to_string).or_else(|| Some(outside_hint.clone()));
            Some(Classified { domain, doc_type: None, source: "caller" })
        };
        let mut r = CreateRequest { path: Some("Notes/".into()), ..req("Local", "2026-10-29") };
        r.inbox = Some("inbox".into());
        let w = create_new(&v, &r, Some(&hook)).unwrap();
        assert_eq!(w.taxonomy, "caller");
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert!(text.contains("domain: Notes\n"), "the hook answered: {text:?}");
        let _ = std::fs::remove_dir_all(v);
    }

    /// taxonomy fallback vs the explicit override, miss the same parts.
    #[test]
    fn taxonomy_sources() {
        let v = dir("tax");
        let r = CreateRequest { path: Some("Notes/".into()), ..req("T", "2026-10-29") };
        let w = create_new(&v, &r, None).unwrap();
        assert_eq!(w.taxonomy, "fallback");
        let text = std::fs::read_to_string(v.join(&w.path)).unwrap();
        assert!(text.contains("domain: notes"), "the first segment lowercases: {text:?}");
        let _ = std::fs::remove_dir_all(v);
    }
}
