//! Jev (System One) as SCORER and JUDGE only: it grades correctness, scores tree
//! children for `walk`, and replays lapis' shadow rerank on the baseline page.
//! It never writes an answer. Missing transport or low confidence ⇒ explicit
//! `unavailable` / `uncertain`, never a guessed grade. The walk judge alone ranks
//! on score with no confidence gate and records confidence (spike policy).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::JevCfg;
use crate::contract::{Candidate, Judge, TomeError};
use crate::record::{CandidateScore, Correctness};
use lapis_lattice::Hit;

#[derive(Debug, Clone)]
pub enum Transport {
    None {
        reason: String,
    },
    Http {
        endpoint: String,
        key: String,
    },
    /// Canned replies, front to back. Tests only.
    Fake(std::sync::Arc<Mutex<Vec<Value>>>),
}

#[derive(Debug, Clone)]
pub struct SystemOne {
    pub transport: Transport,
    pub floor: f64,
    pub timeout: Duration,
}

impl SystemOne {
    pub fn from_config(cfg: &JevCfg) -> Self {
        let transport = match cfg.transport.as_str() {
            "http" => match std::env::var(&cfg.key_env) {
                Ok(k) if !k.trim().is_empty() => Transport::Http { endpoint: cfg.endpoint.clone(), key: k },
                _ => Transport::None { reason: format!("{}_absent", cfg.key_env.to_lowercase()) },
            },
            _ => Transport::None { reason: "jev.transport=none".into() },
        };
        Self { transport, floor: cfg.confidence_floor, timeout: Duration::from_secs(cfg.timeout_s) }
    }

    pub fn fake(replies: Vec<Value>) -> Self {
        Self {
            transport: Transport::Fake(std::sync::Arc::new(Mutex::new(replies))),
            floor: 0.6,
            timeout: Duration::from_secs(1),
        }
    }

    pub fn name(&self) -> &'static str {
        match self.transport {
            Transport::None { .. } => "none",
            Transport::Http { .. } => "http",
            Transport::Fake(_) => "fake",
        }
    }

    pub async fn decide(&self, state: &str, questions: Value) -> Result<Value, String> {
        match &self.transport {
            Transport::None { reason } => Err(reason.clone()),
            Transport::Fake(q) => {
                let mut q = q.lock().map_err(|_| "fake lock".to_string())?;
                if q.is_empty() { Err("fake exhausted".into()) } else { Ok(q.remove(0)) }
            }
            Transport::Http { endpoint, key } => {
                let client =
                    reqwest::Client::builder().timeout(self.timeout).build().map_err(|e| e.to_string())?;
                let resp = client
                    .post(endpoint)
                    .header("Authorization", format!("Bearer {key}"))
                    .header("Accept", "application/json")
                    .json(&json!({ "model": "jev-latest", "state": state, "questions": questions }))
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                let status = resp.status();
                let text = resp.text().await.map_err(|e| e.to_string())?;
                if !status.is_success() {
                    return Err(format!(
                        "systemone {status}: {}",
                        text.chars().take(200).collect::<String>()
                    ));
                }
                serde_json::from_str(&text).map_err(|e| format!("systemone json: {e}"))
            }
        }
    }
}

fn answers(body: &Value) -> &Value {
    body.get("answers").unwrap_or(body)
}

// ---- grading ---------------------------------------------------------------

pub fn grade_questions() -> Value {
    json!({
        "grade": {
            "type": "choice",
            "instructions": "Compare the candidate answer with the reference answer to the question. Judge factual agreement only; ignore wording and length.",
            "criteria": {
                "exact": "The candidate states every fact in the reference answer and nothing that contradicts it",
                "partial": "The candidate states some but not all reference facts, or is imprecise, without contradicting the reference",
                "wrong": "The candidate contradicts the reference, misses its key facts, or says the answer is not in the passages"
            }
        }
    })
}

