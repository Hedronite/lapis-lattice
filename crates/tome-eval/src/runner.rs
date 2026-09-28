//! Runs both arms per question with the same answerer and budget and writes
//! `results.jsonl` + `summary.json` (schema `tome-eval-result` 0.1.0).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::answer::{Answerer, PassageIn, build_prompt, fit_budget};
use crate::baseline::{self, ChunkMap};
use crate::config::EvalConfig;
use crate::contract::{Budget, DocId, TomeApi};
use crate::jev::{JevJudge, SystemOne};
use crate::metrics;
use crate::questions::Question;
use crate::record::*;

pub struct RunOpts {
    pub questions_file: PathBuf,
    pub out_dir: PathBuf,
    pub arms: Vec<Arm>,
    pub smoke: bool,
    pub git_sha: String,
}

pub struct Harness {
    pub cfg: EvalConfig,
    pub answerer: Answerer,
    pub jev: SystemOne,
    pub chunks: ChunkMap,
    pub tome: Arc<dyn TomeApi>,
}

/// Retrieval outcome of one arm before answering.
struct Retrieved {
    passages: Vec<PassageIn>,
    opened: Opened,
    latency_ms: f64,
    judge_calls: Option<u32>,
    judge_prompt_tokens: Option<u64>,
    rerank_status: Option<String>,
    /// `(summary_model, summary_temperature)` from the walked doc's `DocMeta`.
    summary: Option<(String, f64)>,
}

fn now() -> String {
    jiff::Timestamp::now().to_string()
}

fn doc_title(doc: &str) -> String {
    Path::new(doc).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| doc.to_string())
}

impl Harness {
    pub fn model_info(&self) -> ModelInfo {
        let a = &self.cfg.answer;
        ModelInfo {
            model_id: a.model.clone(),
            provider: a.provider.clone(),
            agent: Some(a.agent.clone()),
            temperature: a.temperature,
            max_output_tokens: a.max_output_tokens,
            context_budget_tokens: self.cfg.budget.context_budget_tokens,
        }
    }

    pub fn baseline_settings(&self, rerank_status: Option<String>) -> BaselineSettings {
        let b = &self.cfg.baseline;
        let i = &b.indexer;
        BaselineSettings {
            indexer: i.path.clone(),
            indexer_git_commit: i.git_commit.clone(),
            indexer_sha256: i.sha256.clone(),
            chunk_target_chars: i.chunk_target_chars,
            chunk_max_chars: i.chunk_max_chars,
            chunk_overlap_chars: i.chunk_overlap_chars,
            embedding_model: i.embedding_model.clone(),
            embedding_dim: i.embedding_dim,
            lattice_url: b.lattice_url.clone(),
            lattice_db_fingerprint: i.lattice_db_fingerprint.clone(),
            search: SearchSettings {
                mode: b.mode.clone(),
                top_k: b.top_k,
                retrieve_limit: b.retrieve_limit,
                per_doc: false,
                doc_filter: "same_pdf_post_filter".into(),
                transport: Some(b.transport.clone()),
                retrieve_k: (b.transport == "http").then_some(b.retrieve_k),
                domain: None,
            },
            rerank_jev: RerankSettings {
                enabled: b.rerank_jev,
                transport: self.jev.name().into(),
                status: rerank_status,
                confidence_floor: Some(self.jev.floor),
            },
        }
    }

    pub fn tome_settings(&self) -> TomeSettings {
        let t = &self.cfg.tome;
        TomeSettings {
            backend: self.tome.backend_name().into(),
            builder_version: Some(self.tome.builder_version()),
            beam: t.beam,
            max_judge_calls: t.max_judge_calls,
            max_open_pages: t.max_open_pages,
            max_open_bytes: t.max_open_bytes,
            judge_transport: Some(self.jev.name().into()),
            walk_cache: false,
            index_dir: Some(t.index_dir.display().to_string()),
            summary_model: None,
            summary_temperature: None,
        }
    }

