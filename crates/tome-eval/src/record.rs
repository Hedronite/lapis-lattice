//! Records that serialise to `evals/tome/schema/tome-eval-result.schema.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contract::{Judged, RootPath, Walk};

pub const SCHEMA_VERSION: &str = "0.3.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    Baseline,
    Tome,
}

impl Arm {
    pub fn as_str(self) -> &'static str {
        match self {
            Arm::Baseline => "baseline",
            Arm::Tome => "tome",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Correctness {
    Exact,
    Partial,
    Wrong,
    Error,
}

impl Correctness {
    /// Marci plan scale: wrong 0 / partial 1 / right 2 (errors count 0).
    pub fn score(self) -> u8 {
        match self {
            Correctness::Exact => 2,
            Correctness::Partial => 1,
            Correctness::Wrong | Correctness::Error => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grade {
    /// graded | unavailable | uncertain | not_applicable
    pub status: String,
    pub grader: String,
    pub score: Option<u8>,
    pub confidence: Option<f64>,
    pub blind_label: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMetrics {
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub hit: bool,
    pub precision_tol1: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Latency {
    pub retrieve: f64,
    pub answer: f64,
    pub grade: Option<f64>,
    pub total: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub judge_calls: Option<u32>,
    pub judge_prompt_tokens: Option<u64>,
    pub estimated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opened {
    /// `chunks` (baseline) | `nodes` (tome)
    pub kind: String,
    pub count: u32,
    pub ids: Vec<String>,
    pub pages: Vec<u32>,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub provider: String,
    pub agent: Option<String>,
    pub temperature: f64,
    pub max_output_tokens: u32,
    pub context_budget_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchSettings {
    pub mode: String,
    pub top_k: u32,
    pub retrieve_limit: u32,
    pub per_doc: bool,
    pub doc_filter: String,
    /// `http` (lattice `/search`) | `lapis` (`lapis --json search`).
    #[serde(default)]
    pub transport: Option<String>,
    /// Lattice per-modality candidate pool (http only).
    #[serde(default)]
    pub retrieve_k: Option<u32>,
    /// Lattice `domain` filter applied for this question's PDF.
    #[serde(default)]
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankSettings {
    pub enabled: bool,
    pub transport: String,
    pub status: Option<String>,
    pub confidence_floor: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineSettings {
    pub indexer: String,
    pub indexer_git_commit: Option<String>,
    pub indexer_sha256: String,
    pub chunk_target_chars: u32,
    pub chunk_max_chars: u32,
    pub chunk_overlap_chars: u32,
    pub embedding_model: String,
    pub embedding_dim: Option<u32>,
    pub lattice_url: String,
    pub lattice_db_fingerprint: Option<String>,
    pub search: SearchSettings,
    pub rerank_jev: RerankSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TomeSettings {
    pub backend: String,
    pub builder_version: Option<String>,
    pub beam: u8,
    pub max_judge_calls: u32,
    pub max_open_pages: u32,
    pub max_open_bytes: u32,
    pub judge_transport: Option<String>,
    pub walk_cache: bool,
    /// `<vault>/.lapis/tomes` as configured (vault-relative when relative).
    #[serde(default)]
    pub index_dir: Option<String>,
    /// `DocMeta.summary_model` of the walked doc (`{provider}/{model}`); null when unread.
    #[serde(default)]
    pub summary_model: Option<String>,
    /// `DocMeta.summary_temperature` of the walked doc.
    #[serde(default)]
    pub summary_temperature: Option<f64>,
    /// `[tome] root_calls` passed as `Budget::root_calls` (0.3.0).
    #[serde(default)]
    pub root_calls: Option<u32>,
    /// `[tome] root_batch_size` passed as `Budget::root_batch_size` (0.3.0).
    #[serde(default)]
    pub root_batch_size: Option<u32>,
    /// `[tome] root_top_k` passed as `Budget::root_top_k` (0.3.0).
    #[serde(default)]
    pub root_top_k: Option<u32>,
}

/// One judged tree node, from `Walk::judged`: what the walk ranked on (score) and
/// what it no longer gates on (confidence).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateScore {
    pub node_id: String,
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    /// 0..=3 score the walk ranked on. Always set from 0.3.0 (`Walk::judged`);
    /// `None` only in 0.2.0 records.
    pub score: Option<u8>,
    pub confidence: Option<f64>,
}

impl From<&Judged> for CandidateScore {
    fn from(j: &Judged) -> Self {
        Self {
            node_id: j.node_id.0.clone(),
            title: j.title.clone(),
            page_start: j.page_start,
            page_end: j.page_end,
            score: Some(j.score),
            confidence: j.confidence,
        }
    }
}

impl CandidateScore {
    /// The pre-ruling judge failed closed on a confidence below the floor.
    pub fn below(&self, floor: f64) -> bool {
        self.confidence.is_some_and(|c| c < floor)
    }
}

/// Tome-arm walk judging (schema 0.2.0; root provenance 0.3.0). `null` on baseline
/// records and on walks that returned an error (no `Walk`, so nothing was judged
/// on record).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalkScores {
    /// `score_only`: rank on score, no confidence gate (spike policy).
    pub policy: String,
    /// Reporting floor (0.6); never tuned to eval data.
    pub confidence_floor: f64,
    /// Some judged candidate had confidence < floor, so the pre-ruling judge
    /// would have failed this walk closed (`judge_unavailable`).
    pub would_fail_closed: bool,
    pub candidates: Vec<CandidateScore>,
    /// `batch` | `lexical_fallback`: which root-pass path ranked the roots (0.3.0).
    #[serde(default)]
    pub root_path: Option<String>,
    /// Calls the root pass spent (`Walk::root_judge_calls`, 0.3.0).
    #[serde(default)]
    pub root_judge_calls: Option<u32>,
}

impl WalkScores {
    pub fn score_only(candidates: Vec<CandidateScore>, floor: f64) -> Self {
        Self {
            policy: "score_only".into(),
            confidence_floor: floor,
            would_fail_closed: candidates.iter().any(|c| c.below(floor)),
            candidates,
            root_path: None,
            root_judge_calls: None,
        }
    }

    /// Everything from the walk itself: `judged` (every scored node, in walk
    /// order), `root_path` and `root_judge_calls`.
    pub fn from_walk(walk: &Walk, floor: f64) -> Self {
        let root_path = match walk.root_path {
            RootPath::Batch => "batch",
            RootPath::LexicalFallback => "lexical_fallback",
        };
        Self {
            root_path: Some(root_path.into()),
            root_judge_calls: Some(walk.root_judge_calls),
            ..Self::score_only(walk.judged.iter().map(CandidateScore::from).collect(), floor)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRecord {
    pub record: String,
    pub schema_version: String,
    pub run_id: String,
    pub question_id: String,
    pub doc: String,
    pub doc_sha256: String,
    pub arm: Arm,
    pub scored: bool,
    pub answer: Option<String>,
    pub correctness: Option<Correctness>,
    pub grade: Grade,
    pub cited_pages: Vec<u32>,
    pub gold_pages: Vec<u32>,
    pub page_metrics: PageMetrics,
    pub latency_ms: Latency,
    pub tokens: Tokens,
    pub opened: Opened,
    pub error: Option<ErrorInfo>,
    pub model: ModelInfo,
    pub baseline: BaselineSettings,
    pub tome: TomeSettings,
    /// Per-candidate walk scores (0.2.0) and root provenance (0.3.0); `null` for
    /// baseline and for failed walks.
    #[serde(default)]
    pub walk_scores: Option<WalkScores>,
    pub git_sha: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CorrectnessCounts {
    pub exact: u32,
    pub partial: u32,
    pub wrong: u32,
    pub error: u32,
    pub ungraded: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocSummary {
    pub n: u32,
    pub exact_or_partial: u32,
    pub mean_correctness: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArmSummary {
    pub n: u32,
    pub n_errors: u32,
    pub errors_by_kind: BTreeMap<String, u32>,
    pub silent_empties: u32,
    pub n_graded: u32,
    pub mean_correctness: Option<f64>,
    pub correctness_counts: CorrectnessCounts,
    pub citation_hit_rate: Option<f64>,
    pub mean_page_precision: Option<f64>,
    pub mean_page_recall: Option<f64>,
    pub latency_p50_ms: Option<f64>,
    pub latency_p95_ms: Option<f64>,
    pub mean_prompt_tokens: Option<f64>,
    pub mean_completion_tokens: Option<f64>,
    pub mean_opened: Option<f64>,
    pub per_doc: BTreeMap<String, DocSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arms {
    pub baseline: ArmSummary,
    pub tome: ArmSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerQuestion {
    pub question_id: String,
    pub baseline: Option<Correctness>,
    pub tome: Option<Correctness>,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub pass: Option<bool>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    /// keep | shelve | incomplete
    pub decision: String,
    pub wins: u32,
    pub losses: u32,
    pub ties: u32,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryRecord {
    pub record: String,
    pub schema_version: String,
    pub run_id: String,
    pub questions_file: String,
    pub questions_sha256: String,
    pub n_questions: u32,
    pub smoke: bool,
    pub arms: Arms,
    pub per_question: Vec<PerQuestion>,
    pub verdict: Verdict,
    pub model: ModelInfo,
    pub baseline: BaselineSettings,
    pub tome: TomeSettings,
    /// Tome walks (scored questions) that judged at least one candidate (0.2.0).
    #[serde(default)]
    pub walks_judged: u32,
    /// Of those, walks that would have failed closed at a 0.6 confidence floor.
    #[serde(default)]
    pub would_fail_closed_at_0_6: u32,
    pub git_sha: String,
    pub started_at: String,
    pub finished_at: String,
}
