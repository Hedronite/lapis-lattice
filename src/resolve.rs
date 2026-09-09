//! `lapis resolve`: a wikilink (or a lattice `dst_raw`) → the vault path it
//! points at, Obsidian-style: an exact vault-relative path wins, else a unique
//! basename match anywhere in the scan set; several matches are a collision.

use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::{notes, tasks};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    /// The link as given.
    pub link: String,
    /// Link target with `[[ ]]`, alias, anchor and `.md` stripped.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// Vault-relative path when exactly one note matches.
    pub path: Option<String>,
    pub resolved: bool,
    /// `exact`, `basename`, `collision`, or `dangling`.
    pub how: &'static str,
    /// Every candidate when there is more than one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<String>,
}

/// Split `[[target|alias#anchor]]` (any of the decorations optional).
pub fn parse_link(link: &str) -> (String, Option<String>, Option<String>) {
    let inner = link.trim().trim_start_matches("[[").trim_end_matches("]]").trim();
    let (rest, alias) = match inner.split_once('|') {
        Some((t, a)) => (t, Some(a.trim().to_string()).filter(|a| !a.is_empty())),
        None => (inner, None),
    };
    let (target, anchor) = match rest.split_once('#') {
        Some((t, a)) => (t, Some(a.trim().to_string()).filter(|a| !a.is_empty())),
        None => (rest, None),
    };
    let target = target.trim().trim_end_matches(".md").trim_end_matches('/').to_string();
    (target, alias, anchor)
}

fn stem(rel: &str) -> &str {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.strip_suffix(".md").unwrap_or(base)
}

/// Resolve against `files` (vault-relative `.md` paths). Pure, for tests.
pub fn resolve_in(link: &str, files: &[String], vault_name: Option<&str>) -> Resolved {
    let (target, alias, anchor) = parse_link(link);
    let mut out = Resolved {
        link: link.to_string(),
        target: target.clone(),
        alias,
        anchor,
        path: None,
        resolved: false,
        how: "dangling",
        candidates: vec![],
    };
    if target.is_empty() {
        return out;
    }
    // Some indexers prefix the vault folder name (`Atrium/Cross-References/X`).
    let mut targets = vec![target.clone()];
    if let Some(v) = vault_name
        && let Some(rest) = target.strip_prefix(&format!("{v}/"))
    {
        targets.push(rest.to_string());
    }
    for t in &targets {
        let want = format!("{t}.md");
        if let Some(f) = files.iter().find(|f| f.as_str() == want || f.eq_ignore_ascii_case(&want)) {
            out.path = Some(f.clone());
            out.resolved = true;
            out.how = "exact";
            return out;
        }
    }
    let base = stem(targets.last().unwrap()).to_lowercase();
    let mut hits: Vec<&String> = files.iter().filter(|f| stem(f).to_lowercase() == base).collect();
    hits.sort_by_key(|f| (f.matches('/').count(), f.as_str()));
    match hits.len() {
        0 => {}
        1 => {
            out.path = Some(hits[0].clone());
            out.resolved = true;
            out.how = "basename";
        }
        _ => {
            out.how = "collision";
            out.candidates = hits.into_iter().cloned().collect();
        }
    }
    out
}

/// Resolve against the vault on disk (same scan set as tasks / the TUI tree).
pub fn resolve(root: &Path, link: &str) -> Result<Resolved> {
    let files = tasks::scan_files(root, None)?;
    let vault_name = root.file_name().map(|s| s.to_string_lossy().to_string());
    let mut r = resolve_in(link, &files, vault_name.as_deref());
    // A direct path that exists but sits outside the scan set (e.g. a PDF) still resolves.
    if !r.resolved && r.how == "dangling" {
        for cand in [r.target.clone(), format!("{}.md", r.target)] {
            if let Ok((rel, abs)) = notes::resolve(root, &cand)
                && abs.is_file()
            {
                r.path = Some(rel);
                r.resolved = true;
                r.how = "exact";
                break;
            }
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> {
        ["Cross-References/Hedronite-Capital.md", "foundry/lapis/SPEC.md", "notes/SPEC.md", "index.md"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn parses_alias_anchor_and_brackets() {
        // canonical Obsidian order: target#anchor|alias
        assert_eq!(parse_link("[[A/B#sec|shown]]"), ("A/B".into(), Some("shown".into()), Some("sec".into())));
        assert_eq!(parse_link("[[A/B|shown]]"), ("A/B".into(), Some("shown".into()), None));
        assert_eq!(parse_link("plain"), ("plain".into(), None, None));
        assert_eq!(parse_link("[[x.md]]"), ("x".into(), None, None));
        assert_eq!(parse_link("[[]]"), (String::new(), None, None));
    }

    #[test]
    fn exact_basename_collision_dangling() {
        let f = files();
        let r = resolve_in("[[Cross-References/Hedronite-Capital]]", &f, Some("Atrium"));
        assert_eq!(
            (r.resolved, r.how, r.path.as_deref()),
            (true, "exact", Some("Cross-References/Hedronite-Capital.md"))
        );
        // vault-name prefix as emitted in some dst_raw values
        let r = resolve_in("Atrium/Cross-References/Hedronite-Capital", &f, Some("Atrium"));
        assert_eq!((r.how, r.path.as_deref()), ("exact", Some("Cross-References/Hedronite-Capital.md")));
        let r = resolve_in("[[hedronite-capital|Cap]]", &f, None);
        assert_eq!((r.how, r.alias.as_deref()), ("basename", Some("Cap")));
        let r = resolve_in("[[SPEC]]", &f, None);
        assert!(!r.resolved);
        assert_eq!(r.how, "collision");
        // shallowest path first, like Obsidian's shortest-path rule
        assert_eq!(r.candidates, ["notes/SPEC.md", "foundry/lapis/SPEC.md"]);
        let r = resolve_in("aes_schema_genesis_canon", &f, None);
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!((r.resolved, r.how, r.path), (false, "dangling", None));
        assert!(j["path"].is_null());
        assert_eq!(j["resolved"], false);
    }
}
