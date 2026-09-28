//! Public API on authored PDFs: outline, headings, fail-closed, budgets, walk.

#[path = "support/pdf.rs"]
mod pdf;

use std::path::{Path, PathBuf};

use tome_tree::{
    Budget, BuildOptions, DocId, FakeJudge, Judge, NodeId, NodeSource, OPEN_BYTE_CAP, OPEN_PAGE_CAP,
    OpenPassages, RootPath, SummaryModel, TomeError, TomeIndex,
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

    let missing = index.tree(&DocId::try_from("ab".repeat(32)).unwrap(), None, None).unwrap_err();
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
    let judge = FakeJudge::new([
        ("0001", 3u8),
        ("0002", 3u8),
        ("0001.0001", 1u8),
        ("0001.0002", 3u8),
        ("0002.0001", 3u8),
        ("0002.0002", 0u8),
    ]);
    let walked = index.walk(&meta.doc_id, "where is the section", &judge, Budget::default()).unwrap();
    assert_eq!(walked.judge_calls, 6);
    assert_eq!(walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0001.0002", "0002.0001"]);
    assert!(walked.passages.iter().all(|p| !p.truncated));
    assert!(walked.passages.len() <= OPEN_PAGE_CAP as usize);

    let tight = Budget { max_judge_calls: 1, ..Budget::default() };
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
    let walked = index.walk(&meta.doc_id, "q", &FakeJudge::new([("0001", 2u8)]), Budget::default()).unwrap();
    assert_eq!(walked.judge_calls, 1);
    assert_eq!(walked.nodes[0].as_str(), "0001");
    assert_eq!(walked.passages.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wide_outline_walks_inside_the_default_budget() {
    let dir = scratch("wide");
    let pdf_path = dir.join("wide.pdf");
    let mut marks = vec![
        pdf::Mark { title: "R1", page: 0, parent: None },
        pdf::Mark { title: "R1a", page: 0, parent: Some(0) },
        pdf::Mark { title: "R1b", page: 0, parent: Some(1) },
        pdf::Mark { title: "R2", page: 6, parent: None },
        pdf::Mark { title: "R2a", page: 6, parent: Some(3) },
        pdf::Mark { title: "R2b", page: 6, parent: Some(4) },
    ];
    for i in 0..18 {
        marks.push(pdf::Mark { title: "Later", page: 12 + i * 4, parent: None });
    }
    pdf::write(&pdf_path, &pdf::prose(12 + 18 * 4), &marks);
    let meta = build_at(&dir, &pdf_path, "wide", opts("wide"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let roots = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert_eq!(roots.len(), 20);

    struct PreferFirstTwo;
    impl Judge for PreferFirstTwo {
        fn score(&self, _: &str, candidate: &tome_tree::Candidate) -> tome_tree::Result<u8> {
            let id = candidate.id.as_str();
            if id == "0001" || id == "0002" || id.starts_with("0001.") || id.starts_with("0002.") {
                Ok(3)
            } else {
                Ok(0)
            }
        }
    }
    let walked = index.walk(&meta.doc_id, "deep section", &PreferFirstTwo, Budget::default()).unwrap();
    assert!(walked.judge_calls < 24, "calls {} used the whole judge budget", walked.judge_calls);
    assert_eq!(
        walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
        vec!["0001.0001.0001", "0002.0001.0001"]
    );
    assert!(walked.passages.len() <= OPEN_PAGE_CAP as usize);
    assert!(walked.passages.iter().all(|p| !p.truncated));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn walk_opens_whole_nodes_that_fit_the_page_budget() {
    let dir = scratch("fit");
    let pdf_path = dir.join("fit.pdf");
    pdf::write(
        &pdf_path,
        &pdf::prose(20),
        &[
            pdf::Mark { title: "Left", page: 0, parent: None },
            pdf::Mark { title: "Right", page: 10, parent: None },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "fit", opts("fit"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let judge = FakeJudge::new([("0001", 1u8), ("0002", 3u8)]);
    let walked = index.walk(&meta.doc_id, "q", &judge, Budget::default()).unwrap();
    assert_eq!(walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0002"]);
    assert_eq!(walked.passages.len(), 10);
    assert_eq!(walked.passages.first().unwrap().page, 11);
    assert_eq!(walked.passages.last().unwrap().page, 20);
    assert!(walked.passages.iter().all(|p| !p.truncated));
    assert_eq!(walked.skipped.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0001"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn doc_id_rejects_path_traversal() {
    for raw in ["../secret", "..", "a/../../etc/passwd", "ab", &"g".repeat(64), ""] {
        let err = DocId::parse(raw).unwrap_err();
        assert_eq!(code(&err), "parse", "{raw}");
    }
    assert!(DocId::try_from("../secret").is_err());
    assert!(DocId::try_from("not-a-hash".to_string()).is_err());
    let id = DocId::parse(&"AB".repeat(32)).unwrap();
    assert_eq!(id.as_str(), "ab".repeat(32));

    let dir = scratch("trav");
    let store = dir.join("tomes");
    let index = TomeIndex::open(&store).unwrap();
    let err = index.tree(&id, None, None).unwrap_err();
    assert_eq!(code(&err), "unknown_doc");
    let missing = store.join(format!("{}.tree.json", id.as_str()));
    assert_eq!(missing.parent(), Some(store.as_path()));
    assert!(!dir.join(format!("{}.tree.json", id.as_str())).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_page_is_an_error_and_directory_read_is_io() {
    let dir = scratch("pages");
    let pdf_path = dir.join("p.pdf");
    pdf::write(&pdf_path, &pdf::prose(1), &[pdf::Mark { title: "Only", page: 0, parent: None }]);
    let meta = build_at(&dir, &pdf_path, "p", opts("p"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let pages = dir.join("tomes").join(format!("{}.pages.jsonl", meta.doc_id));
    std::fs::write(&pages, "{\"page\":2,\"text\":\"nope\"}\n").unwrap();
    let err = index.passages(&meta.doc_id, &[NodeId::from("0001")]).unwrap_err();
    assert_eq!(code(&err), "parse");
    assert!(err.to_string().contains("missing"));

    let err = index.build(&dir, "dir", &opts("dir")).unwrap_err();
    assert_eq!(code(&err), "io");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn outline_item_without_destination_is_skipped() {
    let dir = scratch("nodest");
    let pdf_path = dir.join("n.pdf");
    pdf::write(
        &pdf_path,
        &pdf::prose(2),
        &[
            pdf::Mark { title: "Ghost", page: 0, parent: None },
            pdf::Mark { title: "Kept child", page: 1, parent: Some(0) },
            pdf::Mark { title: "Sibling", page: 0, parent: None },
        ],
    );
    pdf::strip_outline_action(&pdf_path, "Ghost");
    let meta = build_at(&dir, &pdf_path, "n", opts("n"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    let titles: Vec<_> = nodes.iter().map(|n| n.title.as_str()).collect();
    assert!(titles.contains(&"Kept child"), "{titles:?}");
    assert!(titles.contains(&"Sibling"), "{titles:?}");
    assert!(!titles.contains(&"Ghost"), "{titles:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dense_leaf_walk_fits_the_byte_cap() {
    let dir = scratch("dense");
    let pdf_path = dir.join("dense.pdf");
    let line = "d".repeat(6_000);
    let pages: Vec<Vec<&str>> = (0..10).map(|_| vec![line.as_str()]).collect();
    pdf::write(&pdf_path, &pages, &[pdf::Mark { title: "Dense", page: 0, parent: None }]);
    let meta = build_at(&dir, &pdf_path, "dense", opts("dense"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert!(nodes[0].child_count >= 2, "dense leaf should split");
    struct Any;
    impl Judge for Any {
        fn score(&self, _: &str, _: &tome_tree::Candidate) -> tome_tree::Result<u8> {
            Ok(1)
        }
    }
    let walked = index.walk(&meta.doc_id, "dense", &Any, Budget::default()).unwrap();
    assert!(!walked.passages.is_empty());
    let bytes: usize = walked.passages.iter().map(|p| p.text.len()).sum();
    assert!(bytes <= OPEN_BYTE_CAP, "opened {bytes} bytes");
    assert!(walked.passages.len() <= OPEN_PAGE_CAP as usize);
    assert!(walked.passages.iter().all(|p| !p.truncated));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_page_is_a_parse_fallback_not_a_panic() {
    let dir = scratch("panic-page");
    let pdf_path = dir.join("bad.pdf");
    pdf::write_panic_page(&pdf_path);
    let meta = build_at(&dir, &pdf_path, "bad", opts("bad"));
    assert!(meta.outline);
    assert_eq!(meta.summary_model, "opencode/test-model");
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    let text: String = nodes.iter().map(|n| n.lead.clone()).collect();
    assert!(text.contains("gamma marker"), "good page survived the bad page: {text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cyclic_page_tree_is_parse_not_a_stack_overflow() {
    let dir = scratch("cycle-pages");
    let pdf_path = dir.join("cycle.pdf");
    pdf::write_cyclic_page_tree(&pdf_path);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index.build(&pdf_path, "cycle.pdf", &opts("cycle")).unwrap_err();
    assert_eq!(code(&err), "parse", "{err}");
    assert!(err.to_string().contains("cycle"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cyclic_outline_is_parse_not_a_stack_overflow() {
    let dir = scratch("cycle-outline");
    let pdf_path = dir.join("cycle.pdf");
    pdf::write_cyclic_outline(&pdf_path);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index.build(&pdf_path, "cycle.pdf", &opts("cycle")).unwrap_err();
    assert_eq!(code(&err), "parse", "{err}");
    assert!(err.to_string().contains("cycle"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn indirect_kids_and_a_deep_page_tree_still_build() {
    let dir = scratch("kids");
    let indirect = dir.join("indirect.pdf");
    pdf::write_indirect_kids(&indirect);
    let meta = build_at(&dir, &indirect, "indirect", opts("indirect"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert!(nodes.iter().any(|n| n.lead.contains("indirect marker")), "{nodes:?}");

    let deep = dir.join("deep.pdf");
    pdf::write_deep_page_tree(&deep);
    let meta = build_at(&dir, &deep, "deep", opts("deep"));
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert!(nodes.iter().any(|n| n.lead.contains("deep marker")), "{nodes:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn image_do_falls_back_per_page_and_keeps_the_text() {
    let dir = scratch("image-do");
    let pdf_path = dir.join("image.pdf");
    pdf::write_image_do(&pdf_path);
    let meta = build_at(&dir, &pdf_path, "image", opts("image"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    let text: String = nodes.iter().map(|n| n.lead.clone()).collect();
    assert!(text.contains("gamma marker"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_broken_destination_does_not_fail_the_book() {
    let dir = scratch("baddest");
    let pdf_path = dir.join("two.pdf");
    pdf::write_broken_dest(&pdf_path);
    let meta = build_at(&dir, &pdf_path, "two", opts("two"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    let titles: Vec<_> = nodes.iter().map(|n| n.title.as_str()).collect();
    assert!(titles.contains(&"Beta"), "{titles:?}");
    assert!(!titles.contains(&"Alpha"), "{titles:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wide_descent_stays_inside_the_budget() {
    let dir = scratch("descend");
    let pdf_path = dir.join("wide.pdf");
    let titles: Vec<&'static str> = (0..40)
        .map(|i| {
            if i == 39 {
                "Brass foundry"
            } else {
                Box::leak(format!("Section {i:02}").into_boxed_str()) as &'static str
            }
        })
        .collect();
    let mut marks = vec![pdf::Mark { title: "Book", page: 0, parent: None }];
    for (i, title) in titles.iter().copied().enumerate() {
        marks.push(pdf::Mark { title, page: i, parent: Some(0) });
    }
    pdf::write(&pdf_path, &pdf::prose(40), &marks);
    let meta = build_at(&dir, &pdf_path, "wide", opts("wide"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let judge = FakeJudge::new([("0001", 3u8), ("0001.0040", 3u8)]).with_default(0).fail_batch();
    let walked = index.walk(&meta.doc_id, "brass foundry", &judge, Budget::default()).unwrap();
    assert!(walked.judge_calls <= 24, "descent spent {}", walked.judge_calls);
    assert!(walked.judge_calls - walked.root_judge_calls <= 8, "{walked:?}");
    assert!(walked.nodes.iter().any(|id| id.as_str() == "0001.0040"), "{:?}", walked.nodes);
    assert!(!walked.passages.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cyclic_form_without_an_outline_is_no_structure() {
    let dir = scratch("cycle-form");
    let pdf_path = dir.join("form.pdf");
    pdf::write_cyclic_form(&pdf_path);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index.build(&pdf_path, "form.pdf", &opts("form")).unwrap_err();
    assert_eq!(code(&err), "no_structure", "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn failed_walk_leaves_summary_model_on_the_doc() {
    let dir = scratch("summary");
    let pdf_path = dir.join("one.pdf");
    pdf::write(
        &pdf_path,
        &[vec!["CHAPTER 1", "Introduction", "alpha marker."]],
        &[pdf::Mark { title: "Chapter", page: 0, parent: None }],
    );
    let meta = build_at(&dir, &pdf_path, "one", opts("one"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let err = index
        .walk(&meta.doc_id, "q", &FakeJudge::new(Vec::<(&str, u8)>::new()), Budget::default())
        .unwrap_err();
    assert_eq!(code(&err), "judge_unavailable");
    let again = index.meta(&meta.doc_id).unwrap();
    assert_eq!(again.summary_model, "opencode/test-model");
    assert_eq!(again.summary_temperature, 0.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn low_confidence_ranks_on_score_and_still_walks() {
    let dir = scratch("conf");
    let pdf_path = dir.join("two.pdf");
    pdf::write(
        &pdf_path,
        &pdf::prose(20),
        &[
            pdf::Mark { title: "Alpha", page: 0, parent: None },
            pdf::Mark { title: "Beta", page: 10, parent: None },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "two", opts("two"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    // Score 3 at confidence 0.1 outranks score 1 at confidence 0.99. The low
    // confidence is recorded and does not abort or drop the candidate.
    let judge =
        FakeJudge::new([("0001", 3u8), ("0002", 1u8)]).with_confidence([("0001", 0.1), ("0002", 0.99)]);
    let walked = index.walk(&meta.doc_id, "beta", &judge, Budget::default()).unwrap();
    assert_eq!(walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0001"]);
    assert_eq!(walked.skipped.iter().map(|id| id.as_str()).collect::<Vec<_>>(), vec!["0002"]);
    assert_eq!(walked.judged.len(), 2);
    assert_eq!(walked.judged[0].score, 3);
    assert_eq!(walked.judged[0].confidence, Some(0.1));
    assert_eq!(walked.judged[0].rank, 3);
    assert_eq!(walked.judged[1].score, 1);
    assert_eq!(walked.judged[1].confidence, Some(0.99));
    assert_eq!(walked.judged[1].rank, 1);
    assert!(!walked.passages.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

const THIRTY_CHAPTERS: [&str; 30] = [
    "Chapter 01",
    "Chapter 02",
    "Chapter 03",
    "Chapter 04",
    "Chapter 05",
    "Chapter 06",
    "Chapter 07",
    "Chapter 08",
    "Chapter 09",
    "Chapter 10",
    "Chapter 11",
    "Chapter 12",
    "Chapter 13",
    "Chapter 14",
    "Chapter 15",
    "Chapter 16",
    "Chapter 17",
    "Chapter 18",
    "Chapter 19",
    "Chapter 20",
    "Chapter 21",
    "Chapter 22",
    "Chapter 23",
    "Chapter 24",
    "Chapter 25",
    "Chapter 26",
    "Chapter 27",
    "Chapter 28",
    "Chapter 29",
    "Brass foundry",
];

fn thirty_root_book(dir: &Path, name: &str) -> tome_tree::DocMeta {
    let pdf_path = dir.join(format!("{name}.pdf"));
    let marks: Vec<pdf::Mark> = THIRTY_CHAPTERS
        .iter()
        .enumerate()
        .map(|(i, title)| pdf::Mark { title, page: i, parent: None })
        .collect();
    pdf::write(&pdf_path, &pdf::prose(30), &marks);
    build_at(dir, &pdf_path, name, opts(name))
}

#[test]
fn thirty_roots_finish_inside_the_root_budget() {
    let dir = scratch("thirty");
    let meta = thirty_root_book(&dir, "thirty");
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let roots = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert_eq!(roots.len(), 30);
    let judge = FakeJudge::new([("0010", 3u8)]).with_default(1).batched();
    let walked = index.walk(&meta.doc_id, "chapter", &judge, Budget::default()).unwrap();
    assert_eq!(walked.root_path, RootPath::Batch);
    assert_eq!(walked.root_judge_calls, 2, "30 roots at batch 16 are two calls");
    assert!(walked.root_judge_calls <= walked.judge_calls);
    assert!(walked.root_judge_calls <= 4);
    assert_eq!(walked.judged.len(), 30);
    assert!(walked.nodes.iter().any(|id| id.as_str() == "0010"));
    assert!(!walked.passages.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn malformed_batch_falls_back_to_lexical_ranking() {
    let dir = scratch("fallback");
    let meta = thirty_root_book(&dir, "fallback");
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    // The matching title is the last root, so an id-order top-k would miss it.
    let judge = FakeJudge::new([("0030", 3u8)]).with_default(0).fail_batch();
    let walked = index.walk(&meta.doc_id, "brass foundry", &judge, Budget::default()).unwrap();
    assert_eq!(walked.root_path, RootPath::LexicalFallback);
    assert_eq!(walked.root_judge_calls, 4, "the failed batch costs one of the four root calls");
    assert_eq!(walked.judged.len(), 3, "three one-at-a-time calls remain after the failed batch");
    assert_eq!(walked.judged[0].node_id.as_str(), "0030");
    assert!(walked.nodes.iter().any(|id| id.as_str() == "0030"));
    assert!(!walked.passages.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn candidate_lead_includes_child_titles() {
    let dir = scratch("children");
    let pdf_path = dir.join("tree.pdf");
    pdf::write(
        &pdf_path,
        &[vec!["CHAPTER 1", "Intro body."], vec!["Section A", "detail body."]],
        &[
            pdf::Mark { title: "Chapter", page: 0, parent: None },
            pdf::Mark { title: "Section A", page: 1, parent: Some(0) },
        ],
    );
    let meta = build_at(&dir, &pdf_path, "tree", opts("tree"));
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    struct See(std::sync::Mutex<String>);
    impl Judge for See {
        fn score(&self, _: &str, c: &tome_tree::Candidate) -> tome_tree::Result<u8> {
            if c.id.as_str() == "0001" {
                *self.0.lock().unwrap() = c.lead.clone();
            }
            Ok(if c.id.as_str() == "0001" { 3 } else { 0 })
        }
    }
    let see = See(std::sync::Mutex::new(String::new()));
    index.walk(&meta.doc_id, "q", &see, Budget::default()).unwrap();
    let lead = see.0.lock().unwrap().clone();
    assert!(lead.contains("Section A"), "{lead}");
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
