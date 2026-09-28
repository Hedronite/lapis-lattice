//! Post-retrieve Jev gate. Facet TypeSafe / System One is the transport;
//! this module is the judge, not a writer and not an index.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Result;
use crate::http::SearchResult;
use lapis_lattice::Hit;

#[cfg(feature = "tome")]
mod batch;
mod facet;

/// Bundled Facet OpenCollection (Hit rerank recipe).
pub const FACET_COLLECTION: &str = include_str!("../docs/examples/typesafe/opencollection.yml");
pub const FACET_SELECTOR: &str = "items/0/items/0";
pub const FACET_ENVIRONMENT: &str = "typesafe";

const DEFAULT_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
const SNIPPET_MAX: usize = 2000;
const CONFIDENCE_FLOOR: f64 = 0.6;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Shipped System One questions (Noul + Score + citation Choice).
pub fn questions() -> Value {
    json!({
        "answers": {
            "type": "noul",
            "instructions": "Does this chunk answer the search query?",
            "criteria": {
                "true": "The chunk contains information that directly answers the query",
                "false": "The chunk is off-topic or does not answer the query"
            }
        },
        "relevance": {
            "type": "score",
            "instructions": "How relevant is this chunk to the query?",
            "criteria": [
                "Unrelated; no useful overlap with the query",
                "Tangentially related; shared terms only",
                "Partially useful; some supporting detail",
                "Directly answers the query"
            ]
        },
        "cite": {
            "type": "choice",
            "instructions": "How does this chunk relate to the query as a citation?",
            "criteria": {
                "supports": "The chunk supports an answer to the query",
                "contradicts": "The chunk contradicts or refutes an answer to the query",
                "unrelated": "The chunk is not a citation for the query"
            }
        }
    })
}

/// Page-level shadow report. Attached to [`SearchResult::jev`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevMeta {
    pub shadow: bool,
    pub transport: String,
    pub status: String,
    pub reranked: bool,
    /// Shadow: a Jev answer is never an authorization.
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl JevMeta {
    fn new(transport: &str, status: &str, reranked: bool, reason: Option<String>) -> Self {
        Self {
            shadow: true,
            transport: transport.to_string(),
            status: status.to_string(),
            reranked,
            approved: false,
            reason,
        }
    }
}

/// Per-hit shadow annotation. Lives on [`Hit::jev`], never in sqlite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevJudgment {
    pub status: String,
    pub shadow: bool,
    pub approved: bool,
    pub retrieve_rank: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow_rank: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answers: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relevance: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cite: Option<String>,
}

/// How to reach System One. Never holds a Lattice row.
pub enum Transport {
    None {
        reason: &'static str,
    },
    Http {
        endpoint: String,
        key: String,
    },
    Facet {
        bin: PathBuf,
    },
    #[cfg(test)]
    Fake(FakeScript),
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::None { reason } => f.debug_struct("None").field("reason", reason).finish(),
            Transport::Http { endpoint, .. } => {
                f.debug_struct("Http").field("endpoint", endpoint).field("key", &"<redacted>").finish()
            }
            Transport::Facet { bin } => f.debug_struct("Facet").field("bin", bin).finish(),
            #[cfg(test)]
            Transport::Fake(_) => write!(f, "Fake"),
        }
    }
}

/// Canned System One bodies, front to back. Offline tests only.
#[cfg(test)]
#[derive(Clone)]
pub struct FakeScript {
    replies: std::sync::Arc<std::sync::Mutex<Vec<std::result::Result<Value, String>>>>,
    /// `(state, questions)` of each decide, in order. Tests check the batch prompt.
    seen: std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>>,
}

#[cfg(test)]
impl FakeScript {
    pub fn replies(replies: Vec<Value>) -> Self {
        Self {
            replies: std::sync::Arc::new(std::sync::Mutex::new(replies.into_iter().map(Ok).collect())),
            seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }
}

impl Transport {
    /// Facet binary if present, else `$TYPESAFE_API_KEY`, else none. No network.
    pub fn resolve() -> Self {
        match std::env::var("LAPIS_JEV_TRANSPORT") {
            Ok(v) if v.eq_ignore_ascii_case("none") => {
                return Transport::None { reason: "LAPIS_JEV_TRANSPORT=none" };
            }
            Ok(v) if v.eq_ignore_ascii_case("facet") => {
                return match find_on_path("facet") {
                    Some(bin) => Transport::Facet { bin },
                    None => Transport::None { reason: "facet_not_on_path" },
                };
            }
            Ok(v) if v.eq_ignore_ascii_case("http") => return http_from_env(),
            _ => {}
        }
        if let Some(bin) = find_on_path("facet") {
            return Transport::Facet { bin };
        }
        http_from_env()
    }

