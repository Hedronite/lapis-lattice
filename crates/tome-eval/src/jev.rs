//! Jev (System One) as SCORER and JUDGE only: it grades correctness, scores tree
//! children for `walk`, and replays lapis' shadow rerank on the baseline page.
//! It never writes an answer. Missing transport or low confidence ⇒ explicit
//! `unavailable` / `uncertain`, never a guessed grade. The walk judge alone ranks
//! on score with no confidence gate (spike policy); `Walk::judged` records confidence.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::JevCfg;
use crate::contract::{Assessment, Candidate, Judge, TomeError};
use crate::record::Correctness;
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

/// The batch request lapis sends. This is lapis' own `src/jev/batch.rs`, compiled
/// into this crate from the same file, so the eval judge and `lapis --features tome`
/// cannot drift apart. It needs only `tome_tree`, serde and serde_json. The flake
/// builds from the repo root (`src = ./.`), like the schema `include_str!`.
#[path = "../../../src/jev/batch.rs"]
mod batch;

/// Fixed reporting floor for `would_fail_closed_at_0_6` (Eli ruling 2026-09-28).
/// It matches `[jev] confidence_floor` and is never tuned to eval data.
pub const FAIL_CLOSED_FLOOR: f64 = 0.6;

/// `tome_tree::Judge` over System One, the same protocol as lapis' `JevJudge`
/// (`src/jev.rs`, feature `tome`). The trait is sync, so the harness runs `walk`
/// on a blocking thread of a multi-thread runtime and this blocks on the handle.
/// Jev only scores; it never answers.
///
/// - `assess`: one candidate, the shipped rerank questions, score 0..=3 plus the
///   minimum reported confidence.
/// - `score_batch`: one System One call per batch (`batch::state` +
///   `batch::questions`). Ids the reply did not score come back `None`; the
///   walk judges those one at a time and charges each call. A reply with no
///   array and no per-id score is `parse`, and the walk pre-ranks.
/// - `batch_cost` is 1 (the batch request only). The walk's root allowance is
///   the root count, capped at 512, not `root_calls`.
///
/// Spike policy (Eli ruling 2026-09-28): the walk ranks on score ONLY, with no
/// confidence gate. Per-candidate score and confidence come from `Walk::judged`.
/// A missing or out-of-range score, or a dead transport, is `judge_unavailable`.
pub struct JevJudge {
    pub so: SystemOne,
    pub handle: tokio::runtime::Handle,
    /// System One calls actually sent (a batch is one). Should equal `Walk::judge_calls`.
    pub calls: AtomicU32,
    pub prompt_chars: AtomicU32,
}

impl JevJudge {
    pub fn new(so: SystemOne, handle: tokio::runtime::Handle) -> Self {
        Self { so, handle, calls: AtomicU32::new(0), prompt_chars: AtomicU32::new(0) }
    }

    fn decide(&self, state: &str, questions: Value) -> crate::contract::Result<Value> {
        if let Transport::None { reason } = &self.so.transport {
            return Err(unavailable(reason.clone()));
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.prompt_chars.fetch_add(state.len() as u32, Ordering::Relaxed);
        self.handle.block_on(self.so.decide(state, questions)).map_err(unavailable)
    }
}

fn unavailable(reason: impl Into<String>) -> TomeError {
    TomeError::JudgeUnavailable { reason: reason.into() }
}

impl Judge for JevJudge {
    fn score(&self, query: &str, c: &Candidate) -> crate::contract::Result<u8> {
        Ok(self.assess(query, c)?.score)
    }

    fn assess(&self, query: &str, c: &Candidate) -> crate::contract::Result<Assessment> {
        let body = self.decide(&candidate_state(query, c), rerank_questions())?;
        relevance_assessment(&body)
    }

    fn score_batch(
        &self,
        query: &str,
        candidates: &[Candidate],
    ) -> crate::contract::Result<Vec<Option<Assessment>>> {
        if candidates.len() <= 1 {
            return candidates.iter().map(|c| self.assess(query, c).map(Some)).collect();
        }
        let body = self.decide(&batch::state(query, candidates), batch::questions(candidates))?;
        // Missing ids stay `None`. The walk judges them and counts each call.
        batch::assessments(&body, candidates)
    }

