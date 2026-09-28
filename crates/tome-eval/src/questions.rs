//! `evals/tome/questions.jsonl` (+ the non-scored fail-closed probe file).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Snippet {
    pub page: u32,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExpectError {
    pub codes: Vec<String>,
    #[serde(default)]
    pub also_acceptable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Question {
    #[serde(alias = "qid")]
    pub id: String,
    pub doc: String,
    pub doc_sha256: String,
    pub question: String,
    #[serde(alias = "gold_answer")]
    pub expected_answer: Option<String>,
    /// Inclusive ranges of 1-based physical PDF pages.
    pub gold_pages: Vec<[u32; 2]>,
    pub answer_type: String,
    pub difficulty: String,
    #[serde(default)]
    pub snippets: Vec<Snippet>,
    #[serde(default = "yes")]
    pub scored: bool,
    #[serde(default)]
    pub expect_error: Option<ExpectError>,
}

fn yes() -> bool {
    true
}

impl Question {
    pub fn gold_page_list(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.gold_pages.iter().flat_map(|[a, b]| *a..=*b).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

pub fn load(path: &Path) -> Result<Vec<Question>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let q: Question =
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
        out.push(q);
    }
    Ok(out)
}

/// Structural checks the committed set must pass (ids unique, pages sane, snippets inside gold).
pub fn check(qs: &[Question]) -> Vec<String> {
    let mut errs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for q in qs {
        if !seen.insert(q.id.clone()) {
            errs.push(format!("{}: duplicate id", q.id));
        }
        if q.doc_sha256.len() != 64 || !q.doc_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            errs.push(format!("{}: doc_sha256 is not a sha256", q.id));
        }
        if q.scored {
            if q.gold_pages.is_empty() || q.expected_answer.as_deref().unwrap_or("").is_empty() {
                errs.push(format!("{}: scored question needs gold_pages and expected_answer", q.id));
            }
            if q.snippets.is_empty() {
                errs.push(format!("{}: scored question needs a supporting snippet", q.id));
            }
        } else if q.expect_error.is_none() {
            errs.push(format!("{}: non-scored probe needs expect_error", q.id));
        }
        for [a, b] in &q.gold_pages {
            if *a == 0 || a > b {
                errs.push(format!("{}: bad gold range [{a},{b}]", q.id));
            }
        }
        let gold = q.gold_page_list();
        for s in &q.snippets {
            if !gold.contains(&s.page) {
                errs.push(format!("{}: snippet page {} outside gold", q.id, s.page));
            }
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tome")
    }

    #[test]
    fn committed_question_set_is_well_formed() {
        let qs = load(&root().join("questions.jsonl")).unwrap();
        assert_eq!(qs.len(), 20);
        assert!(qs.iter().all(|q| q.scored));
        assert_eq!(check(&qs), Vec::<String>::new());
        let mut per_doc = std::collections::BTreeMap::new();
        for q in &qs {
            *per_doc.entry(q.doc.clone()).or_insert(0) += 1;
        }
        assert_eq!(per_doc.len(), 4);
        assert!(per_doc.values().all(|n| *n >= 3));
        assert!(!qs.iter().any(|q| q.doc.contains("Red Team Guide")), "fail-closed doc is not scored");
    }

    #[test]
    fn fail_closed_probe_is_separate_and_expects_an_error() {
        let fc = load(&root().join("failclosed.jsonl")).unwrap();
        assert_eq!(fc.len(), 1);
        assert!(!fc[0].scored);
        assert!(fc[0].doc.contains("The Red Team Guide.pdf"));
        assert!(fc[0].expect_error.as_ref().unwrap().codes.contains(&"no_structure".to_string()));
        assert_eq!(check(&fc), Vec::<String>::new());
    }

    #[test]
    fn gold_ranges_expand_inclusively() {
        let q: Question = serde_json::from_str(
            r#"{"qid":"x","doc":"d","doc_sha256":"00","question":"q","gold_answer":"a","gold_pages":[[3,4],[9,9]],"answer_type":"fact","difficulty":"multi_section"}"#,
        )
        .unwrap();
        assert_eq!(q.gold_page_list(), vec![3, 4, 9]);
        assert_eq!(q.id, "x");
    }
}