    /// Why this transport cannot judge, when it is [`Transport::None`].
    #[cfg(feature = "tome")]
    pub fn missing_reason(&self) -> Option<&'static str> {
        match self {
            Transport::None { reason } => Some(*reason),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Transport::None { .. } => "none",
            Transport::Http { .. } => "http",
            Transport::Facet { .. } => "facet",
            #[cfg(test)]
            Transport::Fake(_) => "fake",
        }
    }

    async fn decide(&self, state: &str) -> std::result::Result<Value, String> {
        // Hit rerank keeps the bundled recipe and `--var state`.
        self.decide_with(state, &questions(), false).await
    }

    /// `questions` selects the answer schema. Hit rerank sends [`questions`]
    /// with `inline` false. Every tome judge call passes `inline` so the
    /// request body is the escaped JSON, including a one-node call.
    async fn decide_with(
        &self,
        state: &str,
        questions: &Value,
        inline: bool,
    ) -> std::result::Result<Value, String> {
        match self {
            Transport::None { reason } => Err((*reason).into()),
            #[cfg(test)]
            Transport::Fake(script) => {
                if let Ok(mut seen) = script.seen.lock() {
                    seen.push((state.to_string(), questions.clone()));
                }
                let mut q = script.replies.lock().map_err(|_| "fake lock".to_string())?;
                match q.first() {
                    None => Err("fake exhausted".into()),
                    Some(_) => q.remove(0),
                }
            }
            Transport::Http { endpoint, key } => http_decide(endpoint, key, state, questions).await,
            Transport::Facet { bin } => facet_decide(bin, state, questions, inline).await,
        }
    }
}

fn http_from_env() -> Transport {
    match std::env::var("TYPESAFE_API_KEY") {
        Ok(key) if !key.trim().is_empty() => Transport::Http {
            endpoint: std::env::var("LAPIS_JEV_ENDPOINT")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.into()),
            key,
        },
        _ => Transport::None { reason: "typesafe_key_absent" },
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Bound query↔chunk state. Path stays attached; snippet is clipped.
pub fn hit_state(query: &str, hit: &Hit) -> String {
    let snippet = clip(hit.snippet.as_deref().unwrap_or(""), SNIPPET_MAX);
    let heading = hit.heading.as_deref().unwrap_or("");
    format!(
        "Search query: {query}\n\nNote: {}\nTitle: {}\nHeading: {heading}\n\nChunk:\n{snippet}",
        hit.path, hit.title
    )
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn systemone_body(state: &str, questions: &Value) -> Value {
    json!({
        "model": "jev-latest",
        "state": state,
        "questions": questions,
    })
}

async fn http_decide(
    endpoint: &str,
    key: &str,
    state: &str,
    questions: &Value,
) -> std::result::Result<Value, String> {
    let client = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build().map_err(|e| e.to_string())?;
    let resp = client
        .post(endpoint)
        .header("Authorization", format!("Bearer {key}"))
        .header("Accept", "application/json")
        .json(&systemone_body(state, questions))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("systemone {status}: {}", clip(&text, 200)));
    }
    serde_json::from_str(&text).map_err(|e| format!("systemone json: {e}"))
}

/// Facet YAML for one tome judge call. The body is escaped JSON, so a
/// `{{typesafeApiKey}}` in the query or the page text is not a placeholder.
/// `asked` is the shipped relevance schema for one candidate, or one score
/// question per id for a batch.
fn tome_facet_yaml(state: &str, asked: &Value) -> String {
    facet::collection(&facet::json(&systemone_body(state, asked)))
}

/// `(yaml, escaped)`. `escaped` means the body is inlined and `--var state`
/// must not be passed. Tome calls pass `inline`; hit rerank does not.
fn facet_collection_text(state: &str, asked: &Value, inline: bool) -> (String, bool) {
    let escaped = inline || asked != &questions();
    let text = if escaped { tome_facet_yaml(state, asked) } else { FACET_COLLECTION.to_string() };
    (text, escaped)
}

async fn facet_decide(
    bin: &Path,
    state: &str,
    asked: &Value,
    inline: bool,
) -> std::result::Result<Value, String> {
    let dir = tempfile_dir()?;
    let yaml = dir.join("opencollection.yml");
    let (text, escaped) = facet_collection_text(state, asked, inline);
    std::fs::write(&yaml, text).map_err(|e| e.to_string())?;
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--json")
        .arg("request")
        .arg("run")
        .arg(&yaml)
        .arg(FACET_SELECTOR)
        .arg("--environment")
        .arg(FACET_ENVIRONMENT)
        .arg("--no-record");
    if !escaped {
        cmd.arg("--var").arg(format!("state={state}"));
    }
    let out = cmd.output().await.map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        return Err(format!("facet exit {}: {}", out.status, clip(&format!("{err}{stdout}"), 300)));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("facet json: {e}"))?;
    parse_facet_answers(&v)
}

