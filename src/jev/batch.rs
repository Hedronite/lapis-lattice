//! One System One call scores many tome candidates.
//!
//! The state asks for a JSON array of `{id, score, confidence}`. The questions
//! are one score rubric per candidate id, which is what System One actually
//! returns. A body that is only the single hit-rerank relevance score is not a
//! batch.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use tome_tree::{Assessment, Candidate, Result, TomeError};

const PAGE_LEAD: usize = 240;

/// Prompt for one batch call. Child titles stay in front of a capped page lead.
pub(super) fn state(query: &str, candidates: &[Candidate]) -> String {
    let mut out = format!(
        "Search query: {query}\n\n\
         These are table-of-contents nodes, not retrieved passages. \
         Reply with a JSON array. Each element is one object \
         {{\"id\",\"score\",\"confidence\"}} for a candidate below. \
         `id` is that candidate's id. `score` is an integer 0, 1, 2, or 3. \
         `confidence` is a number from 0 to 1 and does not drop a node. \
         Include every id you can score.\n\n"
    );
    for candidate in candidates {
        let lead: String = candidate.lead.chars().take(PAGE_LEAD).collect();
        out.push_str(&format!(
            "id: {}\ntitle: {}\npages: {}-{}\n",
            candidate.id, candidate.title, candidate.page_start, candidate.page_end
        ));
        if !candidate.child_titles.is_empty() {
            out.push_str("child titles:\n");
            for child in &candidate.child_titles {
                let title: String = child.title.chars().take(80).collect();
                out.push_str(&format!("- {title} (pp. {}–{})\n", child.page_start, child.page_end));
            }
        }
        out.push_str(&format!("lead:\n{lead}\n\n"));
    }
    out
}

/// One score question per candidate. Keys are the candidate ids, so a System
/// One answers map is already `{id → {score, confidence}}`.
pub(super) fn questions(candidates: &[Candidate]) -> Value {
    let mut map = Map::new();
    for candidate in candidates {
        map.insert(
            candidate.id.as_str().to_string(),
            json!({
                "type": "score",
                "instructions": format!(
                    "How relevant is section {} ({}) to the query? \
                     The batch reply is a JSON array of {{\"id\",\"score\",\"confidence\"}}, \
                     one object per candidate.",
                    candidate.id, candidate.title
                ),
                "criteria": [
                    "Unrelated; no useful overlap with the query",
                    "Tangentially related; shared terms only",
                    "Partially useful; some supporting detail",
                    "Directly answers the query"
                ]
            }),
        );
    }
    Value::Object(map)
}

/// Strict scores for the ids that came back. `None` slots are the ids the
/// batch did not score. No array and no per-id score is `parse`.
pub(super) fn assessments(body: &Value, candidates: &[Candidate]) -> Result<Vec<Option<Assessment>>> {
    if let Some(rows) = batch_array(body) {
        let slots = slots_from_rows(rows, candidates);
        if slots.iter().any(Option::is_some) {
            return Ok(slots);
        }
    }
    if let Some(slots) = slots_from_scores(body, candidates) {
        return Ok(slots);
    }
    Err(TomeError::Parse("batch response has no score array".into()))
}

fn batch_array(body: &Value) -> Option<&Vec<Value>> {
    if let Some(rows) = body.as_array() {
        return Some(rows);
    }
    for pointer in ["/answers", "/sections", "/answers/sections"] {
        if let Some(rows) = body.pointer(pointer).and_then(Value::as_array) {
            return Some(rows);
        }
    }
    None
}

#[derive(Deserialize)]
struct Row {
    id: String,
    score: f64,
    confidence: f64,
}

fn strict_row(value: &Value) -> Option<(String, Assessment)> {
    let row: Row = serde_json::from_value(value.clone()).ok()?;
    if !(0.0..=3.0).contains(&row.score) || !(0.0..=1.0).contains(&row.confidence) {
        return None;
    }
    Some((row.id, Assessment { score: row.score.round() as u8, confidence: Some(row.confidence) }))
}

fn slots_from_rows(rows: &[Value], candidates: &[Candidate]) -> Vec<Option<Assessment>> {
    let mut by_id = std::collections::BTreeMap::new();
    for row in rows {
        if let Some((id, assessment)) = strict_row(row)
            && !by_id.contains_key(&id)
        {
            by_id.insert(id, assessment);
        }
    }
    candidates.iter().map(|candidate| by_id.get(candidate.id.as_str()).copied()).collect()
}

/// System One answers keyed by the question id we sent (the candidate id).
fn slots_from_scores(body: &Value, candidates: &[Candidate]) -> Option<Vec<Option<Assessment>>> {
    let answers = body.get("answers").filter(|v| v.is_object()).unwrap_or(body);
    if !answers.is_object() {
        return None;
    }
    let mut any = false;
    let mut slots = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let key = candidate.id.as_str();
        let score = answers.pointer(&format!("/{key}/score")).and_then(Value::as_f64);
        let confidence = answers.pointer(&format!("/{key}/confidence")).and_then(Value::as_f64);
        match score.filter(|score| (0.0..=3.0).contains(score)) {
            Some(score) => {
                any = true;
                slots.push(Some(Assessment { score: score.round() as u8, confidence }));
            }
            None => slots.push(None),
        }
    }
    any.then_some(slots)
}
