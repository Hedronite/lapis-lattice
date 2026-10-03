//! Heading levels and a root pass that scores every root up to the cap.

#[path = "support/pdf.rs"]
#[allow(dead_code)]
mod pdf;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tome_tree::{
    Budget, BuildOptions, FakeJudge, NodeId, NodeSource, ROOT_SCORE_CAP, RootPath, SummaryModel, TomeIndex,
};

fn scratch(name: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("tome-tree-{name}-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn opts(title: &str) -> BuildOptions {
    BuildOptions {
        allow_windows: false,
        force: false,
        llm_struct: false,
        summary_model: SummaryModel {
            provider: "opencode".into(),
            model: "test-model".into(),
            temperature: 0.0,
        },
        title: title.into(),
    }
}

fn build_at(dir: &Path, pdf_path: &Path, title: &str) -> tome_tree::DocMeta {
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    index.build(pdf_path, &pdf_path.display().to_string(), &opts(title)).unwrap()
}

#[test]
fn mixed_font_sizes_build_a_three_level_tree() {
    let dir = scratch("levels");
    let pdf_path = dir.join("levels.pdf");
    pdf::write_sized(
        &pdf_path,
        &[
            vec![("Part One", 22.0), ("Opening prose of the part.", 11.0)],
            vec![("Section Two", 16.0), ("Opening prose of the section.", 11.0)],
            vec![("Detail Three", 12.0), ("Opening prose of the detail.", 11.0)],
        ],
        &[],
    );
    let meta = build_at(&dir, &pdf_path, "levels");
    assert!(!meta.outline);
    assert_eq!(meta.source, NodeSource::Heading);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, None).unwrap();
    assert_eq!(nodes.len(), 1, "{nodes:?}");
    assert_eq!(nodes[0].title, "Part One");
    assert_eq!(nodes[0].level, 1);
    assert_eq!(nodes[0].children.len(), 1);
    assert_eq!(nodes[0].children[0].title, "Section Two");
    assert_eq!(nodes[0].children[0].level, 2);
    let detail = &nodes[0].children[0].children;
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0].title, "Detail Three");
    assert_eq!(detail[0].level, 3);
    assert!(detail[0].children.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headings_without_levels_stay_flat() {
    let dir = scratch("flat");
    let pdf_path = dir.join("flat.pdf");
    let pages = [
        vec!["Alpha Notes", "This page is ordinary prose."],
        vec!["Beta Notes", "This page is ordinary prose."],
        vec!["Gamma Notes", "This page is ordinary prose."],
    ];
    pdf::write(&pdf_path, &pages, &[]);
    let meta = build_at(&dir, &pdf_path, "flat");
    assert_eq!(meta.source, NodeSource::Heading);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let nodes = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert_eq!(nodes.len(), 3, "{nodes:?}");
    assert!(nodes.iter().all(|node| node.child_count == 0), "{nodes:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn outline_book(dir: &Path, name: &str, pages: usize) -> tome_tree::DocMeta {
    let pdf_path = dir.join(format!("{name}.pdf"));
    let titles: Vec<String> = (0..pages).map(|i| format!("Chapter {i:04}")).collect();
    let marks: Vec<pdf::Mark> = titles
        .iter()
        .enumerate()
        .map(|(i, title)| pdf::Mark {
            title: Box::leak(title.clone().into_boxed_str()),
            page: i,
            parent: None,
        })
        .collect();
    pdf::write(&pdf_path, &pdf::prose(pages), &marks);
    build_at(dir, &pdf_path, name)
}

fn covered(walked: &tome_tree::Walk) -> BTreeSet<String> {
    let mut ids: BTreeSet<_> = walked.judged.iter().map(|j| j.node_id.as_str().to_string()).collect();
    ids.extend(walked.roots_skipped.iter().map(|id| id.as_str().to_string()));
    ids
}

#[test]
fn two_hundred_roots_are_all_judged() {
    let dir = scratch("two-hundred");
    let meta = outline_book(&dir, "two-hundred", 200);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let roots = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert_eq!(roots.len(), 200);
    let judge = FakeJudge::new(std::iter::empty::<(&str, u8)>()).with_default(1).batched();
    let walked = index.walk(&meta.doc_id, "chapter", &judge, Budget::default()).unwrap();
    assert_eq!(walked.root_path, RootPath::Batch);
    assert_eq!(walked.judged.len(), 200);
    assert!(walked.roots_skipped.is_empty());
    assert_eq!(walked.root_judge_calls, 13, "200 roots at batch 16 are 13 calls");
    assert_eq!(
        walked.judge_calls, walked.root_judge_calls,
        "one-page leaves do not spend the descent budget"
    );
    assert!(walked.judge_calls <= ROOT_SCORE_CAP as u32);
    let expected: BTreeSet<_> = roots.iter().map(|node| node.id.as_str().to_string()).collect();
    assert_eq!(covered(&walked), expected);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn roots_past_the_cap_are_listed() {
    let pages = ROOT_SCORE_CAP + 8;
    let dir = scratch("capped");
    let meta = outline_book(&dir, "capped", pages);
    let index = TomeIndex::open(&dir.join("tomes")).unwrap();
    let roots = index.tree(&meta.doc_id, None, Some(0)).unwrap();
    assert_eq!(roots.len(), pages);
    let judge = FakeJudge::new(std::iter::empty::<(&str, u8)>()).with_default(1).batched();
    let walked = index.walk(&meta.doc_id, "chapter", &judge, Budget::default()).unwrap();
    assert_eq!(walked.judged.len(), ROOT_SCORE_CAP);
    assert_eq!(walked.roots_skipped.len(), 8);
    assert_eq!(walked.root_judge_calls, (ROOT_SCORE_CAP / 16) as u32);
    assert_eq!(walked.judge_calls, walked.root_judge_calls);
    let expected: BTreeSet<_> = roots.iter().map(|node| node.id.as_str().to_string()).collect();
    assert_eq!(covered(&walked), expected, "every root is judged or listed");
    let skipped: BTreeSet<_> = walked.roots_skipped.iter().map(|id| id.as_str().to_string()).collect();
    assert!(skipped.contains(&format!("{:04}", ROOT_SCORE_CAP + 1)));
    assert!(!skipped.contains(NodeId::from("0001").as_str()));
    let _ = std::fs::remove_dir_all(&dir);
}