fn tempfile_dir() -> std::result::Result<PathBuf, String> {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let dir = std::env::temp_dir().join(format!("lapis-jev-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn parse_facet_answers(v: &Value) -> std::result::Result<Value, String> {
    let content = v.pointer("/response/body/content").and_then(Value::as_str).ok_or_else(|| {
        if v.get("error").is_some() {
            format!("facet error: {}", v["error"])
        } else {
            "facet response missing body".into()
        }
    })?;
    serde_json::from_str(content).map_err(|e| format!("facet body json: {e}"))
}

/// Judge the returned page. Missing transport ≠ approve; hits stay.
pub async fn apply_rerank(result: &mut SearchResult, transport: Transport) -> Result<()> {
    if let Transport::None { reason } = &transport {
        result.jev =
            Some(serde_json::to_value(JevMeta::new("none", "unavailable", false, Some((*reason).into())))?);
        return Ok(());
    }
    let query = result.query.clone();
    let mut parsed = Vec::with_capacity(result.hits.len());
    for hit in &result.hits {
        let state = hit_state(&query, hit);
        match transport.decide(&state).await {
            Ok(body) => parsed.push(judgment_from_answers(hit.rank, &body)),
            Err(_) => parsed.push(uncertain(hit.rank)),
        }
    }
    for (hit, j) in result.hits.iter_mut().zip(parsed.iter()) {
        hit.jev = Some(serde_json::to_value(j)?);
    }
    let all_judged = !parsed.is_empty() && parsed.iter().all(|j| j.status == "judged");
    let any_judged = parsed.iter().any(|j| j.status == "judged");
    let status = if all_judged {
        "judged"
    } else if any_judged {
        "uncertain"
    } else {
        "unavailable"
    };
    let reason = if status == "unavailable" { Some("empty_or_transport".into()) } else { None };
    result.jev = Some(serde_json::to_value(JevMeta::new(transport.name(), status, false, reason))?);
    apply_shadow_order(result);
    Ok(())
}

fn apply_shadow_order(result: &mut SearchResult) {
    let mut judged = Vec::new();
    for (i, hit) in result.hits.iter().enumerate() {
        let Some(j) = hit.jev.as_ref().and_then(|v| serde_json::from_value::<JevJudgment>(v.clone()).ok())
        else {
            return;
        };
        if j.status != "judged" {
            return;
        }
        judged.push((i, score_of(&j)));
    }
    if judged.is_empty() {
        return;
    }
    judged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let changed = judged.iter().enumerate().any(|(pos, (i, _))| pos != *i);
    if !changed {
        if let Some(meta) = result.jev.as_mut().and_then(Value::as_object_mut) {
            meta.insert("reranked".into(), json!(false));
            meta.insert("status".into(), json!("judged"));
        }
        return;
    }
    let mut next = Vec::with_capacity(result.hits.len());
    let old = std::mem::take(&mut result.hits);
    for (pos, (i, _)) in judged.iter().enumerate() {
        let mut hit = old[*i].clone();
        if let Some(v) = hit.jev.as_mut().and_then(Value::as_object_mut) {
            v.insert("shadowRank".into(), json!(pos as u32 + 1));
        }
        next.push(hit);
    }
    result.hits = next;
    if let Some(meta) = result.jev.as_mut().and_then(Value::as_object_mut) {
        meta.insert("reranked".into(), json!(true));
        meta.insert("status".into(), json!("judged"));
        meta.insert("approved".into(), json!(false));
        meta.insert("shadow".into(), json!(true));
    }
}

fn score_of(j: &JevJudgment) -> f64 {
    let rel = j.relevance.unwrap_or(0.0);
    let ans = j.answers.unwrap_or(0.0);
    rel + ans
}

fn uncertain(retrieve_rank: u32) -> JevJudgment {
    JevJudgment {
        status: "uncertain".into(),
        shadow: true,
        approved: false,
        retrieve_rank,
        shadow_rank: None,
        answers: None,
        relevance: None,
        confidence: None,
        cite: None,
    }
}

/// Parse a System One `answers` object. Empty / low confidence → uncertain.
pub fn judgment_from_answers(retrieve_rank: u32, body: &Value) -> JevJudgment {
    let answers = body.get("answers").unwrap_or(body);
    let noul = answers.pointer("/answers/noul").and_then(Value::as_f64);
    let noul_conf = answers.pointer("/answers/confidence").and_then(Value::as_f64);
    let score = answers.pointer("/relevance/score").and_then(Value::as_f64);
    let score_conf = answers.pointer("/relevance/confidence").and_then(Value::as_f64);
    let cite = answers.pointer("/cite/choice").and_then(Value::as_str).map(str::to_string);
    let cite_conf = answers.pointer("/cite/confidence").and_then(Value::as_f64);
    let confidence = [noul_conf, score_conf, cite_conf].into_iter().flatten().reduce(f64::min);
    let empty = noul.is_none() && score.is_none() && cite.as_deref().unwrap_or("").is_empty();
    let low = confidence.is_some_and(|c| c < CONFIDENCE_FLOOR);
    if empty || low {
        return uncertain(retrieve_rank);
    }
    JevJudgment {
        status: "judged".into(),
        shadow: true,
        approved: false,
        retrieve_rank,
        shadow_rank: None,
        answers: noul,
        relevance: score,
        confidence,
        cite,
    }
}

/// Human one-liner for a hit's Jev annotation (CLI text mode).
pub fn hit_line(hit: &Hit) -> Option<String> {
    let j = hit.jev.as_ref()?;
    let status = j.get("status").and_then(Value::as_str).unwrap_or("?");
    if status == "judged" {
        let ans = j.get("answers").and_then(Value::as_f64);
        let rel = j.get("relevance").and_then(Value::as_f64);
        let cite = j.get("cite").and_then(Value::as_str).unwrap_or("-");
        Some(format!(
            "jev=shadow answers={} relevance={} cite={cite} (not approved)",
            ans.map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
            rel.map(|n| format!("{n:.2}")).unwrap_or_else(|| "-".into()),
        ))
    } else {
        Some(format!("jev={status} (shadow; not approved)"))
    }
}

/// Scores tome-tree candidates with the existing System One transport.
///
/// Relevance is the shipped 0–3 score. A missing transport, a transport error,
/// a missing score, or a score outside 0..=3 is `judge_unavailable`. Confidence
/// is returned on the assessment and is not a gate: the walk ranks on the score.
///
/// Several candidates are one `decide` call. The state asks for a JSON array
/// of `{id, score, confidence}`, and the questions are one score rubric per
/// candidate id. Ids the array (or the per-id answers) actually scored are
/// `Some`. Missing ids are `None`: the walk judges those one at a time and
/// counts each call. A body with no array and no per-id score is `parse`, and
/// the walk then pre-ranks. A dead transport stays `judge_unavailable`.
///
/// `score` blocks on a multi-thread tokio runtime (`block_in_place` + `block_on`).
/// The `lapis` binary is multi-thread. A current-thread runtime panics.
#[cfg(feature = "tome")]
pub struct JevJudge {
    transport: Transport,
}

#[cfg(feature = "tome")]
impl JevJudge {
    pub fn resolve() -> Self {
        Self { transport: Transport::resolve() }
    }
}

#[cfg(feature = "tome")]
impl tome_tree::Judge for JevJudge {
    fn score(&self, query: &str, candidate: &tome_tree::Candidate) -> tome_tree::Result<u8> {
        Ok(self.assess(query, candidate)?.score)
    }

    fn assess(
        &self,
        query: &str,
        candidate: &tome_tree::Candidate,
    ) -> tome_tree::Result<tome_tree::Assessment> {
        let body = self.decide_state(&candidate_state(query, candidate))?;
        relevance_assessment(&body)
    }

    fn score_batch(
        &self,
        query: &str,
        candidates: &[tome_tree::Candidate],
    ) -> tome_tree::Result<Vec<Option<tome_tree::Assessment>>> {
        if candidates.len() <= 1 {
            return candidates.iter().map(|candidate| self.assess(query, candidate).map(Some)).collect();
        }
        let questions = batch::questions(candidates);
        let body = self.decide_questions(&batch::state(query, candidates), &questions)?;
        // Missing ids stay `None`. The walk judges them and counts each call.
        batch::assessments(&body, candidates)
    }

    fn batch_cost(&self, _candidates: &[tome_tree::Candidate]) -> u32 {
        // The batch request only. Fill-ins are separate `assess` calls.
        1
    }
}

#[cfg(feature = "tome")]
impl JevJudge {
    fn decide_state(&self, state: &str) -> tome_tree::Result<Value> {
        self.decide_questions(state, &questions())
    }

    fn decide_questions(&self, state: &str, questions: &Value) -> tome_tree::Result<Value> {
        if let Some(reason) = self.transport.missing_reason() {
            return Err(tome_tree::TomeError::JudgeUnavailable { reason: reason.to_string() });
        }
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| tome_tree::TomeError::JudgeUnavailable { reason: "no tokio runtime".into() })?;
        match tokio::task::block_in_place(|| {
            handle.block_on(self.transport.decide_with(state, questions, true))
        }) {
            Err(reason) => Err(tome_tree::TomeError::JudgeUnavailable { reason }),
            Ok(body) => Ok(body),
        }
    }
}

#[cfg(feature = "tome")]
fn candidate_state(query: &str, candidate: &tome_tree::Candidate) -> String {
    // The shipped questions ask about a retrieved chunk. A TOC node is not
    // one: say so, and pass the page lead and the child titles as separate
    // fields so a long lead cannot hide the titles.
    let mut out = format!(
        "Search query: {query}\n\n\
         This is a table-of-contents node, not a retrieved passage.\n\n\
         Section: {}\nPages: {}-{}\n",
        candidate.title, candidate.page_start, candidate.page_end
    );
    if !candidate.child_titles.is_empty() {
        out.push_str("\nChild titles:\n");
        for child in &candidate.child_titles {
            out.push_str(&format!("- {} (pp. {}–{})\n", child.title, child.page_start, child.page_end));
        }
    }
    out.push_str(&format!("\nLead:\n{}", candidate.lead));
    out
}

/// Relevance score and the minimum reported confidence. Low confidence is not
/// an error here; a missing or out-of-range score is.
#[cfg(feature = "tome")]
fn relevance_assessment(body: &Value) -> tome_tree::Result<tome_tree::Assessment> {
    let answers = body.get("answers").unwrap_or(body);
    let score = answers.pointer("/relevance/score").and_then(Value::as_f64);
    let noul_conf = answers.pointer("/answers/confidence").and_then(Value::as_f64);
    let score_conf = answers.pointer("/relevance/confidence").and_then(Value::as_f64);
    let cite_conf = answers.pointer("/cite/confidence").and_then(Value::as_f64);
    let confidence = [noul_conf, score_conf, cite_conf].into_iter().flatten().reduce(f64::min);
    let Some(score) = score else {
        return Err(tome_tree::TomeError::JudgeUnavailable { reason: "relevance score missing".into() });
    };
    if !(0.0..=3.0).contains(&score) {
        return Err(tome_tree::TomeError::JudgeUnavailable {
            reason: format!("relevance {score} outside 0..=3"),
        });
    }
    Ok(tome_tree::Assessment { score: score.round() as u8, confidence })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_lattice::Mode;
    use std::sync::Mutex;

    fn hit(path: &str, rank: u32, snippet: &str) -> Hit {
        Hit {
            path: path.into(),
            kind: lapis_lattice::MARKDOWN.into(),
            title: path.into(),
            heading: None,
            snippet: Some(snippet.into()),
            score: 0.1 * rank as f64,
            rank,
            domain: None,
            doc_type: None,
            tags: vec![],
            chunk_id: None,
            chunk_index: None,
            jev: None,
        }
    }

    fn page(hits: Vec<Hit>) -> SearchResult {
        SearchResult {
            query: "welcome".into(),
            mode: Mode::Bm25,
            modalities: vec!["bm25".into()],
            latency_ms: Some(1.0),
            latency: crate::http::Latency::default(),
            count: hits.len(),
            hits,
            jev: None,
        }
    }

    fn judged_body(noul: f64, score: f64, cite: &str, conf: f64) -> Value {
        json!({
            "answers": {
                "answers": { "type": "noul", "noul": noul, "confidence": conf },
                "relevance": { "type": "score", "score": score, "confidence": conf },
                "cite": { "type": "choice", "choice": cite, "confidence": conf }
            }
        })
    }

    #[test]
    fn collection_is_secret_and_shadow_and_has_no_key() {
        assert!(!FACET_COLLECTION.contains("sk-"));
        assert!(!FACET_COLLECTION.contains("Bearer ts_"));
        assert!(
            FACET_COLLECTION.contains("$TYPESAFE_API_KEY"),
            "comments may name the env var; the YAML must not bake a value"
        );
        let body = FACET_COLLECTION.split("data: |-").nth(1).unwrap_or("");
        assert!(!body.contains("TYPESAFE_API_KEY"), "recipe JSON must not mention the key");
        assert!(FACET_COLLECTION.contains("secret: true"));
        assert!(FACET_COLLECTION.contains("typesafeApiKey"));
        assert!(FACET_COLLECTION.contains("jevShadow"));
        assert!(FACET_COLLECTION.contains("\"true\""));
        assert!(FACET_COLLECTION.contains("\"type\": \"noul\""));
        assert!(FACET_COLLECTION.contains("\"type\": \"score\""));
        assert!(FACET_COLLECTION.contains("\"type\": \"choice\""));
        assert!(FACET_COLLECTION.contains("supports"));
        assert!(FACET_COLLECTION.contains("contradicts"));
        assert!(FACET_COLLECTION.contains("unrelated"));
        assert!(!questions().to_string().contains("TYPESAFE"));
    }

    #[test]
    fn empty_or_low_confidence_is_uncertain_not_approve() {
        let empty = judgment_from_answers(1, &json!({}));
        assert_eq!(empty.status, "uncertain");
        assert!(!empty.approved && empty.shadow);
        let low = judgment_from_answers(1, &judged_body(0.9, 3.0, "supports", 0.2));
        assert_eq!(low.status, "uncertain");
        assert!(!low.approved);
        let ok = judgment_from_answers(2, &judged_body(0.8, 3.0, "supports", 0.95));
        assert_eq!(ok.status, "judged");
        assert_eq!(ok.cite.as_deref(), Some("supports"));
        assert_eq!(ok.answers, Some(0.8));
        assert!(!ok.approved);
    }

    #[test]
    fn state_binds_path_and_clips_bytes() {
        let long = "α".repeat(3000);
        let h = hit("notes/Welcome.md", 1, &long);
        let s = hit_state("does welcome exist", &h);
        assert!(s.contains("Search query: does welcome exist"));
        assert!(s.contains("Note: notes/Welcome.md"));
        assert!(s.len() < 3000 + 200, "snippet clipped, got {}", s.len());
        assert!(!s.contains("TYPESAFE"));
    }

    #[tokio::test]
    async fn missing_transport_is_unavailable_and_does_not_touch_hits() {
        let mut r = page(vec![hit("a.md", 1, "hello"), hit("b.md", 2, "there")]);
        apply_rerank(&mut r, Transport::None { reason: "typesafe_key_absent" }).await.unwrap();
        assert!(r.hits.iter().all(|h| h.jev.is_none()), "no approval theater on the hits");
        let meta: JevMeta = serde_json::from_value(r.jev.unwrap()).unwrap();
        assert_eq!(meta.status, "unavailable");
        assert!(!meta.approved && meta.shadow && !meta.reranked);
        assert_eq!(meta.reason.as_deref(), Some("typesafe_key_absent"));
        assert_eq!(r.hits[0].path, "a.md");
    }

    #[tokio::test]
    async fn fake_confident_scores_reorder_but_keep_retrieve_rank() {
        let script = FakeScript::replies(vec![
            judged_body(0.2, 0.0, "unrelated", 0.9),
            judged_body(0.95, 3.0, "supports", 0.99),
        ]);
        let mut r = page(vec![hit("weak.md", 1, "noise"), hit("strong.md", 2, "welcome home")]);
        apply_rerank(&mut r, Transport::Fake(script)).await.unwrap();
        assert_eq!(r.hits[0].path, "strong.md");
        assert_eq!(r.hits[0].rank, 2, "retrieve rank is preserved");
        assert_eq!(r.hits[1].path, "weak.md");
        assert_eq!(r.hits[1].rank, 1);
        let j0: JevJudgment = serde_json::from_value(r.hits[0].jev.clone().unwrap()).unwrap();
        assert_eq!(j0.shadow_rank, Some(1));
        assert_eq!(j0.cite.as_deref(), Some("supports"));
        assert!(!j0.approved && j0.shadow);
        let meta: JevMeta = serde_json::from_value(r.jev.unwrap()).unwrap();
        assert!(meta.reranked && meta.shadow && !meta.approved);
        assert_eq!(meta.transport, "fake");
    }

    #[tokio::test]
    async fn fake_uncertain_does_not_reorder_or_approve() {
        let script = FakeScript::replies(vec![judged_body(0.9, 3.0, "supports", 0.99), json!({})]);
        let mut r = page(vec![hit("a.md", 1, "a"), hit("b.md", 2, "b")]);
        apply_rerank(&mut r, Transport::Fake(script)).await.unwrap();
        assert_eq!(r.hits[0].path, "a.md");
        assert_eq!(r.hits[1].path, "b.md");
        let meta: JevMeta = serde_json::from_value(r.jev.unwrap()).unwrap();
        assert_eq!(meta.status, "uncertain");
        assert!(!meta.reranked && !meta.approved);
        let j1: JevJudgment = serde_json::from_value(r.hits[1].jev.clone().unwrap()).unwrap();
        assert_eq!(j1.status, "uncertain");
    }

    #[test]
    fn resolve_without_key_or_facet_is_none() {
        let _lock = ENV.lock().unwrap();
        let old_path = std::env::var_os("PATH");
        let old_key = std::env::var_os("TYPESAFE_API_KEY");
        let old_force = std::env::var_os("LAPIS_JEV_TRANSPORT");
        unsafe {
            std::env::set_var("PATH", "/nonexistent-lapis-jev-path");
            std::env::remove_var("TYPESAFE_API_KEY");
            std::env::remove_var("LAPIS_JEV_TRANSPORT");
        }
        let t = Transport::resolve();
        match old_path {
            Some(v) => unsafe { std::env::set_var("PATH", v) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        match old_key {
            Some(v) => unsafe { std::env::set_var("TYPESAFE_API_KEY", v) },
            None => unsafe { std::env::remove_var("TYPESAFE_API_KEY") },
        }
        match old_force {
            Some(v) => unsafe { std::env::set_var("LAPIS_JEV_TRANSPORT", v) },
            None => unsafe { std::env::remove_var("LAPIS_JEV_TRANSPORT") },
        }
        assert!(matches!(t, Transport::None { reason } if reason == "typesafe_key_absent"));
    }

    #[test]
    fn http_debug_redacts_the_key() {
        let t = Transport::Http { endpoint: DEFAULT_ENDPOINT.into(), key: "sk-secret-live".into() };
        let s = format!("{t:?}");
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("sk-secret-live"));
    }

    #[test]
    fn facet_response_body_is_unwrapped() {
        let inner = judged_body(1.0, 3.0, "supports", 1.0);
        let wrapped = json!({
            "response": { "body": { "content": inner.to_string() } }
        });
        let answers = parse_facet_answers(&wrapped).unwrap();
        let j = judgment_from_answers(1, &answers);
        assert_eq!(j.status, "judged");
        assert_eq!(j.cite.as_deref(), Some("supports"));
    }

    static ENV: Mutex<()> = Mutex::new(());

    #[cfg(feature = "tome")]
    #[tokio::test(flavor = "multi_thread")]
    async fn tome_judge_maps_relevance_and_refuses_uncertain() {
        let candidate = tome_tree::Candidate {
            id: tome_tree::NodeId::from("0001"),
            title: "Chapter".into(),
            lead: "lead text".into(),
            child_titles: vec![],
            page_start: 1,
            page_end: 2,
            level: 1,
        };
        let judge = JevJudge {
            transport: Transport::Fake(FakeScript::replies(vec![judged_body(0.9, 3.0, "supports", 0.99)])),
        };
        assert_eq!(tome_tree::Judge::score(&judge, "where", &candidate).unwrap(), 3);

        let judge = JevJudge { transport: Transport::Fake(FakeScript::replies(vec![json!({})])) };
        let err = tome_tree::Judge::score(&judge, "where", &candidate).unwrap_err();
        assert_eq!(err.code(), "judge_unavailable");

        let judge = JevJudge { transport: Transport::None { reason: "typesafe_key_absent" } };
        let err = tome_tree::Judge::score(&judge, "where", &candidate).unwrap_err();
        assert!(err.to_string().contains("typesafe_key_absent"));

        let low = JevJudge {
            transport: Transport::Fake(FakeScript::replies(vec![judged_body(0.4, 1.4, "unrelated", 0.35)])),
        };
        let assessment = tome_tree::Judge::assess(&low, "where", &candidate).unwrap();
        assert_eq!(assessment.score, 1);
        assert_eq!(assessment.confidence, Some(0.35));
    }

    #[cfg(feature = "tome")]
    #[tokio::test(flavor = "multi_thread")]
    async fn tome_batch_parses_a_json_array_and_keeps_partial_ids() {
        let mut with_child = candidate_of("0001");
        with_child.lead = "alpha ".repeat(80);
        with_child.child_titles =
            vec![tome_tree::ChildTitle { title: "Section A".into(), page_start: 2, page_end: 2 }];
        let candidates = [with_child, candidate_of("0002")];

        let realistic = json!({
            "model": "jev-1.13.0",
            "answers": [
                { "id": "0001", "score": 3, "confidence": 0.82 },
                { "id": "0002", "score": 1, "confidence": 0.44 }
            ],
            "usage": { "input_tokens": 512, "output_tokens": 40 }
        });
        let parsed = batch::assessments(&realistic, &candidates).unwrap();
        assert_eq!(parsed[0].unwrap().score, 3);
        assert_eq!(parsed[0].unwrap().confidence, Some(0.82));
        assert_eq!(parsed[1].unwrap().score, 1);
        let loose = json!([{ "id": "0001", "score": 2 }]);
        assert!(batch::assessments(&loose, &candidates).is_err(), "a row needs id, score, and confidence");
        let wide = json!([{ "id": "0001", "score": 9, "confidence": 0.5 }]);
        assert!(batch::assessments(&wide, &candidates).is_err(), "a score outside 0..=3 is not a batch row");

        let script = FakeScript::replies(vec![realistic]);
        let batched = JevJudge { transport: Transport::Fake(script.clone()) };
        assert_eq!(tome_tree::Judge::batch_cost(&batched, &candidates), 1);
        let got = tome_tree::Judge::score_batch(&batched, "where", &candidates).unwrap();
        assert_eq!(got[0].unwrap().score, 3);
        assert_eq!(got[0].unwrap().confidence, Some(0.82));
        assert_eq!(got[1].unwrap().score, 1);
        let (state, asked) = script.seen.lock().unwrap()[0].clone();
        assert!(state.contains("JSON array"), "{state}");
        assert!(state.contains("\"id\""), "{state}");
        assert!(state.contains("child titles:\n- Section A"), "child titles are their own field: {state}");
        let page = state.split("lead:\n").nth(1).unwrap().split("\n\n").next().unwrap();
        assert_eq!(page.chars().count(), 240, "the page lead is capped on its own: {page}");
        assert!(!page.contains("Section A"), "{page}");
        assert_eq!(asked["0001"]["type"].as_str(), Some("score"));
        assert_eq!(asked["0002"]["type"].as_str(), Some("score"));
        assert!(asked.get("relevance").is_none(), "a batch must not send the single relevance question");

        let partial_script =
            FakeScript::replies(vec![json!([{ "id": "0001", "score": 2, "confidence": 0.7 }])]);
        let partial = JevJudge { transport: Transport::Fake(partial_script.clone()) };
        let got = tome_tree::Judge::score_batch(&partial, "where", &candidates).unwrap();
        assert_eq!(got[0].unwrap().score, 2, "the id that came back is kept");
        assert_eq!(got[0].unwrap().confidence, Some(0.7));
        assert!(got[1].is_none(), "the missing id is not filled in inside the batch call");
        assert_eq!(partial_script.seen.lock().unwrap().len(), 1, "a partial reply is one model call");

        let from_scores = json!({
            "model": "jev-1.13.0",
            "answers": {
                "0001": { "type": "score", "score": 2.2, "confidence": 0.6 },
                "0002": { "type": "score", "score": 0.4, "confidence": 0.3 }
            }
        });
        let mapped = batch::assessments(&from_scores, &candidates).unwrap();
        assert_eq!(mapped[0].unwrap().score, 2);
        assert_eq!(mapped[1].unwrap().score, 0);

        let single = JevJudge {
            transport: Transport::Fake(FakeScript::replies(vec![judged_body(0.9, 3.0, "supports", 0.99)])),
        };
        let err = tome_tree::Judge::score_batch(&single, "where", &candidates).unwrap_err();
        assert_eq!(err.code(), "parse", "one relevance score is not a batch");

        let dead = JevJudge { transport: Transport::None { reason: "typesafe_key_absent" } };
        let err = tome_tree::Judge::score_batch(&dead, "where", &candidates).unwrap_err();
        assert_eq!(err.code(), "judge_unavailable");
    }

    #[cfg(feature = "tome")]
    fn candidate_of(id: &str) -> tome_tree::Candidate {
        tome_tree::Candidate {
            id: tome_tree::NodeId::from(id),
            title: "Chapter".into(),
            lead: "lead text".into(),
            child_titles: vec![],
            page_start: 1,
            page_end: 2,
            level: 1,
        }
    }

    #[test]
    fn facet_batch_keeps_a_placeholder_in_pdf_text_literal() {
        let state = "page text mentions {{typesafeApiKey}} and should stay literal";
        let questions = json!({
            "0001": {
                "type": "score",
                "instructions": "section title {{typesafeApiKey}}",
                "criteria": ["a", "b", "c", "d"]
            }
        });
        let body = facet::json(&systemone_body(state, &questions));
        assert!(!body.contains("{{"), "template braces must not survive into the JSON source: {body}");
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["state"].as_str().unwrap(), state);
        assert!(parsed["questions"]["0001"]["instructions"].as_str().unwrap().contains("{{typesafeApiKey}}"));
        let leaked = body.replace("{{typesafeApiKey}}", "SECRET");
        let parsed: Value = serde_json::from_str(&leaked).unwrap();
        assert!(parsed["state"].as_str().unwrap().contains("{{typesafeApiKey}}"));
        let yaml = facet::collection(&body);
        let json_line = yaml.lines().find(|line| line.contains("\\u007b")).expect("escaped body");
        assert!(!json_line.contains("{{typesafeApiKey}}"), "{json_line}");
        assert!(yaml.contains("Bearer {{typesafeApiKey}}"), "the header placeholder is the real one");
    }

    #[test]
    fn facet_single_keeps_a_placeholder_in_pdf_text_literal() {
        let state = "Search query: {{typesafeApiKey}}\n\nLead:\npage mentions {{typesafeApiKey}}";
        let (yaml, escaped) = facet_collection_text(state, &questions(), true);
        assert!(escaped, "a single candidate must not use --var state");
        assert!(!yaml.contains("{{state}}"), "the bundled state variable is the old recipe");
        assert!(yaml.contains("\"type\": \"score\"") || yaml.contains("\"type\":\"score\""));
        assert!(yaml.contains("Bearer {{typesafeApiKey}}"), "the header placeholder is the real one");
        let json_line = yaml.lines().find(|line| line.contains("jev-latest")).expect("inlined body");
        assert!(!json_line.contains("{{"), "single-path JSON must not contain a placeholder: {json_line}");
        let parsed: Value = serde_json::from_str(json_line.trim()).unwrap();
        assert_eq!(parsed["state"].as_str().unwrap(), state);
        assert_eq!(parsed["questions"]["relevance"]["type"].as_str(), Some("score"));
        let leaked = json_line.replace("{{typesafeApiKey}}", "SECRET");
        let parsed: Value = serde_json::from_str(leaked.trim()).unwrap();
        assert!(parsed["state"].as_str().unwrap().contains("{{typesafeApiKey}}"));
    }
}
