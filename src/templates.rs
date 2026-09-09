//! Templates: built-ins shipped in the binary plus `.lapis/templates/*.md`.
//!
//! Every built-in starts with a HAL create-set overlay (SPEC.md § Templates).
//! Placeholders: `{{title}}`, `{{date}}`, `{{date:YYYY-MM-DD}}`, `{{week}}`,
//! `{{month}}`, `{{slug}}`, `{{director}}`, `{{cursor}}`.

use std::path::Path;

use serde::Serialize;

use crate::error::{LapisError, Result};
use crate::hal;
use crate::notes;

#[derive(Debug, Clone, Serialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    /// Where the template expects to land (hint for pickers), vault-relative.
    pub target: String,
    pub builtin: bool,
    #[serde(skip)]
    pub text: String,
}

const BUILTINS: &[(&str, &str, &str, &str)] = &[
    ("builtin.note", "Plain note", "inbox/", "---\ntype: note\n---\n# {{title}}\n\n{{cursor}}\n"),
    (
        "builtin.daily",
        "Daily note",
        "Daily/{{date}}.md",
        "---\ntype: daily-note\ndomain: vault-operation\n---\n# {{date}}\n\n## Tasks\n\n- [ ] {{cursor}}\n\n## Notes\n\n## Log\n",
    ),
    (
        "builtin.weekly",
        "Weekly note",
        "Weekly/{{week}}.md",
        "---\ntype: weekly-note\ndomain: vault-operation\n---\n# Week {{week}}\n\n## Focus\n\n- [ ] {{cursor}}\n\n## Review\n",
    ),
    (
        "builtin.monthly",
        "Monthly note",
        "Monthly/{{month}}.md",
        "---\ntype: monthly-note\ndomain: vault-operation\n---\n# {{month}}\n\n## Goals\n\n- [ ] {{cursor}}\n\n## Retro\n",
    ),
    (
        "builtin.mail_drop",
        "Mail-room drop",
        "agents/mail_room/{{director}}/{{date}}-{{slug}}.md",
        "---\ntype: mail-drop\ndoc_type: mail-drop\ndomain: mail-room\nstatus: dropped\nto: {{director}}\n---\n# {{title}}\n\n**To:** {{director}}  \n**Date:** {{date}}\n\n{{cursor}}\n",
    ),
    (
        "builtin.foundry_status",
        "Foundry status",
        "foundry/<product>/STATUS.md",
        "---\ntype: status\nartifact_class: status\nstatus: live\n---\n# {{title}} — status {{date}}\n\n## Plate\n\n| Surface | State |\n|---|---|\n| | |\n\n## Decisions\n\n{{cursor}}\n",
    ),
    (
        "builtin.adr",
        "Architecture decision record",
        "<current folder>/",
        "---\ntype: adr\nstatus: proposed\n---\n# ADR: {{title}}\n\nDate: {{date}}\n\n## Context\n\n{{cursor}}\n\n## Decision\n\n## Consequences\n",
    ),
];

pub fn builtins() -> Vec<Template> {
    BUILTINS
        .iter()
        .map(|(id, name, target, text)| Template {
            id: id.to_string(),
            name: name.to_string(),
            target: target.to_string(),
            builtin: true,
            text: text.to_string(),
        })
        .collect()
}

/// Built-ins first, then `.lapis/templates/*.md` (id = file stem).
pub fn list(root: &Path) -> Vec<Template> {
    let mut out = builtins();
    let dir = root.join(".lapis/templates");
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut custom: Vec<Template> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let stem = name.strip_suffix(".md")?.to_string();
                let text = std::fs::read_to_string(e.path()).ok()?;
                let title = hal::title_from_hal(&hal::parse(&text).hal).unwrap_or_else(|| stem.clone());
                Some(Template { id: stem, name: title, target: String::new(), builtin: false, text })
            })
            .collect();
        custom.sort_by(|a, b| a.id.cmp(&b.id));
        out.extend(custom);
    }
    out
}

pub fn find(root: &Path, id: &str) -> Result<Template> {
    let id = id.trim();
    if let Some(t) = builtins().into_iter().find(|t| t.id == id || t.id.strip_prefix("builtin.") == Some(id))
    {
        return Ok(t);
    }
    let clean = notes::clean_rel(id)?;
    let file = root.join(".lapis/templates").join(format!("{clean}.md"));
    let text = std::fs::read_to_string(&file).map_err(|_| {
        LapisError::Usage(format!(
            "template not found: {id} (built-ins: {})",
            BUILTINS.iter().map(|b| b.0).collect::<Vec<_>>().join(", ")
        ))
    })?;
    Ok(Template { id: clean, name: id.to_string(), target: String::new(), builtin: false, text })
}

#[derive(Debug, Clone, Default)]
pub struct Vars {
    pub title: String,
    pub date: String,
    pub week: String,
    pub month: String,
    pub director: String,
}

pub fn substitute(text: &str, v: &Vars) -> String {
    text.replace("{{title}}", &v.title)
        .replace("{{date:YYYY-MM-DD}}", &v.date)
        .replace("{{date}}", &v.date)
        .replace("{{week}}", &v.week)
        .replace("{{month}}", &v.month)
        .replace("{{slug}}", &crate::write::slug(&v.title))
        .replace("{{director}}", &v.director)
        .replace("{{cursor}}", "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_present_and_hal_wrapped() {
        for id in ["builtin.daily", "builtin.mail_drop", "builtin.weekly", "builtin.monthly"] {
            let t = builtins().into_iter().find(|t| t.id == id).expect(id);
            let p = hal::parse(&t.text);
            assert!(p.hal_valid && p.hal.contains_key("type"), "{id} must start with a HAL overlay");
        }
    }

    #[test]
    fn find_by_short_or_full_id_and_custom() {
        let dir = std::env::temp_dir().join(format!("lapis-tpl-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".lapis/templates")).unwrap();
        std::fs::write(dir.join(".lapis/templates/rfc.md"), "---\nname: RFC\ntype: rfc\n---\n# {{title}}\n")
            .unwrap();
        assert_eq!(find(&dir, "daily").unwrap().id, "builtin.daily");
        assert_eq!(find(&dir, "builtin.mail_drop").unwrap().id, "builtin.mail_drop");
        let c = find(&dir, "rfc").unwrap();
        assert!(!c.builtin);
        assert!(find(&dir, "nope").is_err());
        let all = list(&dir);
        assert!(all.iter().any(|t| t.id == "rfc" && t.name == "RFC"));
        assert!(all.iter().filter(|t| t.builtin).count() >= 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn substitution() {
        let v = Vars {
            title: "Hello World".into(),
            date: "2026-09-09".into(),
            week: "2026-W37".into(),
            month: "2026-09".into(),
            director: "Marci".into(),
        };
        let s = substitute("{{director}}/{{date}}-{{slug}} {{week}} {{month}} {{title}}{{cursor}}", &v);
        assert_eq!(s, "Marci/2026-09-09-hello-world 2026-W37 2026-09 Hello World");
    }
}
