//! `rag-fm-wikilink-gate@0.1.0` — deterministic frontmatter and wikilink check.
//!
//! In scope: vault-relative markdown creates and touches under `foundry/**`
//! and `agents/mail_room/**`. The default mode is shadow: the [`GateResult`]
//! is logged and the write proceeds. `LAPIS_RAG_GATE=hard` refuses `fail` and
//! `cannot_tell`. Reads never enter this module.
//!
//! Wikilinks resolve through [`lapis_lattice::Engine::documents`] plus the
//! same `resolve_in` rules as `lapis resolve`. The vault tree is not the
//! source of truth for a hit.

#[cfg(test)]
use std::cell::Cell;
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{LapisError, Result};
use crate::{hal, notes, resolve};

/// Frozen policy id. Matches `schema/gate-result.schema.json`.
pub const POLICY_ID: &str = "rag-fm-wikilink-gate@0.1.0";

/// `hard` blocks. Anything else, including unset, is shadow.
pub const ENV_MODE: &str = "LAPIS_RAG_GATE";

const REQUIRED_KEYS: [&str; 4] = ["title", "type", "updated", "tags"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Shadow,
    Hard,
}

#[cfg(test)]
thread_local! {
    static MODE_OVERRIDE: Cell<Option<Mode>> = const { Cell::new(None) };
}

/// Restores the previous process-local mode override when dropped.
#[cfg(test)]
#[must_use = "the override lasts until this guard is dropped"]
pub struct ModeOverride {
    prev: Option<Mode>,
}

#[cfg(test)]
pub fn force_mode(mode: Mode) -> ModeOverride {
    let prev = MODE_OVERRIDE.with(|c| c.replace(Some(mode)));
    ModeOverride { prev }
}

#[cfg(test)]
impl Drop for ModeOverride {
    fn drop(&mut self) {
        let prev = self.prev.take();
        MODE_OVERRIDE.with(|c| c.set(prev));
    }
}

pub fn mode_from_env() -> Mode {
    match std::env::var(ENV_MODE) {
        Ok(v) if v.trim() == "hard" => Mode::Hard,
        _ => Mode::Shadow,
    }
}

pub fn current_mode() -> Mode {
    #[cfg(test)]
    if let Some(mode) = MODE_OVERRIDE.with(|c| c.get()) {
        return mode;
    }
    mode_from_env()
}

/// Markdown under `foundry/` or `agents/mail_room/`. Other trees, and
/// non-markdown files, are informational skips.
pub fn in_scope(rel: &str) -> bool {
    let rel = rel.trim_start_matches("./");
    if notes::kind_of(rel) != notes::Kind::Markdown {
        return false;
    }
    rel.starts_with("foundry/") || rel.starts_with("agents/mail_room/")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    Ok,
    Fail,
    CannotTell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    MissingFmFence,
    MissingFmKey,
    InvalidFm,
    DanglingWikilink,
    AmbiguousWikilink,
    VerifiedForbidden,
    OutOfScope,
    Other,
}

