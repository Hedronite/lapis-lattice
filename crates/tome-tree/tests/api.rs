//! Public API on authored PDFs: outline, headings, fail-closed, budgets, walk.

#[path = "support/pdf.rs"]
mod pdf;

use std::path::{Path, PathBuf};

use tome_tree::{
    Budget, BuildOptions, DocId, FakeJudge, Judge, NodeId, NodeSource, OPEN_BYTE_CAP, OPEN_PAGE_CAP,
    OpenPassages, SummaryModel, TomeError, TomeIndex,
};

fn scratch(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("tome-tree-{name}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn model() -> SummaryModel {
    SummaryModel { provider: "opencode".into(), model: "test-model".into(), temperature: 0.0 }
}

fn opts(title: &str) -> BuildOptions {
    BuildOptions {
        allow_windows: false,
        force: false,
        llm_struct: false,
        summary_model: model(),
        title: title.into(),
    }
}

fn build_at(dir: &Path, pdf: &Path, title: &str, mut options: BuildOptions) -> tome_tree::DocMeta {
    options.title = title.into();
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    index.build(pdf, &pdf.display().to_string(), &options).unwrap()
}

fn code(err: &TomeError) -> &'static str {
    err.code()
}

#[test]
fn outline_tree_drops_cover_and_open_returns_pages() {
    let dir = scratch("outline");
    let pdf_path = dir.join("outline.pdf");
    let pages = vec![
        vec!["CHAPTER 1", "Introduction", "alpha marker lives on page one."],
        vec!["Section A", "beta marker lives on page two."],
        vec!["CHAPTER 2", "Storage", "gamma marker lives on page three."],
    ];
    pdf::write(
        &pdf_path,
        &pages,
        &[
            pdf::Mark { title: "Cover", page: 0, parent: None },
            pdf::Mark { title: "Chapter One", page: 0, parent: None },
            pdf::Mark { title: "Section A", page: 1, parent: Some(1) },
            pdf::Mark { title: "Chapter Two", page: 2, parent: None },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "outline", opts("outline"));
    assert!(meta.outline);
    assert_eq!(meta.source, NodeSource::Outline);
    assert_eq!(meta.pages, 3);
    assert_eq!(meta.summary_model, "opencode/test-model");
    assert_eq!(meta.summary_temperature, 0.0);
    assert_eq!(meta.doc_id.as_str(), meta.sha256.as_str());
    assert_eq!(meta.doc_id.as_str().len(), 64);

    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let docs = index.docs().unwrap();
    assert_eq!(docs.len(), 1);
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert_eq!(nodes.len(), 2, "cover is front matter");
    assert_eq!(nodes[0].title, "Chapter One");
    assert_eq!(nodes[0].id.as_str(), "0001");
    assert_eq!(nodes[0].level, 1);
    assert_eq!(nodes[0].page_start, 1);
    assert_eq!(nodes[0].page_end, 2);
    assert_eq!(nodes[0].source, NodeSource::Outline);
    assert_eq!(nodes[0].child_count, 1);
    assert_eq!(nodes[0].children[0].id.as_str(), "0001.0001");
    assert_eq!(nodes[0].children[0].title, "Section A");
    assert_eq!(nodes[0].children[0].page_start, 2);
    assert_eq!(nodes[0].children[0].page_end, 2);
    assert_eq!(nodes[1].title, "Chapter Two");
    assert_eq!(nodes[1].page_start, 3);
    assert!(nodes[0].lead.contains("alpha marker") || nodes[0].lead.contains("CHAPTER 1"));
    assert_eq!(nodes[0].summary, nodes[0].lead);
    assert!(nodes[0].lead.chars().count() <= 400);

    let cut = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert!(cut[0].children.is_empty());
    assert_eq!(cut[0].child_count, 1);

    let section = NodeId::from("0001.0001");
    let passages = OpenPassages::open(&index, &meta.doc_id, std::slice::from_ref(&section)).unwrap();
    assert_eq!(passages.len(), 1);
    assert_eq!(passages[0].page, 2);
    assert!(!passages[0].truncated);
    assert!(passages[0].text.contains("beta marker"), "{}", passages[0].text);

    let missing = index.tree(&DocId::from("ab".repeat(32)), None, None).unwrap_err();
    assert_eq!(code(&missing), "unknown_doc");
    let bad = index.open(&meta.doc_id, &[NodeId::from("9999")]).unwrap_err();
    assert_eq!(code(&bad), "unknown_node");

    let again = index.build(&pdf_path, &pdf_path.display().to_string(), &opts("outline")).unwrap();
    assert_eq!(again.built_at, meta.built_at, "cache hit keeps the original build");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headings_when_there_is_no_outline() {
    let dir = scratch("heads");
    let pdf_path = dir.join("heads.pdf");
    let pages = vec![
        vec!["LATTICE HANDBOOK", "CHAPTER 1", "Introduction", "This chapter introduces the lattice index."],
        vec!["LATTICE HANDBOOK", "The index uses fts5 for lexical search over markdown files."],
        vec!["LATTICE HANDBOOK", "CHAPTER 2", "Storage", "Pages live beside the sqlite file."],
        vec!["LATTICE HANDBOOK", "1.1 Page records", "Each page is one jsonl row."],
    ];
    pdf::write(&pdf_path, &pages, &[]);
    let meta = build_at(&dir, &pdf_path, "heads", opts("heads"));
    assert!(!meta.outline);
    assert_eq!(meta.source, NodeSource::Heading);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert_eq!(nodes[0].title, "CHAPTER 1 Introduction");
    assert_eq!(nodes[0].source, NodeSource::Heading);
    assert_eq!(nodes[0].page_start, 1);
    assert_eq!(nodes[0].page_end, 2);
    assert_eq!(nodes[1].title, "CHAPTER 2 Storage");
    assert_eq!(nodes[1].children[0].title, "1.1 Page records");
    assert_eq!(nodes[1].children[0].id.as_str(), "0002.0001");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_structure_errors_and_writes_nothing() {
    let dir = scratch("plain");
    let pdf_path = dir.join("plain.pdf");
    pdf::write(&pdf_path, &pdf::prose(2), &[]);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index.build(&pdf_path, pdf_path.display().to_string().as_str(), &opts("plain")).unwrap_err();
    assert_eq!(code(&err), "no_structure");
    assert!(index.docs().unwrap().is_empty());
    let entries: Vec<_> = std::fs::read_dir(dir.join("tomes")).unwrap().collect();
    assert!(entries.iter().all(|e| {
        let name = e.as_ref().unwrap().file_name();
        !name.to_string_lossy().ends_with(".tree.json")
    }));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allow_windows_is_explicit() {
    let dir = scratch("windows");
    let pdf_path = dir.join("plain.pdf");
    pdf::write(&pdf_path, &pdf::prose(2), &[]);
    let mut options = opts("plain");
    options.allow_windows = true;
    let meta = build_at(&dir, &pdf_path, "plain", options);
    assert_eq!(meta.source, NodeSource::Window);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].source, NodeSource::Window);
    assert_eq!(nodes[0].page_start, 1);
    assert_eq!(nodes[0].page_end, 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn llm_flag_fails_closed() {
    let dir = scratch("llm");
    let pdf_path = dir.join("plain.pdf");
    pdf::write(&pdf_path, &pdf::prose(1), &[]);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let mut options = opts("plain");
    options.llm_struct = true;
    let err = index.build(&pdf_path, "plain.pdf", &options).unwrap_err();
    assert_eq!(code(&err), "no_structure");
    let msg = err.to_string();
    assert!(msg.contains("llm-struct"), "{msg}");
    assert!(index.docs().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn oversized_outline_node_splits_into_windows() {
    let dir = scratch("split");
    let pdf_path = dir.join("long.pdf");
    pdf::write(&pdf_path, &pdf::prose(25), &[pdf::Mark { title: "Whole", page: 0, parent: None }]);
    let meta = build_at(&dir, &pdf_path, "long", opts("long"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].source, NodeSource::Outline);
    assert_eq!(nodes[0].child_count, 3);
    assert_eq!(nodes[0].children[0].source, NodeSource::Window);
    assert_eq!(nodes[0].children[0].page_start, 1);
    assert_eq!(nodes[0].children[0].page_end, 10);
    assert!(nodes[0].children[0].title.contains("(pp. 1–10)"));
    assert_eq!(nodes[0].children[0].id.as_str(), "0001.0001");
    assert_eq!(nodes[0].children[2].page_end, 25);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_budget_is_a_hard_error() {
    let dir = scratch("budget");
    let wide = dir.join("wide.pdf");
    pdf::write(&wide, &pdf::prose(13), &[pdf::Mark { title: "All", page: 0, parent: None }]);
    let meta = build_at(&dir, &wide, "wide", opts("wide"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let root = NodeId::from("0001");
    let err = index.open(&meta.doc_id, std::slice::from_ref(&root)).unwrap_err();
    assert_eq!(code(&err), "over_budget");

    let fit = dir.join("fit.pdf");
    pdf::write(
        &fit,
        &pdf::prose(OPEN_PAGE_CAP as usize),
        &[pdf::Mark { title: "All", page: 0, parent: None }],
    );
    let fit_meta = index.build(&fit, &fit.display().to_string(), &opts("fit")).unwrap();
    let pages = index.open(&fit_meta.doc_id, &[NodeId::from("0001")]).unwrap();
    assert_eq!(pages.len(), OPEN_PAGE_CAP as usize);

    let huge = dir.join("huge.pdf");
    let blob = "A".repeat(OPEN_BYTE_CAP + 64);
    pdf::write(&huge, &[vec![blob.as_str()]], &[pdf::Mark { title: "Blob", page: 0, parent: None }]);
    let huge_meta = index.build(&huge, &huge.display().to_string(), &opts("huge")).unwrap();
    let err = index.open(&huge_meta.doc_id, &[NodeId::from("0001")]).unwrap_err();
    assert_eq!(code(&err), "over_budget");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changed_pdf_is_stale() {
    let dir = scratch("stale");
    let pdf_path = dir.join("book.pdf");
    pdf::write(&pdf_path, &pdf::prose(1), &[pdf::Mark { title: "Only", page: 0, parent: None }]);
    let meta = build_at(&dir, &pdf_path, "book", opts("book"));
    let mut bytes = std::fs::read(&pdf_path).unwrap();
    bytes.push(b' ');
    std::fs::write(&pdf_path, &bytes).unwrap();
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index.tree(&meta.doc_id, None, None).unwrap_err();
    assert_eq!(code(&err), "stale");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn walk_beam_stops_at_short_nodes_and_fails_closed() {
    let dir = scratch("walk");
    let pdf_path = dir.join("walk.pdf");
    let pages = pdf::prose(12);
    pdf::write(
        &pdf_path,
        &pages,
        &[
            pdf::Mark { title: "Alpha", page: 0, parent: None },
            pdf::Mark { title: "A1", page: 0, parent: Some(0) },
            pdf::Mark { title: "A2", page: 3, parent: Some(0) },
            pdf::Mark { title: "Beta", page: 6, parent: None },
            pdf::Mark { title: "B1", page: 6, parent: Some(3) },
            pdf::Mark { title: "B2", page: 9, parent: Some(3) },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "walk", opts("walk"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let judge =
        FakeJudge::new([("0001.0001", 1u8), ("0001.0002", 3u8), ("0002.0001", 3u8), ("0002.0002", 0u8)]);
    let walked = index.walk(&meta.doc_id, "where is the section", &judge, Budget::default()).unwrap();
    assert_eq!(walked.judge_calls, 4);
    assert_eq!(walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0001.0002", "0002.0001"]);
    assert!(walked.passages.iter().all(|p| !p.truncated));
    assert!(walked.passages.len() <= OPEN_PAGE_CAP as usize);

    let tight = Budget { max_judge_calls: 1, max_pages: 12 };
    let err = index.walk(&meta.doc_id, "q", &judge, tight).unwrap_err();
    assert_eq!(code(&err), "over_budget");

    let err = index
        .walk(&meta.doc_id, "q", &FakeJudge::new(std::iter::empty::<(&str, u8)>()), Budget::default())
        .unwrap_err();
    assert_eq!(code(&err), "judge_unavailable");

    struct Wide;
    impl Judge for Wide {
        fn score(&self, _: &str, _: &tome_tree::Candidate) -> tome_tree::Result<u8> {
            Ok(9)
        }
    }
    let err = index.walk(&meta.doc_id, "q", &Wide, Budget::default()).unwrap_err();
    assert_eq!(code(&err), "judge_unavailable");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn walk_does_not_descend_into_a_short_node() {
    let dir = scratch("short");
    let pdf_path = dir.join("short.pdf");
    pdf::write(
        &pdf_path,
        &pdf::prose(3),
        &[
            pdf::Mark { title: "Parent", page: 0, parent: None },
            pdf::Mark { title: "Child", page: 1, parent: Some(0) },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "short", opts("short"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let walked = index
        .walk(&meta.doc_id, "q", &FakeJudge::new(std::iter::empty::<(&str, u8)>()), Budget::default())
        .unwrap();
    assert_eq!(walked.judge_calls, 0);
    assert_eq!(walked.nodes[0].as_str(), "0001");
    assert_eq!(walked.passages.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Vault PDFs are not in this repo. Set `TOME_VAULT` to a directory that
/// contains the spike picks and run with `--ignored`.
#[test]
#[ignore = "needs TOME_VAULT; book PDFs are not committed"]
fn ignored_vault_picks() {
    let root = std::env::var("TOME_VAULT").expect("TOME_VAULT");
    let picks = [
        "01-Earth-DevOps/Site Reliability Engineering.pdf",
        "06-Wood-DataOps/Database Internals - Petrov.pdf",
        "01-Earth-DevOps/The Kubernetes Book - Poulton.pdf",
    ];
    let dir = scratch("vault");
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    for rel in picks {
        let path = PathBuf::from(&root).join(rel);
        let meta = index.build(&path, rel, &opts(rel)).expect(rel);
        assert!(meta.outline, "{rel}");
        let nodes = index.tree(&meta.doc_id, None, Some(2)).unwrap();
        assert!(!nodes.is_empty(), "{rel}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
