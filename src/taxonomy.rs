//! Create-set classification: `domain` and `doc_type` for a new path.
//!
//! SPEC.md § Create-set: "via `taxonomy.py` rules ported or called". We call:
//! the canon lives in `projects/atrium-lattice/taxonomy.py` and is run under
//! the lattice venv as a child process. `lapis-notes` never spawns one, so
//! the create-set takes this route as its classify hook. If that venv is not
//! present (a vault without a lattice, CI, tests) the hook answers `None` and
//! `lapis-notes` falls back to the first path segment.

use std::path::Path;
use std::process::Command;

use lapis_notes::Classified;

const LATTICE_REL: &str = "projects/atrium-lattice";

pub fn via_taxonomy_py(vault_root: &Path, rel: &str, requested_type: Option<&str>) -> Option<Classified> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_lattice_the_hook_answers_none() {
        // Unique per call: pid alone repeats across parallel tests in one
        // binary, and the clock is coarser than a nanosecond.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("lapis-tax-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(via_taxonomy_py(&dir, "inbox/a.md", Some("capture")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
