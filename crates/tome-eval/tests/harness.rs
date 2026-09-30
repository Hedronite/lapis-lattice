//! Offline end-to-end: fake answerer + fake Jev → records that validate against the
//! committed schema. No network, no lattice. The first test uses the in-memory
//! `FakeTome`; the second builds a real `TomeIndex` from a generated PDF (no book text).

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use tome_eval::answer::Answerer;
use tome_eval::baseline::ChunkMap;
use tome_eval::config::EvalConfig;
use tome_eval::contract::TomeIndex;
use tome_eval::fake::{FakeTome, no_structure_doc, sample_doc};
use tome_eval::jev::SystemOne;
use tome_eval::questions::Question;
use tome_eval::record::Arm;
use tome_eval::runner::{Harness, RunOpts};
use tome_eval::schema_check::{Checker, committed_schema};

fn cfg() -> EvalConfig {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/tome/config.toml");
    let mut c = EvalConfig::load(&p).unwrap();
    c.answer.provider = "fake".into();
    c.baseline.lapis_bin = "/nonexistent/lapis".into();
    // Nothing listens on port 9: the http baseline must fail as lattice_down, not empty.
    c.baseline.lattice_url = "http://127.0.0.1:9".into();
    c
}

fn q(id: &str, doc: &str, sha: &str, scored: bool) -> Question {
    serde_json::from_value(json!({
        "id": id, "doc": doc, "doc_sha256": sha,
        "question": "gamma details of beta?", "expected_answer": if scored { json!("gamma") } else { Value::Null },
        "gold_pages": if scored { json!([[15, 16]]) } else { json!([]) },
        "answer_type": if scored { "fact" } else { "error" }, "difficulty": "deep_subsection",
        "snippets": [], "scored": scored,
        "expect_error": if scored { Value::Null } else { json!({"codes": ["no_structure"]}) }
    }))
    .unwrap()
}

fn grade(choice: &str) -> Value {
    json!({"answers": {"grade": {"choice": choice, "confidence": 0.9}}})
}

/// One-candidate System One reply for the shipped rerank questions.
fn relevance(score: u8, confidence: f64) -> Value {
    json!({"answers": {"relevance": {"score": score, "confidence": confidence}}})
}

