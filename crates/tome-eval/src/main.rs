//! `tome-eval`: run the tome-tree spike A/B, check the question set, validate results.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use tome_eval::answer::Answerer;
use tome_eval::baseline::ChunkMap;
use tome_eval::config::EvalConfig;
use tome_eval::contract::{StubTome, TomeApi};
use tome_eval::jev::SystemOne;
use tome_eval::record::Arm;
use tome_eval::runner::{Harness, RunOpts};
use tome_eval::{questions, schema_check};

#[derive(Parser)]
#[command(
    name = "tome-eval",
    about = "tome-tree spike A/B harness (baseline lapis search --rerank-jev vs tome walk)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Structural checks on questions.jsonl (+ the fail-closed probe file).
    Check {
        #[arg(long, default_value = "evals/tome/questions.jsonl")]
        questions: PathBuf,
        #[arg(long, default_value = "evals/tome/failclosed.jsonl")]
        failclosed: PathBuf,
    },
    /// Run arms over the question set and write results.jsonl + summary.json.
    Run {
        #[arg(long, default_value = "evals/tome/config.toml")]
        config: PathBuf,
        #[arg(long, default_value = "evals/tome/questions.jsonl")]
        questions: PathBuf,
        /// Non-scored fail-closed probes (tome arm only). Pass an empty string to skip.
        #[arg(long, default_value = "evals/tome/failclosed.jsonl")]
        failclosed: String,
        #[arg(long)]
        out: PathBuf,
        /// Only the first N questions (smoke run; verdict is then `incomplete`).
        #[arg(long)]
        limit: Option<usize>,
        /// Only these question ids.
        #[arg(long = "only")]
        only: Vec<String>,
        /// `baseline,tome` (default both).
        #[arg(long, value_delimiter = ',', default_value = "baseline,tome")]
        arms: Vec<String>,
        /// Override the build-time git sha.
        #[arg(long)]
        git_sha: Option<String>,
    },
    /// Validate results.jsonl / summary.json against the committed schema.
    Validate { files: Vec<PathBuf> },
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("tome-eval: {e}");
            ExitCode::from(1)
        }
    }
}

fn real_main() -> Result<ExitCode, String> {
    match Cli::parse().cmd {
        Cmd::Check { questions: qp, failclosed } => {
            let qs = questions::load(&qp)?;
            let fc = questions::load(&failclosed)?;
            let mut errs = questions::check(&qs);
            errs.extend(questions::check(&fc));
            let docs: std::collections::BTreeSet<_> = qs.iter().map(|q| q.doc.as_str()).collect();
            println!(
                "{} scored questions over {} PDFs; {} fail-closed probe(s)",
                qs.len(),
                docs.len(),
                fc.len()
            );
            for e in &errs {
                println!("ERR {e}");
            }
            Ok(if errs.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Validate { files } => {
            let schema = schema_check::committed_schema();
            let checker = schema_check::Checker::new(&schema);
            let mut bad = 0usize;
            let mut n = 0usize;
            for f in files {
                let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                let docs: Vec<serde_json::Value> = if f.extension().is_some_and(|e| e == "jsonl") {
                    text.lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(serde_json::from_str)
                        .collect::<Result<_, _>>()
                } else {
                    serde_json::from_str(&text).map(|v| vec![v])
                }
                .map_err(|e| format!("{}: {e}", f.display()))?;
                for (i, d) in docs.iter().enumerate() {
                    n += 1;
                    let errs = checker.validate(d);
                    if !errs.is_empty() {
                        bad += 1;
                        println!("{}#{}: {}", f.display(), i + 1, errs.join("; "));
                    }
                }
            }
            println!("{n} record(s), {bad} invalid");
            Ok(if bad == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        Cmd::Run { config, questions: qp, failclosed, out, limit, only, arms, git_sha } => {
            let cfg = EvalConfig::load(&config)?;
            let mut qs = questions::load(&qp)?;
            if !only.is_empty() {
                qs.retain(|q| only.contains(&q.id));
            }
            let smoke = limit.is_some() || !only.is_empty();
            if let Some(n) = limit {
                qs.truncate(n);
            }
            let probes =
                if failclosed.is_empty() { vec![] } else { questions::load(&PathBuf::from(&failclosed))? };
            let arms = arms
                .iter()
                .map(|a| match a.as_str() {
                    "baseline" => Ok(Arm::Baseline),
                    "tome" => Ok(Arm::Tome),
                    other => Err(format!("unknown arm {other}")),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let chunks = ChunkMap::load(&cfg.baseline.chunk_map)?;
            // Until crates/tome-tree lands (R2 freeze) the tome arm is the fail-closed stub.
            let tome: Arc<dyn TomeApi> = Arc::new(StubTome);
            let harness = Harness {
                answerer: Answerer::from_config(&cfg.answer)?,
                jev: SystemOne::from_config(&cfg.jev),
                chunks,
                tome,
                cfg,
            };
            let opts = RunOpts {
                questions_file: qp,
                out_dir: out,
                arms,
                smoke,
                git_sha: git_sha.unwrap_or_else(|| env!("TOME_EVAL_GIT_SHA").to_string()),
            };
            let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            let summary = rt.block_on(harness.run(&qs, &probes, &opts))?;
            println!("{}", serde_json::to_string_pretty(&summary.verdict).map_err(|e| e.to_string())?);
            Ok(ExitCode::SUCCESS)
        }
    }
}
