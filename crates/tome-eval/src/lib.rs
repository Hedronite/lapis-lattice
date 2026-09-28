//! tome-eval: the tome-tree spike A/B harness (Jupi). Baseline = `lapis search`
//! (+ Jev shadow rerank) over the lattice HTTP tome chunks; tome = `walk()` with a
//! Jev judge. One shared answerer from `evals/tome/config.toml`; Jev only scores.
//!
//! Clean-room: concepts from VectifyAI/PageIndex@619cbd8 (MIT); no code copied.

pub mod answer;
pub mod baseline;
pub mod config;
pub mod contract;
pub mod fake;
pub mod jev;
pub mod metrics;
pub mod questions;
pub mod record;
pub mod runner;
pub mod schema_check;
