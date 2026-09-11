//! Identifier-search goldens (G0/G1). Drive the shipped [`Engine::search_with`].

use super::*;

fn vault() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-lattice-ids-{}-{n}-{seq}", std::process::id()));
    std::fs::create_dir_all(d.join("notes")).unwrap();
    std::fs::write(
        d.join("Welcome.md"),
        "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nSee [[Alpha]] and [[Missing]].\n",
    )
    .unwrap();
    std::fs::write(
        d.join("notes/Alpha.md"),
        "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nBack to [[Welcome]].\n\n- [ ] a task\n",
    )
    .unwrap();
    d
}

/// G0b: query `AGENTS.md` through the shipped `search_with` on a temp vault
/// that indexes that file. Empty hits while it is indexed is a fail.
#[test]
fn search_agents_md_hits_indexed_file() {
    let d = vault();
    std::fs::write(
        d.join("AGENTS.md"),
        "---\nname: AGENTS\n---\n# AGENTS.md\n\nHalo copilot notes for this vault.\n",
    )
    .unwrap();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();
    let indexed = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
    assert!(
        indexed.iter().any(|r| r.path == "AGENTS.md"),
        "fixture AGENTS.md must be in the index before search"
    );

    let res = e
        .search_with(&SearchParams {
            query: "AGENTS.md".into(),
            limit: 10,
            embedder: Some("none".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        res.hits.iter().any(|h| h.path == "AGENTS.md"),
        "AGENTS.md must be in hits while indexed, got {:?}",
        res.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// G1a: path and HAL title are FTS columns, not UNINDEXED. A title-only
/// token (absent from path and body) must still MATCH.
#[test]
fn path_and_title_participate_in_match() {
    let d = vault();
    std::fs::write(
        d.join("notes/title-only.md"),
        "---\nname: Q1TitleZebra\n---\n# Other\n\nUnrelated prose about kittens.\n",
    )
    .unwrap();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();

    let sql: String = e
        .conn
        .query_row("SELECT sql FROM sqlite_master WHERE name = 'chunks_fts'", [], |r| r.get(0))
        .unwrap();
    let l = sql.to_lowercase();
    assert!(!l.contains("path unindexed"), "path must participate in MATCH: {sql}");
    assert!(l.contains("title"), "title must participate in MATCH: {sql}");
    assert!(!l.contains("title unindexed"), "title must not be UNINDEXED: {sql}");

    let res = e
        .search_with(&SearchParams {
            query: "Q1TitleZebra".into(),
            limit: 10,
            embedder: Some("none".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        res.hits.iter().any(|h| h.path == "notes/title-only.md"),
        "HAL title must MATCH, got {:?}",
        res.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// G1b: basename/path identifier ranks #1 against a long distractor body.
/// Empty hits while the file is indexed is a fail.
#[test]
fn search_goal_struct_ranks_first() {
    let d = vault();
    std::fs::create_dir_all(d.join("pack")).unwrap();
    std::fs::write(
        d.join("pack/GOAL-struct.md"),
        "---\nname: GOAL-struct\ntitle: GOAL-struct\n---\n# Paste\n\n\
         Authorized loop text. Ranking uses path and HAL name, not this body.\n",
    )
    .unwrap();
    let decoy = format!(
        "---\nname: Skills Paradigm\n---\n# Skills Paradigm\n\n{}\n",
        "structure skills paradigm hub vibe lattice ranking. ".repeat(80)
    );
    std::fs::write(d.join("notes/Skills-Paradigm.md"), decoy).unwrap();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();
    let indexed = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
    assert!(
        indexed.iter().any(|r| r.path == "pack/GOAL-struct.md"),
        "fixture GOAL-struct.md must be in the index before search"
    );

    let res = e
        .search_with(&SearchParams {
            query: "GOAL-struct".into(),
            limit: 10,
            embedder: Some("none".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(!res.hits.is_empty(), "indexed GOAL-struct.md must not silent-zero, got empty hits");
    assert_eq!(
        res.hits[0].path,
        "pack/GOAL-struct.md",
        "GOAL-struct.md must rank #1, got {:?}",
        res.hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    assert_eq!(res.hits[0].rank, 1);
    let _ = std::fs::remove_dir_all(&d);
}