    async fn retrieve_baseline(&self, q: &Question) -> Result<Retrieved, ErrorInfo> {
        let b = &self.cfg.baseline;
        let err = |e: baseline::BaselineError| ErrorInfo { kind: e.kind().into(), message: e.to_string() };
        let domain = b.domain.get(&q.doc).map(String::as_str);
        let (hits, search_ms) = match b.transport.as_str() {
            "lapis" => baseline::lapis_search(b, &q.question).await,
            _ => baseline::http_search(b, &q.question, domain).await,
        }
        .map_err(err)?;
        let mut page = baseline::same_pdf(hits, &q.doc, b.top_k);
        if page.is_empty() {
            return Err(err(baseline::BaselineError::NoHitsForDoc(b.retrieve_limit)));
        }
        let t0 = Instant::now();
        let (status, calls) =
            if b.rerank_jev { self.jev.rerank(&q.question, &mut page).await } else { ("disabled", 0) };
        let rerank_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let passages = baseline::to_passages(&page, &self.chunks, &q.doc_sha256).map_err(err)?;
        let mut pages: Vec<u32> = passages.iter().flat_map(|p| p.page_start..=p.page_end).collect();
        pages.sort_unstable();
        pages.dedup();
        Ok(Retrieved {
            opened: Opened {
                kind: "chunks".into(),
                count: page.len() as u32,
                ids: page.iter().map(|h| h.chunk_id.unwrap_or_default().to_string()).collect(),
                pages,
                bytes: Some(passages.iter().map(|p| p.text.len() as u64).sum()),
            },
            passages,
            latency_ms: search_ms + rerank_ms,
            judge_calls: Some(calls),
            judge_prompt_tokens: None,
            rerank_status: Some(status.into()),
            summary: None,
        })
    }

    async fn retrieve_tome(&self, q: &Question) -> Result<Retrieved, ErrorInfo> {
        let t = &self.cfg.tome;
        let budget = Budget { max_judge_calls: t.max_judge_calls, max_pages: t.max_open_pages };
        let tome = Arc::clone(&self.tome);
        let judge = JevJudge::new(self.jev.clone(), tokio::runtime::Handle::current());
        // The library keys docs by PDF sha256; the path is only on DocMeta.
        let doc = DocId::parse(&q.doc_sha256)
            .map_err(|e| ErrorInfo { kind: "bad_input".into(), message: format!("doc_sha256: {e}") })?;
        let query = q.question.clone();
        let t0 = Instant::now();
        // `Judge` is sync: walk on a blocking thread so the judge can block on the
        // (multi-thread) runtime handle without stalling a worker.
        let (walked, judge) = tokio::task::spawn_blocking(move || {
            let walked = tome.meta(&doc).and_then(|m| Ok((m, tome.walk(&doc, &query, &judge, budget)?)));
            (walked, judge)
        })
        .await
        .map_err(|e| ErrorInfo { kind: "internal".into(), message: format!("walk task: {e}") })?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let (meta, walk) = walked.map_err(|e| ErrorInfo { kind: e.code().into(), message: e.to_string() })?;
        if walk.passages.is_empty() {
            return Err(ErrorInfo {
                kind: "empty".into(),
                message: "walk returned no passages and no error".into(),
            });
        }
        let passages: Vec<PassageIn> = walk
            .passages
            .iter()
            .map(|p| PassageIn {
                label: format!("node {}", p.node_id.0),
                page_start: p.page,
                page_end: p.page,
                text: p.text.clone(),
            })
            .collect();
        let mut pages: Vec<u32> = walk.passages.iter().map(|p| p.page).collect();
        pages.sort_unstable();
        pages.dedup();
        let calls = judge.calls.load(std::sync::atomic::Ordering::Relaxed);
        let chars = judge.prompt_chars.load(std::sync::atomic::Ordering::Relaxed) as u64;
        Ok(Retrieved {
            opened: Opened {
                kind: "nodes".into(),
                count: walk.nodes.len() as u32,
                ids: walk.nodes.iter().map(|n| n.0.clone()).collect(),
                pages,
                bytes: Some(passages.iter().map(|p| p.text.len() as u64).sum()),
            },
            passages,
            latency_ms: ms,
            judge_calls: Some(walk.judge_calls.max(calls)),
            judge_prompt_tokens: Some(chars / 4),
            rerank_status: None,
            summary: Some((meta.summary_model, f64::from(meta.summary_temperature))),
        })
    }