pub fn grade_state(question: &str, reference: &str, candidate: &str) -> String {
    format!("Question: {question}\n\nReference answer: {reference}\n\nCandidate answer: {candidate}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradeOut {
    /// graded | unavailable | uncertain
    pub status: &'static str,
    pub correctness: Option<Correctness>,
    pub confidence: Option<f64>,
}

pub fn parse_grade(body: &Value, floor: f64) -> GradeOut {
    let a = answers(body);
    let choice = a.pointer("/grade/choice").and_then(Value::as_str);
    let conf = a.pointer("/grade/confidence").and_then(Value::as_f64);
    let c = match choice {
        Some("exact") => Some(Correctness::Exact),
        Some("partial") => Some(Correctness::Partial),
        Some("wrong") => Some(Correctness::Wrong),
        _ => None,
    };
    match (c, conf) {
        (Some(_), Some(x)) if x < floor => {
            GradeOut { status: "uncertain", correctness: None, confidence: conf }
        }
        (Some(c), _) => GradeOut { status: "graded", correctness: Some(c), confidence: conf },
        (None, _) => GradeOut { status: "uncertain", correctness: None, confidence: conf },
    }
}

impl SystemOne {
    /// Blind grade: the grader sees question, reference and candidate, never the arm.
    pub async fn grade(&self, question: &str, reference: &str, candidate: &str) -> GradeOut {
        match self.decide(&grade_state(question, reference, candidate), grade_questions()).await {
            Ok(body) => parse_grade(&body, self.floor),
            Err(_) => GradeOut { status: "unavailable", correctness: None, confidence: None },
        }
    }
}

// ---- baseline rerank (replays lapis `--rerank-jev` on the same-PDF page) ----

/// Same System One questions lapis ships in `src/jev.rs` (Noul + Score + citation Choice).
pub fn rerank_questions() -> Value {
    json!({
        "answers": { "type": "noul", "instructions": "Does this chunk answer the search query?",
            "criteria": { "true": "The chunk contains information that directly answers the query",
                          "false": "The chunk is off-topic or does not answer the query" } },
        "relevance": { "type": "score", "instructions": "How relevant is this chunk to the query?",
            "criteria": [ "Unrelated; no useful overlap with the query", "Tangentially related; shared terms only",
                          "Partially useful; some supporting detail", "Directly answers the query" ] },
        "cite": { "type": "choice", "instructions": "How does this chunk relate to the query as a citation?",
            "criteria": { "supports": "The chunk supports an answer to the query",
                          "contradicts": "The chunk contradicts or refutes an answer to the query",
                          "unrelated": "The chunk is not a citation for the query" } }
    })
}

pub fn hit_state(query: &str, hit: &Hit) -> String {
    let snippet: String = hit.snippet.as_deref().unwrap_or("").chars().take(2000).collect();
    format!(
        "Search query: {query}\n\nNote: {}\nTitle: {}\nHeading: {}\n\nChunk:\n{snippet}",
        hit.path,
        hit.title,
        hit.heading.as_deref().unwrap_or("")
    )
}

/// `Some(relevance + answers)` when judged with confidence ≥ floor, else `None`.
pub fn rerank_score(body: &Value, floor: f64) -> Option<f64> {
    let a = answers(body);
    let noul = a.pointer("/answers/noul").and_then(Value::as_f64);
    let score = a.pointer("/relevance/score").and_then(Value::as_f64);
    let confs = ["/answers/confidence", "/relevance/confidence", "/cite/confidence"];
    let conf = confs.iter().filter_map(|p| a.pointer(p).and_then(Value::as_f64)).reduce(f64::min);
    if (noul.is_none() && score.is_none()) || conf.is_some_and(|c| c < floor) {
        return None;
    }
    Some(score.unwrap_or(0.0) + noul.unwrap_or(0.0))
}

impl SystemOne {
    /// Reorders only when every hit was judged (lapis shadow semantics). Returns
    /// `judged` | `uncertain` | `unavailable` and the number of System One calls.
    pub async fn rerank(&self, query: &str, hits: &mut Vec<Hit>) -> (&'static str, u32) {
        if matches!(self.transport, Transport::None { .. }) || hits.is_empty() {
            return ("unavailable", 0);
        }
        let mut scores = Vec::with_capacity(hits.len());
        let mut calls = 0;
        for h in hits.iter() {
            calls += 1;
            scores.push(match self.decide(&hit_state(query, h), rerank_questions()).await {
                Ok(b) => rerank_score(&b, self.floor),
                Err(_) => None,
            });
        }
        if scores.iter().all(Option::is_none) {
            return ("unavailable", calls);
        }
        if scores.iter().any(Option::is_none) {
            return ("uncertain", calls);
        }
        let mut idx: Vec<(usize, f64)> = scores.into_iter().map(|s| s.unwrap_or(0.0)).enumerate().collect();
        idx.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let old = std::mem::take(hits);
        *hits = idx.into_iter().map(|(i, _)| old[i].clone()).collect();
        ("judged", calls)
    }
}

// ---- walk judge ------------------------------------------------------------

pub fn child_questions() -> Value {
    json!({
        "section": { "type": "score",
            "instructions": "How likely is it that this book section contains the answer to the question?",
            "criteria": [ "Unrelated to the question", "Shares terms only; unlikely to hold the answer",
                          "Related; may hold part of the answer", "Very likely holds the answer" ] }
    })
}

pub fn child_state(query: &str, c: &Candidate) -> String {
    format!(
        "Question: {query}\n\nSection: {}\nPages: {}-{}\nLead:\n{}",
        c.title,
        c.page_start,
        c.page_end,
        c.lead.chars().take(1200).collect::<String>()
    )
}

/// Fixed reporting floor for `would_fail_closed_at_0_6` (Eli ruling 2026-09-28).
/// It matches `[jev] confidence_floor` and is never tuned to eval data.
pub const FAIL_CLOSED_FLOOR: f64 = 0.6;

/// `tome_tree::Judge` over System One. The trait is sync, so the harness runs `walk`
/// on a blocking thread of a multi-thread runtime and this blocks on the handle.
/// Jev only scores; it never answers.
///
/// Spike policy (Eli ruling 2026-09-28): the walk ranks on score ONLY, with no
/// confidence gate. Every candidate's score and confidence is recorded, so the
/// run can report how many walks *would* have failed closed at the 0.6 floor.
/// This is a small eval-side adapter. Once `tome_tree::Walk` carries per-candidate
/// score/confidence and lapis has a `[tome]` floor with record-and-down-weight
/// mode (crate agent), read them from `Walk` instead. A missing or out-of-range
/// score is malformed output, not low confidence, so it still fails closed.
pub struct JevJudge {
    pub so: SystemOne,
    pub handle: tokio::runtime::Handle,
    pub calls: AtomicU32,
    pub prompt_chars: AtomicU32,
    pub candidates: Mutex<Vec<CandidateScore>>,
}

impl JevJudge {
    pub fn new(so: SystemOne, handle: tokio::runtime::Handle) -> Self {
        Self {
            so,
            handle,
            calls: AtomicU32::new(0),
            prompt_chars: AtomicU32::new(0),
            candidates: Mutex::new(Vec::new()),
        }
    }

    /// Candidates judged so far, in walk order.
    pub fn take_candidates(&self) -> Vec<CandidateScore> {
        self.candidates.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
    }
}

fn unavailable(reason: impl Into<String>) -> TomeError {
    TomeError::JudgeUnavailable { reason: reason.into() }
}

impl Judge for JevJudge {
    fn score(&self, query: &str, c: &Candidate) -> crate::contract::Result<u8> {
        if let Transport::None { reason } = &self.so.transport {
            return Err(unavailable(reason.clone()));
        }
        let state = child_state(query, c);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.prompt_chars.fetch_add(state.len() as u32, Ordering::Relaxed);
        let body = self.handle.block_on(self.so.decide(&state, child_questions())).map_err(unavailable)?;
        let a = answers(&body);
        let score = a.pointer("/section/score").and_then(Value::as_f64);
        let conf = a.pointer("/section/confidence").and_then(Value::as_f64);
        let ok = score.filter(|s| (0.0..=3.0).contains(s)).map(|s| s.round() as u8);
        if let Ok(mut v) = self.candidates.lock() {
            v.push(CandidateScore {
                node_id: c.id.0.clone(),
                title: c.title.clone(),
                page_start: c.page_start,
                page_end: c.page_end,
                score: ok,
                confidence: conf,
            });
        }
        match (ok, score) {
            (Some(s), _) => Ok(s),
            (None, Some(s)) => Err(unavailable(format!("score {s} outside 0..=3 for {}", c.id))),
            (None, None) => Err(unavailable(format!(
                "no score for {} (confidence={conf:?}, floor={})",
                c.id, self.so.floor
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, rank: u32) -> Hit {
        Hit {
            path: path.into(),
            kind: "pdf".into(),
            title: path.into(),
            heading: None,
            snippet: Some("s".into()),
            rank,
            score: 0.0,
            domain: None,
            doc_type: None,
            tags: vec![],
            chunk_id: Some(rank as i64),
            chunk_index: None,
            jev: None,
        }
    }

    fn rr(noul: f64, score: f64, conf: f64) -> Value {
        json!({"answers":{"answers":{"noul":noul,"confidence":conf},"relevance":{"score":score,"confidence":conf},"cite":{"choice":"supports","confidence":conf}}})
    }

    #[test]
    fn grade_is_blind_and_fails_closed() {
        let s = grade_state("q", "ref", "cand");
        assert!(!s.contains("baseline") && !s.contains("tome"));
        let g = parse_grade(&json!({"answers":{"grade":{"choice":"partial","confidence":0.9}}}), 0.6);
        assert_eq!((g.status, g.correctness), ("graded", Some(Correctness::Partial)));
        let low = parse_grade(&json!({"grade":{"choice":"exact","confidence":0.1}}), 0.6);
        assert_eq!((low.status, low.correctness), ("uncertain", None));
        assert_eq!(parse_grade(&json!({}), 0.6).status, "uncertain");
    }

    #[tokio::test]
    async fn missing_transport_is_unavailable_not_a_grade() {
        let so = SystemOne {
            transport: Transport::None { reason: "x".into() },
            floor: 0.6,
            timeout: Duration::from_secs(1),
        };
        let g = so.grade("q", "r", "c").await;
        assert_eq!((g.status, g.correctness), ("unavailable", None));
        let mut hits = vec![hit("a", 1), hit("b", 2)];
        assert_eq!(so.rerank("q", &mut hits).await, ("unavailable", 0));
        assert_eq!(hits[0].path, "a");
    }

    #[tokio::test]
    async fn rerank_reorders_only_when_all_judged() {
        let so = SystemOne::fake(vec![rr(0.1, 0.0, 0.9), rr(0.9, 3.0, 0.9)]);
        let mut hits = vec![hit("weak", 1), hit("strong", 2)];
        assert_eq!(so.rerank("q", &mut hits).await, ("judged", 2));
        assert_eq!(hits[0].path, "strong");
        let so = SystemOne::fake(vec![rr(0.9, 3.0, 0.9), json!({})]);
        let mut hits = vec![hit("a", 1), hit("b", 2)];
        assert_eq!(so.rerank("q", &mut hits).await.0, "uncertain");
        assert_eq!(hits[0].path, "a");
    }

    #[test]
    fn walk_judge_ranks_on_score_only() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let c = Candidate {
            id: crate::contract::NodeId("0001".into()),
            title: "t".into(),
            lead: "l".into(),
            page_start: 1,
            page_end: 2,
            level: 1,
        };
        let j = JevJudge::new(
            SystemOne::fake(vec![json!({"section":{"score":2,"confidence":0.9}})]),
            rt.handle().clone(),
        );
        assert_eq!(j.score("q", &c).unwrap(), 2);
        let j = JevJudge::new(
            SystemOne::fake(vec![json!({"section":{"score":3,"confidence":0.2}})]),
            rt.handle().clone(),
        );
        assert_eq!(j.score("q", &c).unwrap(), 3, "score-only: low confidence is recorded, not gated");
        let rec = j.take_candidates();
        assert_eq!((rec[0].score, rec[0].confidence), (Some(3), Some(0.2)));
        assert!(rec[0].below(FAIL_CLOSED_FLOOR));
        let j =
            JevJudge::new(SystemOne::fake(vec![json!({"section":{"confidence":0.9}})]), rt.handle().clone());
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable", "missing score fails closed");
        assert_eq!(j.take_candidates()[0].score, None);
        let j = JevJudge::new(
            SystemOne::fake(vec![json!({"section":{"score":4,"confidence":0.9}})]),
            rt.handle().clone(),
        );
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable", "1..=4 scale fails closed");
        let none = SystemOne {
            transport: Transport::None { reason: "k".into() },
            floor: 0.6,
            timeout: Duration::from_secs(1),
        };
        let j = JevJudge::new(none, rt.handle().clone());
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable");
    }
}