fn walk_scores() -> Vec<Value> {
    // Fake tome walk: both roots expand, so their 4 sections are assessed in one round
    // (0001.0001, 0001.0002, 0002.0001, 0002.0002); the top 2 are leaves. Then one grade.
    vec![
        relevance(0, 0.9),
        relevance(0, 0.9),
        relevance(1, 0.9),
        // Low confidence on the winning child: score-only still ranks it first,
        // and the record says a 0.6 gate would have failed this walk closed.
        relevance(3, 0.41),
        grade("exact"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn harness_writes_schema_valid_records_for_both_arms_and_the_probe() {
    let dir = std::env::temp_dir().join(format!("tome-eval-test-{}", std::process::id()));
    let qfile = dir.join("q.jsonl");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&qfile, "{}\n").unwrap();
    let h = Harness {
        answerer: Answerer::Fake { reply: r#"{"answer":"gamma","cited_pages":[15,16]}"#.into() },
        jev: SystemOne::fake(walk_scores()),
        chunks: ChunkMap::default(),
        tome: Arc::new(FakeTome::sample()),
        cfg: cfg(),
    };
    let doc = "Archmagus-Stack/09-Tomes/fake/Fake Book.pdf";
    let qs = vec![q("fake-01", doc, sample_doc().as_str(), true)];
    let probes = vec![q(
        "fake-fc-01",
        "Archmagus-Stack/09-Tomes/fake/No Outline.pdf",
        no_structure_doc().as_str(),
        false,
    )];
    let opts = RunOpts {
        questions_file: qfile,
        out_dir: dir.clone(),
        arms: vec![Arm::Baseline, Arm::Tome],
        smoke: true,
        git_sha: "abcdef1".into(),
    };
    let summary = h.run(&qs, &probes, &opts).await.unwrap();

    let text = std::fs::read_to_string(dir.join("results.jsonl")).unwrap();
    let recs: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(recs.len(), 3);
    let schema = committed_schema();
    let c = Checker::new(&schema);
    for r in &recs {
        assert_eq!(c.validate(r), Vec::<String>::new(), "{r}");
    }
    let s: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json")).unwrap()).unwrap();
    assert_eq!(c.validate(&s), Vec::<String>::new());

    // Baseline could not reach the lattice: explicit error, never an empty success.
    assert_eq!(recs[0]["arm"], "baseline");
    assert_eq!(recs[0]["correctness"], "error");
    assert_eq!(recs[0]["error"]["kind"], "lattice_down");
    assert_eq!(recs[0]["baseline"]["search"]["transport"], "http");
    // Tome walked to the gamma leaf, answered, and was graded exact.
    assert_eq!(recs[1]["arm"], "tome");
    assert_eq!(recs[1]["correctness"], "exact", "{}", recs[1]);
    assert_eq!(recs[1]["page_metrics"]["hit"], true);
    assert_eq!(recs[1]["opened"]["kind"], "nodes");
    assert!(recs[1]["opened"]["ids"].as_array().unwrap().contains(&json!("0002.0002")));
    assert_eq!(recs[1]["tome"]["summary_model"], "fake/lead");
    assert_eq!(recs[1]["tome"]["summary_temperature"], 0.0);
    assert_eq!(recs[1]["tome"]["backend"], "fake");
    assert_eq!(recs[1]["schema_version"], "0.3.0");
    let t = &recs[1]["tome"];
    assert_eq!(
        (t["root_calls"].as_u64(), t["root_batch_size"].as_u64(), t["root_top_k"].as_u64()),
        (Some(4), Some(16), Some(6))
    );
    let ws = &recs[1]["walk_scores"];
    assert_eq!((ws["policy"].as_str(), ws["confidence_floor"].as_f64()), (Some("score_only"), Some(0.6)));
    assert_eq!(ws["would_fail_closed"], true);
    let cands = ws["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 4);
    assert!(cands.iter().any(|c| c["node_id"] == "0002.0002" && c["score"] == 3 && c["confidence"] == 0.41));
    assert_eq!((ws["root_path"].as_str(), ws["root_judge_calls"].as_u64()), (Some("batch"), Some(0)));
    assert!(recs[0]["walk_scores"].is_null(), "baseline records carry no walk scores");
    // Probe fails closed with no_structure and is not scored.
    assert_eq!(recs[2]["scored"], false);
    assert_eq!(recs[2]["error"]["kind"], "no_structure");
    assert_eq!(summary.verdict.decision, "incomplete");
    assert_eq!(summary.arms.tome.silent_empties, 0);
    assert_eq!((summary.walks_judged, summary.would_fail_closed_at_0_6), (1, 1));
    assert_eq!(s["would_fail_closed_at_0_6"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- real root pass: more than 4 roots, target past root 4 -------------------

#[allow(dead_code)]
#[path = "../../tome-tree/tests/support/pdf.rs"]
mod pdf;

const EIGHT_CHAPTERS: [&str; 8] = [
    "Chapter 01",
    "Chapter 02",
    "Chapter 03",
    "Chapter 04",
    "Chapter 05",
    "Chapter 06",
    "Brass foundry",
    "Chapter 08",
];

/// Eight one-page roots from a generated outline PDF, built into a real index.
fn eight_root_index(dir: &Path) -> (TomeIndex, tome_eval::contract::DocMeta) {
    let pdf_path = dir.join("eight.pdf");
    let marks: Vec<pdf::Mark> = EIGHT_CHAPTERS
        .iter()
        .enumerate()
        .map(|(i, title)| pdf::Mark { title, page: i, parent: None })
        .collect();
    pdf::write(&pdf_path, &pdf::prose(8), &marks);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let opts = tome_tree::BuildOptions {
        allow_windows: false,
        force: false,
        llm_struct: false,
        summary_model: tome_tree::SummaryModel {
            provider: "fake".into(),
            model: "lead".into(),
            temperature: 0.0,
        },
        title: "eight".into(),
    };
    let meta = index.build(&pdf_path, "Archmagus-Stack/09-Tomes/fake/Eight.pdf", &opts).unwrap();
    (index, meta)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_jev_batch_judges_every_root_so_a_target_past_root_4_is_found() {
    let dir = std::env::temp_dir().join(format!("tome-eval-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let qfile = dir.join("q.jsonl");
    std::fs::write(&qfile, "{}\n").unwrap();
    let (index, meta) = eight_root_index(&dir);
    let roots = tome_eval::contract::TomeApi::tree(&index, &meta.doc_id, None, Some(1)).unwrap();
    assert_eq!(roots.len(), 8);
    assert_eq!((roots[6].id.0.as_str(), roots[6].page_start), ("0007", 7), "target is root 7 of 8");

    let mut rows: Vec<Value> =
        (1..=8).map(|i| json!({"id": format!("{i:04}"), "score": 0, "confidence": 0.9})).collect();
    rows[6] = json!({"id": "0007", "score": 3, "confidence": 0.9});
    let mut c = cfg();
    assert_eq!(c.tome.root_calls, 4, "the old fan-out judge would have seen roots 1-4 only");
    c.tome.index_dir = dir.join("tomes");
    let h = Harness {
        answerer: Answerer::Fake { reply: r#"{"answer":"brass","cited_pages":[7]}"#.into() },
        // One batch reply for all eight roots, then the grade. Nothing else is canned,
        // so a per-root fan-out would exhaust the fake and fail the walk.
        jev: SystemOne::fake(vec![json!({"answers": rows}), grade("exact")]),
        chunks: ChunkMap::default(),
        tome: Arc::new(index),
        cfg: c,
    };
    let mut question = q("roots-01", "Archmagus-Stack/09-Tomes/fake/Eight.pdf", meta.doc_id.as_str(), true);
    question.question = "where is the brass foundry?".into();
    question.gold_pages = vec![[7, 7]];
    let opts = RunOpts {
        questions_file: qfile,
        out_dir: dir.clone(),
        arms: vec![Arm::Tome],
        smoke: true,
        git_sha: "abcdef1".into(),
    };
    let summary = h.run(&[question], &[], &opts).await.unwrap();
    let text = std::fs::read_to_string(dir.join("results.jsonl")).unwrap();
    let recs: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let schema = committed_schema();
    let checker = Checker::new(&schema);
    assert_eq!(checker.validate(&recs[0]), Vec::<String>::new(), "{}", recs[0]);
    let s: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json")).unwrap()).unwrap();
    assert_eq!(checker.validate(&s), Vec::<String>::new());

    let r = &recs[0];
    assert_eq!(r["error"], Value::Null, "{r}");
    assert_eq!(r["correctness"], "exact");
    assert_eq!(r["tome"]["backend"], "tome_tree");
    assert!(r["opened"]["ids"].as_array().unwrap().contains(&json!("0007")), "{r}");
    assert_eq!(r["page_metrics"]["hit"], true);
    let ws = &r["walk_scores"];
    assert_eq!((ws["root_path"].as_str(), ws["root_judge_calls"].as_u64()), (Some("batch"), Some(1)));
    let cands = ws["candidates"].as_array().unwrap();
    assert_eq!(cands.len(), 8, "every root judged: {ws}");
    assert!(cands.iter().any(|c| c["node_id"] == "0007" && c["score"] == 3 && c["confidence"] == 0.9));
    assert_eq!(ws["would_fail_closed"], false);
    assert_eq!(r["tokens"]["judge_calls"], 1, "one System One call for eight roots");
    assert_eq!((summary.walks_judged, summary.would_fail_closed_at_0_6), (1, 0));
    let _ = std::fs::remove_dir_all(&dir);
}
