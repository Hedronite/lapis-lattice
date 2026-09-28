//! `evals/tome/config.toml`: the answer model, budgets and baseline provenance.
//! The model is never hard-coded; swap it here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvalConfig {
    pub answer: AnswerCfg,
    pub budget: BudgetCfg,
    pub baseline: BaselineCfg,
    pub tome: TomeCfg,
    pub jev: JevCfg,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnswerCfg {
    /// `opencode` (subprocess) or `fake` (tests / dry runs).
    pub provider: String,
    /// Passed verbatim to `opencode run --model`, e.g. `opencode-go/deepseek-v4.1-flash`.
    pub model: String,
    /// OpenCode agent written by tome-eval into a scratch project dir; pins temperature, no tools.
    #[serde(default = "default_agent")]
    pub agent: String,
    pub temperature: f64,
    pub max_output_tokens: u32,
    #[serde(default = "default_opencode")]
    pub opencode_bin: String,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BudgetCfg {
    /// Same passage budget for both arms (tokens ≈ chars / 4).
    pub context_budget_tokens: u32,
}

impl BudgetCfg {
    pub fn context_budget_chars(&self) -> usize {
        self.context_budget_tokens as usize * 4
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BaselineCfg {
    /// `http`: GET `<lattice_url>/search` (the lattice service itself; exposes
    /// `retrieve_k` and `domain`). `lapis`: `lapis --json search` (no retrieve_k).
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Per-modality candidate pool for `transport = "http"` (lattice allows 10..=200).
    #[serde(default = "default_retrieve_k")]
    pub retrieve_k: u32,
    /// Vault-relative PDF path → lattice `domain` filter (narrows the vault-wide search).
    #[serde(default)]
    pub domain: std::collections::BTreeMap<String, String>,
    pub lapis_bin: String,
    pub lattice_url: String,
    /// `hybrid` | `bm25` | `vector`.
    pub mode: String,
    /// Hits kept for the answerer after the same-PDF filter.
    pub top_k: u32,
    /// Hits pulled from `lapis search` before the same-PDF filter (lattice caps at 50).
    pub retrieve_limit: u32,
    pub rerank_jev: bool,
    /// chunk_id → pages join map exported read-only from lattice.db.
    pub chunk_map: PathBuf,
    pub indexer: IndexerCfg,
}

fn default_transport() -> String {
    "http".into()
}

fn default_retrieve_k() -> u32 {
    200
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexerCfg {
    pub path: String,
    pub git_commit: Option<String>,
    pub sha256: String,
    pub chunk_target_chars: u32,
    pub chunk_max_chars: u32,
    pub chunk_overlap_chars: u32,
    pub embedding_model: String,
    pub embedding_dim: Option<u32>,
    pub lattice_db_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TomeCfg {
    /// `<vault>/.lapis/tomes`.
    pub index_dir: PathBuf,
    pub beam: u8,
    pub max_judge_calls: u32,
    pub max_open_pages: u32,
    pub max_open_bytes: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JevCfg {
    /// `http` (System One, `$TYPESAFE_API_KEY`) or `none`.
    pub transport: String,
    pub endpoint: String,
    /// Name of the env var holding the key. The key itself is never in config.
    pub key_env: String,
    pub confidence_floor: f64,
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
}

fn default_agent() -> String {
    "tome-answerer".into()
}
fn default_opencode() -> String {
    "opencode".into()
}
fn default_timeout() -> u64 {
    120
}

impl EvalConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut cfg: EvalConfig = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // Relative paths in the file are relative to the file.
        let base = path.parent().unwrap_or(Path::new("."));
        if cfg.baseline.chunk_map.is_relative() {
            cfg.baseline.chunk_map = base.join(&cfg.baseline.chunk_map);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.answer.model.trim().is_empty() {
            return Err("answer.model is empty".into());
        }
        if !(0.0..=2.0).contains(&self.answer.temperature) {
            return Err(format!("answer.temperature out of range: {}", self.answer.temperature));
        }
        if !matches!(self.baseline.mode.as_str(), "hybrid" | "bm25" | "vector") {
            return Err(format!("baseline.mode must be hybrid|bm25|vector, got {}", self.baseline.mode));
        }
        if self.baseline.retrieve_limit == 0 || self.baseline.retrieve_limit > 50 {
            return Err("baseline.retrieve_limit must be 1..=50".into());
        }
        if !matches!(self.baseline.transport.as_str(), "http" | "lapis") {
            return Err(format!("baseline.transport must be http|lapis, got {}", self.baseline.transport));
        }
        if !(10..=200).contains(&self.baseline.retrieve_k) {
            return Err("baseline.retrieve_k must be 10..=200".into());
        }
        if self.baseline.top_k == 0 || self.baseline.top_k > self.baseline.retrieve_limit {
            return Err("baseline.top_k must be 1..=retrieve_limit".into());
        }
        if self.tome.max_open_pages > crate::contract::OPEN_PAGE_CAP
            || self.tome.max_open_bytes as usize > crate::contract::OPEN_BYTE_CAP
        {
            return Err("tome open budget exceeds the library cap (12 pages / 48 KB)".into());
        }
        if self.tome.beam as usize != crate::contract::BEAM {
            return Err(format!(
                "tome.beam = {} but tome_tree walks with the fixed beam {}",
                self.tome.beam,
                crate::contract::BEAM
            ));
        }
        if !matches!(self.jev.transport.as_str(), "http" | "none") {
            return Err(format!("jev.transport must be http|none, got {}", self.jev.transport));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_config_parses_and_validates() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tome/config.toml");
        let c = EvalConfig::load(&p).expect("evals/tome/config.toml");
        assert_eq!(c.answer.temperature, 0.0);
        assert!(!c.answer.model.is_empty());
        assert!(c.baseline.chunk_map.ends_with("data/chunk-pages.jsonl"));
        assert_eq!((c.baseline.transport.as_str(), c.baseline.retrieve_k), ("http", 200));
        let qs = crate::questions::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tome/questions.jsonl"),
        )
        .unwrap();
        for q in qs {
            assert!(c.baseline.domain.contains_key(&q.doc), "no [baseline.domain] entry for {}", q.doc);
        }
    }

    #[test]
    fn model_comes_from_config_not_code() {
        let src = include_str!("answer.rs");
        assert!(!src.contains("deepseek"), "answer model must come from config");
    }
}