    pub async fn run_one(&self, run_id: &str, q: &Question, arm: Arm, git_sha: &str) -> ResultRecord {
        let gold = q.gold_page_list();
        let retrieved = match arm {
            Arm::Baseline => self.retrieve_baseline(q).await,
            Arm::Tome => self.retrieve_tome(q).await,
        };
        let mut rec = ResultRecord {
            record: "result".into(),
            schema_version: SCHEMA_VERSION.into(),
            run_id: run_id.into(),
            question_id: q.id.clone(),
            doc: q.doc.clone(),
            doc_sha256: q.doc_sha256.clone(),
            arm,
            scored: q.scored,
            answer: None,
            correctness: None,
            grade: Grade {
                status: "not_applicable".into(),
                grader: "jev-systemone".into(),
                score: None,
                confidence: None,
                blind_label: None,
            },
            cited_pages: vec![],
            gold_pages: gold.clone(),
            page_metrics: metrics::page_metrics(&[], &gold),
            latency_ms: Latency::default(),
            tokens: Tokens::default(),
            opened: Opened {
                kind: if arm == Arm::Baseline { "chunks" } else { "nodes" }.into(),
                count: 0,
                ids: vec![],
                pages: vec![],
                bytes: None,
            },
            error: None,
            model: self.model_info(),
            baseline: self.baseline_settings(None),
            tome: self.tome_settings(),
            git_sha: git_sha.into(),
            timestamp: now(),
        };
        let r = match retrieved {
            Ok(r) => r,
            Err(e) => {
                rec.correctness = Some(Correctness::Error);
                rec.grade.score = Some(0);
                rec.error = Some(e);
                return rec;
            }
        };
        rec.baseline = self.baseline_settings(r.rerank_status.clone());
        if arm == Arm::Baseline {
            rec.baseline.search.domain = self.cfg.baseline.domain.get(&q.doc).cloned();
        }
        if let Some((model, temp)) = &r.summary {
            rec.tome.summary_model = Some(model.clone());
            rec.tome.summary_temperature = Some(*temp);
        }
        rec.opened = r.opened;
        rec.latency_ms.retrieve = r.latency_ms;
        rec.tokens.judge_calls = r.judge_calls;
        rec.tokens.judge_prompt_tokens = r.judge_prompt_tokens;
        let (passages, _dropped) = fit_budget(r.passages, self.cfg.budget.context_budget_chars());
        let prompt =
            build_prompt(&doc_title(&q.doc), &q.question, &passages, self.cfg.answer.max_output_tokens);
        match self.answerer.answer(&prompt).await {
            Ok(a) => {
                rec.latency_ms.answer = a.latency_ms;
                rec.tokens.prompt_tokens = a.prompt_tokens;
                rec.tokens.completion_tokens = a.completion_tokens;
                rec.tokens.estimated = a.estimated;
                rec.page_metrics = metrics::page_metrics(&a.cited_pages, &gold);
                rec.cited_pages = a.cited_pages;
                rec.answer = Some(a.answer);
            }
            Err(e) => {
                rec.correctness = Some(Correctness::Error);
                rec.grade.score = Some(0);
                rec.error = Some(ErrorInfo { kind: e.kind().into(), message: e.to_string() });
            }
        }
        rec.latency_ms.total = rec.latency_ms.retrieve + rec.latency_ms.answer;
        if let (Some(ans), Some(reference), true) = (&rec.answer, &q.expected_answer, q.scored) {
            let t0 = Instant::now();
            let g = self.jev.grade(&q.question, reference, ans).await;
            rec.latency_ms.grade = Some(t0.elapsed().as_secs_f64() * 1000.0);
            rec.grade.status = g.status.into();
            rec.grade.confidence = g.confidence;
            rec.correctness = g.correctness;
            rec.grade.score = g.correctness.map(Correctness::score);
        }
        rec
    }

