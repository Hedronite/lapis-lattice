//! Offline end-to-end: fake tome backend + fake answerer + fake Jev → records that
//! validate against the committed schema. No network, no lattice, no PDFs.

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use tome_eval::answer::Answerer;
use tome_eval::baseline::ChunkMap;
use tome_eval::config::EvalConfig;
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

fn walk_scores() -> Vec<Value> {
    // Fake tome walk: both roots expand, so their 4 sections are scored in one round
    // (0001.0001, 0001.0002, 0002.0001, 0002.0002); the top 2 are leaves. Then one grade.
    vec![
        json!({"section": {"score": 0, "confidence": 0.9}}),
        json!({"section": {"score": 0, "confidence": 0.9}}),
        json!({"section": {"score": 1, "confidence": 0.9}}),
        json!({"section": {"score": 3, "confidence": 0.9}}),
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
    // Probe fails closed with no_structure and is not scored.
    assert_eq!(recs[2]["scored"], false);
    assert_eq!(recs[2]["error"]["kind"], "no_structure");
    assert_eq!(summary.verdict.decision, "incomplete");
    assert_eq!(summary.arms.tome.silent_empties, 0);
    let _ = std::fs::remove_dir_all(&dir);
}
