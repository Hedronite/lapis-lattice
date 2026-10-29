//! Create-set classification: `domain` and `doc_type` for a new path.
//!
//! SPEC.md § Create-set: "via `taxonomy.py` rules ported or called". Calling
//! the canon must not shell out from a library — a phone cannot spawn
//! processes — so classification is caller-supplied: the CLI passes the
//! `taxonomy.py` child-process route as a hook into [`create_set`] and
//! [`create_new`], a phone passes none. The crate itself carries only the
//! pure fallback and the [`Classified`] shape both routes move through.

/// What a path classifies to, and where the answer came from, reported in
/// `--json` so an agent knows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub domain: Option<String>,
    pub doc_type: Option<String>,
    /// The caller's route, e.g. `"taxonomy.py"`, or `"fallback"` when the
    /// crate classified the path itself.
    pub source: &'static str,
}
/// First path segment, lowercased; a document type only when one was requested.
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
    fn fallback_rules() {
        assert_eq!(
            fallback("Inbox/Drop/x.md", Some("note")),
            Classified { domain: Some("inbox".into()), doc_type: Some("note".into()), source: "fallback" }
        );
        assert_eq!(
            fallback("Single.md", None),
            Classified { domain: None, doc_type: None, source: "fallback" },
            "a root-level file has no first segment to speak of"
        );
        assert_eq!(fallback("projects/", None).domain.as_deref(), Some("projects"));
    }
}