    /// Non-scored fail-closed probe: the tome arm must return an explicit error code.
    pub async fn run_probe(&self, run_id: &str, q: &Question, git_sha: &str) -> (ResultRecord, bool) {
        let rec = self.run_one(run_id, q, Arm::Tome, git_sha).await;
        let expected = q.expect_error.as_ref().is_some_and(|x| {
            rec.error
                .as_ref()
                .is_some_and(|e| x.codes.contains(&e.kind) || x.also_acceptable.contains(&e.kind))
        });
        (rec, expected)
    }

    pub async fn run(
        &self,
        qs: &[Question],
        probes: &[Question],
        opts: &RunOpts,
    ) -> Result<SummaryRecord, String> {
        std::fs::create_dir_all(&opts.out_dir).map_err(|e| format!("{}: {e}", opts.out_dir.display()))?;
        let started = now();
        let run_id = format!("tome-eval-{}", started.replace([':', '.'], "-"));
        let results_path = opts.out_dir.join("results.jsonl");
        let mut f =
            std::fs::File::create(&results_path).map_err(|e| format!("{}: {e}", results_path.display()))?;
        let mut all = Vec::new();
        for q in qs {
            for arm in &opts.arms {
                let rec = self.run_one(&run_id, q, *arm, &opts.git_sha).await;
                writeln!(f, "{}", serde_json::to_string(&rec).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                eprintln!(
                    "{} {:<8} correctness={:?} cited={:?} gold={:?} err={}",
                    rec.question_id,
                    rec.arm.as_str(),
                    rec.correctness,
                    rec.cited_pages,
                    rec.gold_pages,
                    rec.error.as_ref().map_or("-".to_string(), |e| e.kind.clone())
                );
                all.push(rec);
            }
        }
        for p in probes {
            let (rec, ok) = self.run_probe(&run_id, p, &opts.git_sha).await;
            eprintln!(
                "{} probe expected_error={ok} err={:?}",
                rec.question_id,
                rec.error.as_ref().map(|e| &e.kind)
            );
            writeln!(f, "{}", serde_json::to_string(&rec).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            all.push(rec);
        }
        let by = |arm: Arm| all.iter().filter(|r| r.arm == arm).collect::<Vec<_>>();
        let base = metrics::summarize_arm(&by(Arm::Baseline));
        let tome = metrics::summarize_arm(&by(Arm::Tome));
        let pq = metrics::per_question(&all);
        let verdict = metrics::verdict(&base, &tome, &pq, opts.smoke || opts.arms.len() < 2);
        let qbytes = std::fs::read(&opts.questions_file).map_err(|e| e.to_string())?;
        let summary = SummaryRecord {
            record: "summary".into(),
            schema_version: SCHEMA_VERSION.into(),
            run_id,
            questions_file: opts.questions_file.display().to_string(),
            questions_sha256: format!("{:x}", Sha256::digest(&qbytes)),
            n_questions: qs.len() as u32,
            smoke: opts.smoke,
            arms: Arms { baseline: base, tome },
            per_question: pq,
            verdict,
            model: self.model_info(),
            baseline: self.baseline_settings(None),
            tome: self.tome_settings(),
            git_sha: opts.git_sha.clone(),
            started_at: started,
            finished_at: now(),
        };
        let sp = opts.out_dir.join("summary.json");
        std::fs::write(&sp, serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(summary)
    }
}