impl ReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingFmFence => "missing_fm_fence",
            Self::MissingFmKey => "missing_fm_key",
            Self::InvalidFm => "invalid_fm",
            Self::DanglingWikilink => "dangling_wikilink",
            Self::AmbiguousWikilink => "ambiguous_wikilink",
            Self::VerifiedForbidden => "verified_forbidden",
            Self::OutOfScope => "out_of_scope",
            Self::Other => "other",
        }
    }

    fn is_fail(self) -> bool {
        matches!(
            self,
            Self::MissingFmFence
                | Self::MissingFmKey
                | Self::InvalidFm
                | Self::DanglingWikilink
                | Self::VerifiedForbidden
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reason {
    pub code: ReasonCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkRetarget {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrichment {
    pub keys_added: Vec<String>,
    pub links_retargeted: Vec<LinkRetarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateResult {
    pub policy_id: String,
    pub path: String,
    pub status: GateStatus,
    pub reasons: Vec<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enriched: Option<Enrichment>,
    pub remainder_eligible: bool,
}

/// Gate outcome plus the bytes a write should store when the mode allows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub result: GateResult,
    pub text: String,
}

/// Check `text` for `rel`. Does not write and does not log.
pub fn review(root: &Path, rel: &str, text: &str) -> Review {
    if !in_scope(rel) {
        let (result, text) = evaluate(rel, text, &[], "", None);
        return Review { result, text };
    }
    let (indexed, index_error) = match indexed_markdown_paths(root) {
        Ok(paths) => (paths, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let (result, text) = evaluate(rel, text, &indexed, &today(), index_error.as_deref());
    Review { result, text }
}

/// Shadow-log an in-scope result, then either return the enriched bytes or
/// refuse the write. Out-of-scope paths are returned unchanged and unlogged.
pub fn apply_on_write(root: &Path, rel: &str, text: &str) -> Result<String> {
    let review = review(root, rel, text);
    if in_scope(rel) {
        eprintln!("{}", shadow_log_line(&review.result));
    }
    if blocks(current_mode(), &review.result) {
        return Err(LapisError::Usage(block_message(&review.result)));
    }
    Ok(review.text)
}

pub fn blocks(mode: Mode, result: &GateResult) -> bool {
    mode == Mode::Hard && matches!(result.status, GateStatus::Fail | GateStatus::CannotTell)
}

pub fn shadow_log_line(result: &GateResult) -> String {
    let body = serde_json::to_string(result).unwrap_or_else(|_| "{}".to_string());
    format!("lapis: rag-gate {body}")
}

fn block_message(result: &GateResult) -> String {
    let detail = result
        .reasons
        .iter()
        .map(|r| format!("{}: {}", r.code.as_str(), r.message))
        .collect::<Vec<_>>()
        .join("; ");
    format!("{POLICY_ID} blocked {}: {detail}", result.path)
}

fn today() -> String {
    jiff::Zoned::now().strftime("%Y-%m-%d").to_string()
}

fn indexed_markdown_paths(root: &Path) -> Result<Vec<String>> {
    let engine = lapis_lattice::Engine::open(root)?;
    let mut out = Vec::new();
    let mut offset = 0u32;
    loop {
        let page =
            engine.documents(&lapis_lattice::ListParams { limit: 1000, offset, ..Default::default() })?;
        let n = u32::try_from(page.len()).unwrap_or(u32::MAX);
        for doc in page {
            if doc.kind == lapis_lattice::MARKDOWN {
                out.push(doc.path);
            }
        }
        if n < 1000 {
            break;
        }
        offset = offset.saturating_add(n);
    }
    Ok(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fence {
    Missing,
    Closed,
    Unclosed,
}

fn fence_state(text: &str) -> Fence {
    let first = text.split('\n').next().unwrap_or("").trim_end_matches('\r');
    if first != "---" {
        return Fence::Missing;
    }
    let (head, _) = hal::raw_parts(text);
    if head.is_empty() { Fence::Unclosed } else { Fence::Closed }
}

fn title_from_path(path: &str) -> String {
    let stem = notes::stem_of(path);
    let stem = stem.trim();
    if stem.is_empty() { "note".to_string() } else { stem.to_string() }
}

fn yaml_plain(s: &str) -> String {
    let safe = !s.is_empty()
        && !s.starts_with([' ', '-'])
        && !s.ends_with(' ')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.'));
    if safe { s.to_string() } else { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }
}

fn scaffold_if_missing(path: &str, text: &str, today: &str) -> (String, Vec<String>) {
    if fence_state(text) != Fence::Missing {
        return (text.to_string(), Vec::new());
    }
    let title = yaml_plain(&title_from_path(path));
    let fm = format!("---\ntitle: {title}\ntype: note\nupdated: {today}\ntags: []\n---\n");
    (format!("{fm}{text}"), REQUIRED_KEYS.iter().map(|k| (*k).to_string()).collect())
}

struct Occur {
    start: usize,
    end: usize,
    raw: String,
    target: String,
    alias: Option<String>,
    anchor: Option<String>,
}

fn extract_links(body: &str) -> Vec<Occur> {
    let mut out = Vec::new();
    let mut rest = body;
    let mut base = 0usize;
    while let Some(i) = rest.find("[[") {
        let after = i + 2;
        let Some(rel_end) = rest[after..].find("]]") else { break };
        let end = after + rel_end + 2;
        let inner = &rest[after..after + rel_end];
        let (target, alias, anchor) = resolve::parse_link(inner);
        if !target.is_empty() {
            out.push(Occur {
                start: base + i,
                end: base + end,
                raw: rest[i..end].to_string(),
                target,
                alias,
                anchor,
            });
        }
        base += end;
        rest = &rest[end..];
    }
    out
}

fn render_link(target: &str, anchor: &Option<String>, alias: &Option<String>) -> String {
    let mut inner = target.to_string();
    if let Some(anchor) = anchor {
        inner.push('#');
        inner.push_str(anchor);
    }
    if let Some(alias) = alias {
        inner.push('|');
        inner.push_str(alias);
    }
    format!("[[{inner}]]")
}

fn retarget(text: &str, indexed: &[String]) -> (String, Vec<LinkRetarget>) {
    let (head, body) = hal::raw_parts(text);
    let links = extract_links(body);
    if links.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0usize;
    let mut audit = Vec::new();
    for link in links {
        out.push_str(&body[cursor..link.start]);
        let resolved = resolve::resolve_in(&link.raw, indexed, None);
        let replacement = if resolved.resolved
            && matches!(resolved.how, "exact" | "basename")
            && let Some(path) = resolved.path.as_deref()
        {
            let canon = path.strip_suffix(".md").unwrap_or(path);
            if canon == link.target {
                body[link.start..link.end].to_string()
            } else {
                let to = render_link(canon, &link.anchor, &link.alias);
                audit.push(LinkRetarget { from: link.raw.clone(), to: to.clone() });
                to
            }
        } else {
            body[link.start..link.end].to_string()
        };
        out.push_str(&replacement);
        cursor = link.end;
    }
    out.push_str(&body[cursor..]);
    (format!("{head}{out}"), audit)
}

fn reason(
    path: &str,
    code: ReasonCode,
    message: impl Into<String>,
    link: Option<String>,
    key: Option<String>,
) -> Reason {
    Reason { code, message: message.into(), path: Some(path.to_string()), link, key }
}

enum KeyCheck<'a> {
    Ok(&'a str),
    Missing,
    Invalid,
}

fn string_key<'a>(hal: &'a Map<String, Value>, key: &str) -> KeyCheck<'a> {
    match hal.get(key) {
        None | Some(Value::Null) => KeyCheck::Missing,
        Some(Value::String(s)) if s.trim().is_empty() => KeyCheck::Missing,
        Some(Value::String(s)) => KeyCheck::Ok(s.trim()),
        Some(_) => KeyCheck::Invalid,
    }
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
}

fn fm_reasons(text: &str, path: &str) -> Vec<Reason> {
    match fence_state(text) {
        Fence::Missing => {
            vec![reason(
                path,
                ReasonCode::MissingFmFence,
                "leading YAML frontmatter fence is required",
                None,
                None,
            )]
        }
        Fence::Unclosed => {
            vec![reason(path, ReasonCode::InvalidFm, "frontmatter fence is not closed", None, None)]
        }
        Fence::Closed => mapping_reasons(text, path),
    }
}

fn mapping_reasons(text: &str, path: &str) -> Vec<Reason> {
    let parsed = hal::parse(text);
    if !parsed.hal_valid {
        let msg = parsed.error.unwrap_or_else(|| "frontmatter is not a YAML mapping".to_string());
        return vec![reason(path, ReasonCode::InvalidFm, msg, None, None)];
    }
    let mut reasons = Vec::new();
    push_string(&mut reasons, path, &parsed.hal, "title", "title must be a non-empty string");
    push_string(&mut reasons, path, &parsed.hal, "type", "type must be a non-empty string");
    match string_key(&parsed.hal, "updated") {
        KeyCheck::Missing => reasons.push(reason(
            path,
            ReasonCode::MissingFmKey,
            "missing required frontmatter key `updated`",
            None,
            Some("updated".into()),
        )),
        KeyCheck::Invalid => reasons.push(reason(
            path,
            ReasonCode::InvalidFm,
            "updated must be a string",
            None,
            Some("updated".into()),
        )),
        KeyCheck::Ok(s) if !is_date(s) => reasons.push(reason(
            path,
            ReasonCode::InvalidFm,
            "updated must be YYYY-MM-DD",
            None,
            Some("updated".into()),
        )),
        KeyCheck::Ok(_) => {}
    }
    match parsed.hal.get("tags") {
        None | Some(Value::Null) => reasons.push(reason(
            path,
            ReasonCode::MissingFmKey,
            "missing required frontmatter key `tags`",
            None,
            Some("tags".into()),
        )),
        Some(Value::Array(_)) => {}
        Some(_) => reasons.push(reason(
            path,
            ReasonCode::InvalidFm,
            "tags must be a list",
            None,
            Some("tags".into()),
        )),
    }
    reasons
}

fn push_string(reasons: &mut Vec<Reason>, path: &str, hal: &Map<String, Value>, key: &str, invalid: &str) {
    match string_key(hal, key) {
        KeyCheck::Ok(_) => {}
        KeyCheck::Missing => reasons.push(reason(
            path,
            ReasonCode::MissingFmKey,
            format!("missing required frontmatter key `{key}`"),
            None,
            Some(key.to_string()),
        )),
        KeyCheck::Invalid => {
            reasons.push(reason(path, ReasonCode::InvalidFm, invalid, None, Some(key.to_string())))
        }
    }
}

fn has_verified(text: &str) -> bool {
    let parsed = hal::parse(text);
    parsed.hal_valid && parsed.hal.contains_key("verified")
}

fn link_reasons(text: &str, path: &str, indexed: &[String]) -> Vec<Reason> {
    let mut reasons = Vec::new();
    for link in extract_links(hal::raw_parts(text).1) {
        let resolved = resolve::resolve_in(&link.raw, indexed, None);
        match resolved.how {
            "dangling" => reasons.push(reason(
                path,
                ReasonCode::DanglingWikilink,
                format!("wikilink {} matches no indexed note", link.raw),
                Some(link.raw),
                None,
            )),
            "collision" => reasons.push(reason(
                path,
                ReasonCode::AmbiguousWikilink,
                format!("wikilink {} matches {} indexed notes", link.raw, resolved.candidates.len()),
                Some(link.raw),
                None,
            )),
            _ => {}
        }
    }
    reasons
}

fn fold_status(reasons: &[Reason]) -> GateStatus {
    if reasons.iter().any(|r| r.code.is_fail()) {
        GateStatus::Fail
    } else if reasons.iter().any(|r| matches!(r.code, ReasonCode::AmbiguousWikilink | ReasonCode::Other)) {
        GateStatus::CannotTell
    } else {
        GateStatus::Ok
    }
}

fn enrichment(keys_added: Vec<String>, links_retargeted: Vec<LinkRetarget>) -> Option<Enrichment> {
    if keys_added.is_empty() && links_retargeted.is_empty() {
        None
    } else {
        Some(Enrichment { keys_added, links_retargeted })
    }
}

fn evaluate(
    path: &str,
    original: &str,
    indexed: &[String],
    today: &str,
    index_error: Option<&str>,
) -> (GateResult, String) {
    if !in_scope(path) {
        let result = GateResult {
            policy_id: POLICY_ID.to_string(),
            path: path.to_string(),
            status: GateStatus::Ok,
            reasons: vec![reason(
                path,
                ReasonCode::OutOfScope,
                "path is outside foundry/** and agents/mail_room/** markdown create/touch",
                None,
                None,
            )],
            enriched: None,
            remainder_eligible: false,
        };
        return (result, original.to_string());
    }

    let (mut text, mut keys_added) = scaffold_if_missing(path, original, today);
    let mut links_retargeted = Vec::new();
    if index_error.is_none() {
        let (rewritten, retargeted) = retarget(&text, indexed);
        text = rewritten;
        links_retargeted = retargeted;
    }
    let mut reasons = Vec::new();
    if !has_verified(original) && has_verified(&text) {
        reasons.push(reason(
            path,
            ReasonCode::VerifiedForbidden,
            "enrichment must not invent `verified`",
            None,
            Some("verified".into()),
        ));
        text = original.to_string();
        keys_added.clear();
        links_retargeted.clear();
    }
    reasons.extend(fm_reasons(&text, path));
    let linked = !extract_links(hal::raw_parts(&text).1).is_empty();
    if let Some(err) = index_error.filter(|_| linked) {
        reasons.push(reason(
            path,
            ReasonCode::Other,
            format!("lattice index unavailable: {err}"),
            None,
            None,
        ));
    } else if index_error.is_none() {
        reasons.extend(link_reasons(&text, path, indexed));
    }
    let status = fold_status(&reasons);
    let result = GateResult {
        policy_id: POLICY_ID.to_string(),
        path: path.to_string(),
        status,
        reasons,
        enriched: enrichment(keys_added, links_retargeted),
        remainder_eligible: status == GateStatus::CannotTell,
    };
    (result, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TODAY: &str = "2026-09-21";
    const FM: &str = "---\ntitle: Use\ntype: note\nupdated: 2026-09-21\ntags: []\n---\n";

    fn assert_schema(result: &GateResult) {
        let schema: Value = serde_json::from_str(include_str!("../schema/gate-result.schema.json")).unwrap();
        assert_eq!(schema["properties"]["policyId"]["const"], POLICY_ID);
        let value = serde_json::to_value(result).unwrap();
        let props = schema["properties"].as_object().unwrap();
        for key in value.as_object().unwrap().keys() {
            assert!(props.contains_key(key), "unexpected GateResult field {key}");
        }
        for req in schema["required"].as_array().unwrap() {
            let req = req.as_str().unwrap();
            assert!(value.get(req).is_some(), "missing {req}");
        }
        assert_eq!(value["policyId"], POLICY_ID);
        let statuses: Vec<&str> = schema["properties"]["status"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(statuses.contains(&value["status"].as_str().unwrap()));
        assert!(value["remainderEligible"].is_boolean());
        let codes: Vec<&str> = schema["$defs"]["reason"]["properties"]["code"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let reason_props = schema["$defs"]["reason"]["properties"].as_object().unwrap();
        for reason in value["reasons"].as_array().unwrap() {
            let obj = reason.as_object().unwrap();
            for key in obj.keys() {
                assert!(reason_props.contains_key(key), "unexpected reason field {key}");
            }
            assert!(obj.contains_key("code") && obj.contains_key("message"));
            assert!(codes.contains(&obj["code"].as_str().unwrap()));
        }
        if let Some(enriched) = value.get("enriched") {
            let allowed = schema["properties"]["enriched"]["properties"].as_object().unwrap();
            for key in enriched.as_object().unwrap().keys() {
                assert!(allowed.contains_key(key), "unexpected enriched field {key}");
            }
            assert!(enriched.get("keys_added").is_none());
            assert!(enriched.get("links_retargeted").is_none());
            if let Some(links) = enriched.get("linksRetargeted") {
                for link in links.as_array().unwrap() {
                    let obj = link.as_object().unwrap();
                    assert_eq!(obj.len(), 2);
                    assert!(obj["from"].is_string() && obj["to"].is_string());
                }
            }
        }
        assert!(value.get("policy_id").is_none());
        assert!(value.get("remainder_eligible").is_none());
    }

    #[test]
    fn scope_is_vault_relative_markdown_only() {
        assert!(in_scope("foundry/note.md"));
        assert!(in_scope("foundry/a/b.markdown"));
        assert!(in_scope("agents/mail_room/Merci/2026-09-21.md"));
        assert!(!in_scope("foundry/pic.png"));
        assert!(!in_scope("notes/Alpha.md"));
        assert!(!in_scope("agents/mail_room.md"));
        assert!(!in_scope("agents/other/x.md"));
        assert!(!in_scope("Foundry/note.md"));
    }

    #[test]
    fn missing_fence_is_scaffolded_without_verified() {
        let (result, text) = evaluate("foundry/bare.md", "# Hello\n", &[], TODAY, None);
        assert_eq!(result.status, GateStatus::Ok);
        assert!(result.reasons.is_empty());
        assert!(!result.remainder_eligible);
        let enriched = result.enriched.as_ref().unwrap();
        assert_eq!(enriched.keys_added, ["title", "type", "updated", "tags"]);
        assert!(enriched.links_retargeted.is_empty());
        assert!(text.starts_with("---\n"));
        assert!(text.contains("title: bare\n"));
        assert!(text.contains("type: note\n"));
        assert!(text.contains("updated: 2026-09-21\n"));
        assert!(text.contains("tags: []\n"));
        assert!(text.contains("# Hello\n"));
        assert!(!text.contains("verified"));
        assert_schema(&result);
    }

    #[test]
    fn missing_required_key_fails_and_does_not_invent_it() {
        let original = "---\ntype: note\nupdated: 2026-09-21\ntags: []\n---\nbody\n";
        let (result, text) = evaluate("foundry/x.md", original, &[], TODAY, None);
        assert_eq!(result.status, GateStatus::Fail);
        assert!(result.enriched.is_none());
        assert!(!result.remainder_eligible);
        assert_eq!(text, original);
        let missing: Vec<_> = result
            .reasons
            .iter()
            .filter(|r| r.code == ReasonCode::MissingFmKey)
            .map(|r| r.key.as_deref().unwrap())
            .collect();
        assert_eq!(missing, ["title"]);
        assert_schema(&result);
    }

    #[test]
    fn dangling_wikilink_fails() {
        let (result, text) = evaluate(
            "agents/mail_room/m.md",
            &format!("{FM}See [[Missing]].\n"),
            &["notes/Alpha.md".into()],
            TODAY,
            None,
        );
        assert_eq!(result.status, GateStatus::Fail);
        assert!(!result.remainder_eligible);
        assert_eq!(result.reasons.len(), 1);
        assert_eq!(result.reasons[0].code, ReasonCode::DanglingWikilink);
        assert_eq!(result.reasons[0].link.as_deref(), Some("[[Missing]]"));
        assert!(text.contains("[[Missing]]"));
        assert_schema(&result);
    }

    #[test]
    fn unique_hit_retargets_and_audits() {
        let indexed = vec!["notes/Alpha.md".into()];
        let (result, text) =
            evaluate("foundry/use.md", &format!("{FM}See [[Alpha|Cap]].\n"), &indexed, TODAY, None);
        assert_eq!(result.status, GateStatus::Ok, "{:?}", result.reasons);
        assert!(result.reasons.is_empty());
        let links = &result.enriched.as_ref().unwrap().links_retargeted;
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].from, "[[Alpha|Cap]]");
        assert_eq!(links[0].to, "[[notes/Alpha|Cap]]");
        assert!(text.contains("[[notes/Alpha|Cap]]"));
        assert!(!text.contains("[[Alpha|Cap]]"));
        assert_schema(&result);

        let (again, same) =
            evaluate("foundry/use.md", &format!("{FM}[[notes/Alpha]]\n"), &indexed, TODAY, None);
        assert_eq!(again.status, GateStatus::Ok);
        assert!(again.enriched.is_none());
        assert!(same.contains("[[notes/Alpha]]"));
    }

    #[test]
    fn multi_hit_is_cannot_tell_and_remainder_eligible() {
        let indexed = vec!["a/Spec.md".into(), "b/Spec.md".into()];
        let original = format!("{FM}[[Spec]]\n");
        let (result, text) = evaluate("foundry/use.md", &original, &indexed, TODAY, None);
        assert_eq!(result.status, GateStatus::CannotTell);
        assert!(result.remainder_eligible);
        assert!(result.enriched.is_none());
        assert_eq!(text, original);
        assert_eq!(result.reasons[0].code, ReasonCode::AmbiguousWikilink);
        assert_eq!(result.reasons[0].link.as_deref(), Some("[[Spec]]"));
        assert!(!blocks(Mode::Shadow, &result));
        assert!(blocks(Mode::Hard, &result));
        assert_schema(&result);
    }

    #[test]
    fn out_of_scope_skips_without_rewriting() {
        let original = "# no fence\n[[Missing]]\n";
        let (result, text) = evaluate("notes/x.md", original, &["notes/Alpha.md".into()], TODAY, None);
        assert_eq!(result.status, GateStatus::Ok);
        assert_eq!(result.reasons[0].code, ReasonCode::OutOfScope);
        assert!(!result.remainder_eligible);
        assert!(result.enriched.is_none());
        assert_eq!(text, original);
        assert!(!blocks(Mode::Hard, &result));
        assert_schema(&result);
    }

    #[test]
    fn index_miss_is_not_a_disk_hit_and_index_down_is_cannot_tell() {
        let (result, _) = evaluate("foundry/use.md", &format!("{FM}[[Alpha]]\n"), &[], TODAY, None);
        assert_eq!(result.status, GateStatus::Fail);
        assert_eq!(result.reasons[0].code, ReasonCode::DanglingWikilink);

        let (down, text) =
            evaluate("foundry/use.md", &format!("{FM}[[Alpha]]\n"), &[], TODAY, Some("sqlite: locked"));
        assert_eq!(down.status, GateStatus::CannotTell);
        assert!(down.remainder_eligible);
        assert_eq!(down.reasons[0].code, ReasonCode::Other);
        assert!(text.contains("[[Alpha]]"));
        assert_schema(&down);
    }

    #[test]
    fn shadow_log_line_is_the_gate_result() {
        let (result, _) = evaluate("foundry/bare.md", "hello\n", &[], TODAY, None);
        let line = shadow_log_line(&result);
        let json: Value = serde_json::from_str(line.trim_start_matches("lapis: rag-gate ")).unwrap();
        assert_eq!(json["policyId"], POLICY_ID);
        assert_eq!(json["status"], "ok");
        assert_schema(&result);
        assert_eq!(json["enriched"]["keysAdded"], json!(["title", "type", "updated", "tags"]));
    }

    #[test]
    fn reason_codes_match_the_schema_enum() {
        let schema: Value = serde_json::from_str(include_str!("../schema/gate-result.schema.json")).unwrap();
        let codes: Vec<&str> = schema["$defs"]["reason"]["properties"]["code"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let all = [
            ReasonCode::MissingFmFence,
            ReasonCode::MissingFmKey,
            ReasonCode::InvalidFm,
            ReasonCode::DanglingWikilink,
            ReasonCode::AmbiguousWikilink,
            ReasonCode::VerifiedForbidden,
            ReasonCode::OutOfScope,
            ReasonCode::Other,
        ];
        let got: Vec<&str> = all.iter().map(|c| c.as_str()).collect();
        assert_eq!(got, codes);
        for code in all {
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str());
        }
    }

    #[test]
    fn unique_retarget_reads_the_index_not_the_filesystem() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("lapis-rag-{}-{n}-{seq}", std::process::id()));
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let alpha = root.join("notes/Alpha.md");
        std::fs::write(
            &alpha,
            "---\ntitle: Alpha\ntype: note\nupdated: 2026-09-21\ntags: []\n---\n# Alpha\n",
        )
        .unwrap();
        {
            let mut engine = lapis_lattice::Engine::open(&root).unwrap();
            engine.reindex().unwrap();
        }
        std::fs::remove_file(&alpha).unwrap();
        let review = review(&root, "foundry/use.md", &format!("{FM}See [[Alpha]].\n"));
        assert_eq!(review.result.status, GateStatus::Ok, "{:?}", review.result.reasons);
        let links = &review.result.enriched.as_ref().unwrap().links_retargeted;
        assert_eq!(links[0].from, "[[Alpha]]");
        assert_eq!(links[0].to, "[[notes/Alpha]]");
        assert!(review.text.contains("[[notes/Alpha]]"));
        assert_schema(&review.result);
        let _ = std::fs::remove_dir_all(&root);
    }
}
