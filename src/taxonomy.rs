//! Create-set classification: `domain` and `doc_type` for a new path.
//!
//! SPEC.md § Create-set: "via `taxonomy.py` rules ported or called". We call:
//! the canon lives in `projects/atrium-lattice/taxonomy.py` and is run under
//! the lattice venv as a child process. If that venv is not present (a vault
//! without a lattice, CI, tests) we fall back to the first path segment.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    /// `"taxonomy.py"` or `"fallback"`, reported in `--json` so an agent knows.
    pub source: &'static str,
}

const LATTICE_REL: &str = "projects/atrium-lattice";

pub fn classify(vault_root: &Path, rel: &str, requested_type: Option<&str>) -> Classified {
    if let Some(c) = via_taxonomy_py(vault_root, rel, requested_type) {
        return c;
    }
    fallback(rel, requested_type)
}

fn via_taxonomy_py(vault_root: &Path, rel: &str, requested_type: Option<&str>) -> Option<Classified> {
    let lattice = vault_root.join(LATTICE_REL);
    let py = lattice.join(".venv/bin/python");
    if !py.is_file() || !lattice.join("taxonomy.py").is_file() {
        return None;
    }
    let script = "import sys, json, taxonomy\n\
                  p = sys.argv[1]; cur = sys.argv[2] or None\n\
                  print(json.dumps({'domain': taxonomy.domain_for_path(p), \
                  'doc_type': taxonomy.classify_doc_type_for_path(p, cur)}))";
    let out = Command::new(&py)
        .arg("-c")
        .arg(script)
        .arg(rel)
        .arg(requested_type.unwrap_or(""))
        .current_dir(&lattice)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Some(Classified { domain: get("domain"), doc_type: get("doc_type"), source: "taxonomy.py" })
}

pub fn fallback(rel: &str, requested_type: Option<&str>) -> Classified {
    let first = rel.split('/').next().filter(|s| !s.is_empty() && rel.contains('/'));
    Classified {
        domain: first.map(|s| s.to_ascii_lowercase()),
        doc_type: requested_type.map(str::to_string),
        source: "fallback",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_uses_first_segment() {
        let c = fallback("foundry/lapis/x.md", None);
        assert_eq!(c.domain.as_deref(), Some("foundry"));
        assert_eq!(c.doc_type, None);
        let c = fallback("ROOT.md", Some("note"));
        assert_eq!(c.domain, None);
        assert_eq!(c.doc_type.as_deref(), Some("note"));
    }

    #[test]
    fn classify_without_lattice_is_fallback() {
        let dir = std::env::temp_dir().join(format!("lapis-tax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = classify(&dir, "inbox/a.md", Some("capture"));
        assert_eq!(c.source, "fallback");
        assert_eq!(c.domain.as_deref(), Some("inbox"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