    fn batch_cost(&self, _candidates: &[Candidate]) -> u32 {
        // The batch request only. Fill-ins are separate `assess` calls.
        1
    }
}

/// Mirror of lapis `src/jev.rs` `candidate_state` (a private fn there; keep the two
/// in step). A TOC node is not a retrieved chunk, so the prompt says so.
pub fn candidate_state(query: &str, c: &Candidate) -> String {
    // Page lead and child titles are separate fields, so a long lead cannot
    // hide the titles.
    let mut out = format!(
        "Search query: {query}\n\n\
         This is a table-of-contents node, not a retrieved passage.\n\n\
         Section: {}\nPages: {}-{}\n",
        c.title, c.page_start, c.page_end
    );
    if !c.child_titles.is_empty() {
        out.push_str("\nChild titles:\n");
        for child in &c.child_titles {
            out.push_str(&format!("- {} (pp. {}–{})\n", child.title, child.page_start, child.page_end));
        }
    }
    out.push_str(&format!("\nLead:\n{}", c.lead));
    out
}

/// Mirror of lapis `src/jev.rs` `relevance_assessment`: the 0..=3 relevance score
/// and the minimum reported confidence. Low confidence is not an error; a missing
/// or out-of-range score is.
pub fn relevance_assessment(body: &Value) -> crate::contract::Result<Assessment> {
    let a = answers(body);
    let score = a.pointer("/relevance/score").and_then(Value::as_f64);
    let confs = ["/answers/confidence", "/relevance/confidence", "/cite/confidence"];
    let confidence = confs.iter().filter_map(|p| a.pointer(p).and_then(Value::as_f64)).reduce(f64::min);
    let Some(score) = score else {
        return Err(unavailable("relevance score missing"));
    };
    if !(0.0..=3.0).contains(&score) {
        return Err(unavailable(format!("relevance {score} outside 0..=3")));
    }
    Ok(Assessment { score: score.round() as u8, confidence })
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

    fn cand(id: &str) -> Candidate {
        Candidate {
            id: crate::contract::NodeId(id.into()),
            title: format!("t{id}"),
            lead: "l".into(),
            child_titles: vec![],
            page_start: 1,
            page_end: 2,
            level: 1,
        }
    }

    fn judge(rt: &tokio::runtime::Runtime, replies: Vec<Value>) -> JevJudge {
        JevJudge::new(SystemOne::fake(replies), rt.handle().clone())
    }

    #[test]
    fn walk_judge_assesses_on_score_only() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let c = cand("0001");
        let a = judge(&rt, vec![rr(0.9, 2.0, 0.9)]).assess("q", &c).unwrap();
        assert_eq!((a.score, a.confidence), (2, Some(0.9)));
        let low = json!({"answers":{"answers":{"noul":0.9,"confidence":0.8},"relevance":{"score":3,"confidence":0.2}}});
        let a = judge(&rt, vec![low]).assess("q", &c).unwrap();
        assert_eq!((a.score, a.confidence), (3, Some(0.2)), "low confidence is recorded, not gated");
        let j = judge(&rt, vec![json!({"relevance":{"confidence":0.9}})]);
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable", "missing score fails closed");
        let j = judge(&rt, vec![rr(0.9, 4.0, 0.9)]);
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable", "1..=4 scale fails closed");
        let none = SystemOne {
            transport: Transport::None { reason: "k".into() },
            floor: 0.6,
            timeout: Duration::from_secs(1),
        };
        let j = JevJudge::new(none, rt.handle().clone());
        assert_eq!(j.score("q", &c).unwrap_err().code(), "judge_unavailable");
        assert_eq!(j.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn score_batch_is_one_call_and_leaves_missing_ids_to_the_walk() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let cs = [cand("0001"), cand("0002"), cand("0003")];
        let reply = json!({"answers":[
            {"id":"0001","score":1,"confidence":0.8},
            {"id":"0003","score":3,"confidence":0.5}
        ]});
        let j = judge(&rt, vec![reply]);
        assert_eq!(j.batch_cost(&cs), 1);
        let got = j.score_batch("q", &cs).unwrap();
        let got: Vec<Option<(u8, Option<f64>)>> =
            got.iter().map(|a| a.map(|a| (a.score, a.confidence))).collect();
        assert_eq!(got, vec![Some((1, Some(0.8))), None, Some((3, Some(0.5)))]);
        assert_eq!(j.calls.load(Ordering::Relaxed), 1, "a partial reply is one call; the walk fills 0002");

        let full = json!([
            {"id":"0001","score":0,"confidence":0.9},
            {"id":"0002","score":0,"confidence":0.9},
            {"id":"0003","score":2,"confidence":0.9}
        ]);
        let j = judge(&rt, vec![full]);
        assert_eq!(j.score_batch("q", &cs).unwrap()[2].map(|a| a.score), Some(2));
        assert_eq!(j.calls.load(Ordering::Relaxed), 1);

        let j = judge(&rt, vec![json!({"relevance":{"score":3,"confidence":0.9}})]);
        assert_eq!(j.score_batch("q", &cs).unwrap_err().code(), "parse", "a single-hit body is not a batch");
    }

    #[test]
    fn batch_request_matches_lapis() {
        let mut parent = cand("0001");
        parent.child_titles =
            vec![tome_tree::ChildTitle { title: "Section A".into(), page_start: 2, page_end: 2 }];
        let cs = [parent.clone(), cand("0002")];
        let s = batch::state("where is it", &cs);
        assert!(s.starts_with("Search query: where is it\n\n"));
        assert!(s.contains("id: 0002\ntitle: t0002\npages: 1-2\n"));
        assert!(s.contains("child titles:\n- Section A (pp. 2–2)\nlead:\nl\n"), "{s}");
        let one = candidate_state("q", &parent);
        assert!(one.contains("\nChild titles:\n- Section A (pp. 2–2)\n\nLead:\nl"), "{one}");
        let q = batch::questions(&cs);
        assert_eq!(q.as_object().unwrap().keys().collect::<Vec<_>>(), vec!["0001", "0002"]);
        assert_eq!(q["0001"]["type"], "score");
    }
}
