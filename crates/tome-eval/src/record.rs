//! Records that serialise to `evals/tome/schema/tome-eval-result.schema.json`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "0.1.0";

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
    pub git_sha: String,
    pub started_at: String,
    pub finished_at: String,
}
