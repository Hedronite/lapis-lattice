use super::*;

/// Unique per call. `as_nanos()` alone is not: macOS clock granularity is
/// coarser than a nanosecond, so two tests starting together get the same
/// value, share a directory, and clobber each other's fixtures. Tests in a
/// crate run in parallel threads, so pid does not separate them either —
/// hence the counter.
fn vault() -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-lattice-{}-{n}-{seq}", std::process::id()));
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

#[test]
fn index_search_graph_analytics() {
    let d = vault();
    let mut e = Engine::open(&d).unwrap();
    let r = e.reindex().unwrap();
    assert_eq!(r.documents, 2);
    assert!(r.chunks >= 2);
    assert!(r.edges >= 3, "Welcome→Alpha, Welcome→Missing, Alpha→Welcome");

    let hits = e.search("welcome", 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].path, "Welcome.md");
    assert_eq!(hits[0].rank, 1);

    let both = e.neighbors("Welcome.md", "both").unwrap();
    assert!(both.iter().any(|n| n.dst_raw == "Alpha" && n.resolved));
    assert!(both.iter().any(|n| n.dst_raw == "Missing" && !n.resolved && n.path.is_none()));

    let h = e.health().unwrap();
    assert_eq!(h.documents_indexed, 2);
    assert_eq!(h.stale_documents, 0);
    assert!(h.graph.built, "reindex stamps graph_built");
    assert!(h.graph.dangling_links >= 1);
    assert_eq!(h.embedder, "none");
    assert_eq!(h.status, "ok");

    std::fs::write(d.join("notes/New.md"), "# New\n").unwrap();
    assert_eq!(e.health().unwrap().stale_documents, 1, "unindexed file counts as stale");
    std::fs::write(d.join("Welcome.md"), "---\nname: Welcome\ntags: [intro]\n---\n# Welcome\n\nChanged.\n")
        .unwrap();
    assert!(e.health().unwrap().stale_documents >= 1, "mtime drift counts as stale");

    let tags = e.analytics("tags").unwrap();
    assert_eq!(tags.query, "tags");
    assert!(tags.rows.iter().any(|row| row.iter().any(|c| c.contains("intro"))));

    assert!(e.analytics("drop table documents").is_err());
    let _ = std::fs::remove_dir_all(&d);
}

/// B3: every CLI flag reaches the engine. None of them may silently no-op.
#[test]
fn search_params_are_honoured() {
    let d = vault();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();

    // domain is the first path segment; Alpha.md lives under notes/
    let scoped = e
        .search_with(&SearchParams {
            query: "alpha".into(),
            limit: 10,
            domain: Some("notes".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(scoped.hits.iter().all(|h| h.path.starts_with("notes/")));
    let missed = e
        .search_with(&SearchParams {
            query: "alpha".into(),
            limit: 10,
            domain: Some("nope".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(missed.hits.is_empty(), "domain filter actually filters");

    // per_doc collapses to one row per path
    let all =
        e.search_with(&SearchParams { query: "welcome".into(), limit: 50, ..Default::default() }).unwrap();
    let collapsed = e
        .search_with(&SearchParams {
            query: "welcome".into(),
            limit: 50,
            per_doc: true,
            ..Default::default()
        })
        .unwrap();
    let mut paths: Vec<&str> = collapsed.hits.iter().map(|h| h.path.as_str()).collect();
    let before = paths.len();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(paths.len(), before, "per_doc leaves no duplicate paths");
    assert!(collapsed.hits.len() <= all.hits.len());

    // offset pages, and rank stays absolute across pages
    let page1 =
        e.search_with(&SearchParams { query: "welcome".into(), limit: 1, ..Default::default() }).unwrap();
    let page2 = e
        .search_with(&SearchParams { query: "welcome".into(), limit: 1, offset: 1, ..Default::default() })
        .unwrap();
    assert_eq!(page1.hits[0].rank, 1);
    if let Some(h) = page2.hits.first() {
        assert_eq!(h.rank, 2, "offset keeps ranks absolute");
        assert_ne!(h.path.as_str(), page1.hits[0].path.as_str());
    }

    // hybrid runs BM25 alone and says so; vector is refused, not downgraded
    assert_eq!(page1.modalities, ["bm25"]);
    let v = e.search_with(&SearchParams {
        query: "welcome".into(),
        limit: 5,
        mode: Mode::Vector,
        ..Default::default()
    });
    assert!(matches!(v, Err(Error::Usage(m)) if m.contains("needs an embedder")));
    let _ = std::fs::remove_dir_all(&d);
}

/// B10: documents query with filters and paging, carrying hash.
#[test]
fn documents_filter_and_page() {
    let d = vault();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();

    let all = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().all(|r| r.hash.is_some()), "hash lets an agent plan --if-hash writes");
    assert!(all.iter().all(|r| r.updated_at.is_some()));
    assert_eq!(all[0].kind, "markdown");

    let scoped =
        e.documents(&ListParams { prefix: Some("notes/".into()), limit: 50, ..Default::default() }).unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].path, "notes/Alpha.md");
    assert_eq!(scoped[0].priority.as_deref(), Some("high"));

    let tagged =
        e.documents(&ListParams { tag: Some("graph".into()), limit: 50, ..Default::default() }).unwrap();
    assert_eq!(tagged.len(), 1);
    assert_eq!(
        e.documents(&ListParams { tag: Some("nope".into()), limit: 50, ..Default::default() }).unwrap().len(),
        0
    );

    let page = e.documents(&ListParams { limit: 1, offset: 1, ..Default::default() }).unwrap();
    assert_eq!(page.len(), 1);
    assert_ne!(page[0].path, all[0].path);
    let _ = std::fs::remove_dir_all(&d);
}

/// B4: one file in, one file out, without a vault walk re-reading everything.
#[test]
fn reindex_path_is_incremental() {
    let d = vault();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();
    assert!(e.search("kumquat", 10).unwrap().is_empty());

    // append to an existing note: the new word becomes searchable
    std::fs::write(
        d.join("notes/Alpha.md"),
        "---\nname: Alpha\npriority: high\ntags: [graph]\n---\n# Alpha\n\nkumquat harvest.\n",
    )
    .unwrap();
    let r = e.reindex_path("notes/Alpha.md").unwrap();
    assert_eq!(r.documents, 1);
    let hits = e.search("kumquat", 10).unwrap();
    assert_eq!(hits.len(), 1, "FTS rows were replaced, not duplicated");
    assert_eq!(hits[0].path, "notes/Alpha.md");
    assert_eq!(e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap().len(), 2);

    // a brand-new note resolves someone else's dangling link
    std::fs::write(d.join("Missing.md"), "---\nname: Missing\n---\n# Missing\n").unwrap();
    e.reindex_path("Missing.md").unwrap();
    let out = e.neighbors("Welcome.md", "out").unwrap();
    assert!(
        out.iter().any(|n| n.dst_raw == "Missing" && n.resolved),
        "new file resolves an existing dangling edge"
    );

    // deleting the file drops it from the index
    std::fs::remove_file(d.join("Missing.md")).unwrap();
    let r = e.reindex_path("Missing.md").unwrap();
    assert_eq!(r.documents, 0);
    assert!(
        e.documents(&ListParams { limit: 50, ..Default::default() })
            .unwrap()
            .iter()
            .all(|x| x.path != "Missing.md")
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A test double, not a product fallback. Deterministic and content-dependent
/// so cosine ordering is real: different text genuinely gets a different
/// vector. The product never synthesises vectors — see the test below.
struct BagEmbedder {
    model: &'static str,
    dim: usize,
}

impl Embedder for BagEmbedder {
    fn model(&self) -> &str {
        self.model
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn provider(&self) -> &'static str {
        "onnx"
    }
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| bag(t, self.dim)).collect())
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(bag(text, self.dim))
    }
}

fn bag(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for c in text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()) {
        v[(c as usize - 'a' as usize) % dim] += 1.0;
    }
    v
}

/// An embedder that is present but broken — the Ollama-is-down case.
struct DeadEmbedder;

impl Embedder for DeadEmbedder {
    fn model(&self) -> &str {
        "dead-model"
    }
    fn dim(&self) -> usize {
        8
    }
    fn provider(&self) -> &'static str {
        "ollama"
    }
    fn embed_documents(&self, _: &[String]) -> Result<Vec<Vec<f32>>> {
        Err(Error::Usage("ollama unreachable".into()))
    }
    fn embed_query(&self, _: &str) -> Result<Vec<f32>> {
        Err(Error::Usage("ollama unreachable".into()))
    }
}

/// C1/C2/C3: a vault that is not only markdown. One HTML page, one YAML
/// note, and the skip-list directories that must never be walked.
#[test]
fn html_and_yaml_are_indexed_with_their_kind() {
    let d = vault();
    std::fs::write(
        d.join("page.html"),
        "<!doctype html><html><head><title>Release notes</title>\
         <style>b{color:red}</style><script>alert('no')</script></head>\
         <body><h1>Shipping</h1><p>kumquat harvest</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        d.join("deploy.yml"),
        "name: Deploy plan\nstatus: draft\nsteps:\n  - build kumquat\n  - ship\n",
    )
    .unwrap();
    // must never be walked, whatever extension they hold
    for skip in ["node_modules", ".venv"] {
        std::fs::create_dir_all(d.join(skip)).unwrap();
        std::fs::write(d.join(skip).join("junk.yml"), "name: junk\n").unwrap();
        std::fs::write(d.join(skip).join("junk.html"), "<p>junk</p>").unwrap();
    }

    let mut e = Engine::open(&d).unwrap();
    let r = e.reindex().unwrap();
    assert_eq!(r.documents, 4, "two markdown, one html, one yaml — nothing from the skip list");

    let rows = e.documents(&ListParams { limit: 50, ..Default::default() }).unwrap();
    let kind_of = |p: &str| rows.iter().find(|r| r.path == p).map(|r| r.kind.clone());
    assert_eq!(kind_of("page.html").as_deref(), Some("html"));
    assert_eq!(kind_of("deploy.yml").as_deref(), Some("yaml"));
    assert_eq!(kind_of("Welcome.md").as_deref(), Some("markdown"));
    assert!(rows.iter().all(|r| !r.path.contains("node_modules") && !r.path.contains(".venv")));

    // titles come from the format's own idea of a title
    let title = |p: &str| rows.iter().find(|r| r.path == p).and_then(|r| r.title.clone());
    assert_eq!(title("page.html").as_deref(), Some("Release notes"), "<title> wins for html");
    assert_eq!(title("deploy.yml").as_deref(), Some("Deploy plan"), "yaml is its own frontmatter");

    // C3: hits carry the kind recorded at index time
    let hits = e.search("kumquat", 10).unwrap();
    let kinds: std::collections::BTreeSet<&str> = hits.iter().map(|h| h.kind.as_str()).collect();
    assert_eq!(kinds, ["html", "yaml"].into_iter().collect(), "both formats are searchable");
    assert!(hits.iter().any(|h| h.path == "page.html" && h.heading.as_deref() == Some("Shipping")));

    // chrome never reaches the index
    assert!(e.search("alert", 10).unwrap().is_empty(), "script bodies are not searchable");
    assert!(e.search("color", 10).unwrap().is_empty(), "style bodies are not searchable");

    // yaml stays retrievable by its own top-level key
    let by_key = e.search("steps", 10).unwrap();
    assert!(by_key.iter().any(|h| h.path == "deploy.yml"));
    let _ = std::fs::remove_dir_all(&d);
}

/// B9, the named one. A failing embedder must drop the vector arm and write
/// nothing. Storing a placeholder would give cosine 1.0 against every query,
/// so recall would look perfect and be meaningless — worse than no vectors.
#[test]
fn embedder_failure_omits_vector_arm_and_never_writes_a_dummy() {
    let d = vault();
    let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(DeadEmbedder))).unwrap();

    // Indexing still succeeds: files are the source of truth and BM25 answers.
    let r = e.reindex().unwrap();
    assert_eq!(r.documents, 2);
    assert_eq!(r.embedded, 0, "nothing was embedded");
    assert!(r.embed_error.is_some(), "and the failure is reported, not swallowed");

    // The decisive assertion: no placeholder rows.
    let stored: i64 =
        e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).expect("count vectors");
    assert_eq!(stored, 0, "a failed embedder must write no vectors at all");

    // Search still works, and says truthfully that only BM25 ran.
    let res =
        e.search_with(&SearchParams { query: "welcome".into(), limit: 10, ..Default::default() }).unwrap();
    assert!(!res.hits.is_empty(), "BM25 still answers");
    assert_eq!(res.modalities, ["bm25"], "the vector arm is omitted, not faked");
    assert!(
        res.hits.iter().all(|h| h.score < 1.0),
        "no hit carries a cosine-1.0 score from a synthesised vector"
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// A one-note reindex embeds that note, not every chunk still waiting for a
/// vector, and health counts what is embedded against what is indexed.
#[test]
fn reindex_path_embeds_only_its_own_chunks() {
    let d = vault();
    let mut e = Engine::open(&d).unwrap();
    e.reindex().unwrap();
    drop(e);
    let bag = std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 });
    let mut e = Engine::open_with(&d, Some(bag)).unwrap();
    let r = e.reindex_path("./Welcome.md").unwrap();
    let welcome: u64 = e
        .conn
        .query_row("SELECT COUNT(*) FROM chunks WHERE path = 'Welcome.md'", [], |r| r.get::<_, i64>(0))
        .unwrap() as u64;
    assert_eq!(r.embedded, welcome, "only Welcome.md's chunks were embedded");
    let h = e.health().unwrap();
    assert_eq!(h.vectors, welcome);
    assert!(h.chunks > h.vectors, "Alpha.md still waits for a full reindex");
    assert_eq!(e.reindex().unwrap().embedded, h.chunks, "a full reindex embeds every chunk");
    assert_eq!(e.health().unwrap().vectors, e.health().unwrap().chunks);
    let _ = std::fs::remove_dir_all(&d);
}

/// v0.4: the extension is linked into this binary, not loaded from a file.
/// If registration ever stopped happening, every vector path would fail at
/// `CREATE VIRTUAL TABLE` instead of here.
#[test]
fn sqlite_vec_is_linked_and_vec0_is_queryable() {
    let d = vault();
    let mut e =
        Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 }))).unwrap();

    let version: String = e.conn.query_row("SELECT vec_version()", [], |r| r.get(0)).unwrap();
    assert!(version.starts_with('v'), "sqlite-vec answered: {version}");

    // Nothing was loaded from disk to get here, and nothing could be: the
    // C API that enables `load_extension` was never called, so the SQL
    // function refuses.
    let loaded = e.conn.query_row("SELECT load_extension('nonexistent')", [], |r| r.get::<_, String>(0));
    assert!(loaded.is_err(), "load_extension is off, so vec0 can only be here by linking");

    e.reindex().unwrap();
    // The table is a real vec0 virtual table holding the chunk vectors.
    let sql: String =
        e.conn.query_row("SELECT sql FROM sqlite_master WHERE name = 'chunk_vec'", [], |r| r.get(0)).unwrap();
    assert!(sql.contains("USING vec0"), "chunk_vec is a vec0 table: {sql}");
    assert!(sql.contains("float[8]"), "the column is as wide as the embedder: {sql}");
    assert!(sql.contains("distance_metric=cosine"), "cosine, as the Rust scan was: {sql}");
    let _ = std::fs::remove_dir_all(&d);
}

/// v0.4: ranking is a KNN `MATCH` inside SQLite, not every stored blob
/// dragged into Rust and scored there.
#[test]
fn vector_rank_is_a_knn_match_not_a_blob_scan() {
    let d = vault();
    let mut e =
        Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 }))).unwrap();
    e.reindex().unwrap();

    // The planner's own account of the query: a virtual table lookup on
    // chunk_vec, which is what a `MATCH` constraint compiles to.
    let mut stmt = e.conn.prepare(&format!("EXPLAIN QUERY PLAN {}", embed::KNN_SQL)).unwrap();
    let probe = embed::to_blob(&[0.0f32; 8]);
    let plan: Vec<String> = stmt
        .query_map(rusqlite::params![probe, 5i64], |r| r.get::<_, String>(3))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    let plan = plan.join(" | ");
    assert!(plan.contains("chunk_vec"), "the plan reads chunk_vec: {plan}");
    assert!(
        plan.to_lowercase().contains("virtual table"),
        "and reads it as a virtual table, which is where the KNN happens: {plan}"
    );

    // And the query really is a MATCH with a k, not a scan with a LIMIT.
    assert!(embed::KNN_SQL.contains("MATCH"), "{}", embed::KNN_SQL);
    assert!(embed::KNN_SQL.contains("k = ?2"), "{}", embed::KNN_SQL);

    // It answers, and it answers in distance order.
    let ids = embed::vector_rank(&e.conn, &BagEmbedder { model: "bag-v1", dim: 8 }, "welcome", 10).unwrap();
    assert!(!ids.is_empty(), "the KNN arm returns neighbours");
    assert!(ids.len() <= 10);
    let distinct: std::collections::BTreeSet<i64> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len(), "no chunk is returned twice");

    // `k` is honoured as the neighbour count.
    let two = embed::vector_rank(&e.conn, &BagEmbedder { model: "bag-v1", dim: 8 }, "welcome", 2).unwrap();
    assert_eq!(two.len(), 2.min(ids.len()));
    let _ = std::fs::remove_dir_all(&d);
}

/// v0.4: a database written by v0.3 has a blob table. Its vectors are worth
/// keeping, because re-embedding a vault costs a model run per chunk.
#[test]
fn a_pre_v04_blob_table_is_adopted_then_retired() {
    let d = vault();
    {
        // Build the index and let v0.4 write its vectors, then stage the old
        // shape: same rows, in the table an older build would have used.
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        e.reindex().unwrap();
        let ids: Vec<i64> = {
            let mut s = e.conn.prepare("SELECT chunk_id FROM chunks ORDER BY chunk_id").unwrap();
            s.query_map([], |r| r.get(0)).unwrap().filter_map(|r| r.ok()).collect()
        };
        assert!(ids.len() >= 2, "the fixture has chunks to carry over");
        e.conn
            .execute_batch(
                "DROP TABLE IF EXISTS chunk_vec;
                 CREATE TABLE embeddings (chunk_id INTEGER PRIMARY KEY, vec BLOB NOT NULL);",
            )
            .unwrap();
        for (n, id) in ids.iter().enumerate() {
            // One row of the wrong width, to prove it is judged not trusted.
            let wide = if n == 0 { 8 } else { 4 };
            let v: Vec<f32> = (0..wide).map(|i| i as f32).collect();
            e.conn
                .execute(
                    "INSERT INTO embeddings(chunk_id, vec) VALUES(?1, ?2)",
                    rusqlite::params![id, embed::to_blob(&v)],
                )
                .unwrap();
        }
    }
    // Reopening in the same embedding space adopts what fits and retires the
    // old table either way.
    let e =
        Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 }))).unwrap();
    let moved: i64 = e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).unwrap();
    assert_eq!(moved, 1, "only the row of the right width came across");
    let legacy: i64 = e
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'embeddings'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy, 0, "the blob table is gone, so nothing writes to a store nothing reads");
    let _ = std::fs::remove_dir_all(&d);
}

/// Chunk rows and their vectors go together. `vec0` has no foreign key, so
/// without this a reindex would leave neighbours pointing at nothing.
#[test]
fn vectors_follow_their_chunks_on_reindex_and_forget() {
    let d = vault();
    let mut e =
        Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 }))).unwrap();
    let r = e.reindex().unwrap();
    let count =
        |e: &Engine| -> i64 { e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).unwrap() };
    assert_eq!(count(&e), r.chunks as i64);

    // A second full reindex does not double the store.
    let again = e.reindex().unwrap();
    assert_eq!(count(&e), again.chunks as i64, "a reindex replaces vectors, it does not stack them");

    // Every vector still names a live chunk.
    let orphans: i64 = e
        .conn
        .query_row(
            "SELECT COUNT(*) FROM chunk_vec WHERE chunk_id NOT IN (SELECT chunk_id FROM chunks)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0);

    // Removing one note takes its vectors with it.
    std::fs::remove_file(d.join("notes/Alpha.md")).unwrap();
    e.reindex_path("notes/Alpha.md").unwrap();
    let left: i64 = e
        .conn
        .query_row(
            "SELECT COUNT(*) FROM chunk_vec WHERE chunk_id NOT IN (SELECT chunk_id FROM chunks)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "forgetting a path forgets its vectors");
    let _ = std::fs::remove_dir_all(&d);
}

/// The working path: vectors are stored, fusion reports both arms, and a
/// change of embedding space wipes rather than mixing incomparable rows.
#[test]
fn vector_arm_fuses_and_wipes_on_model_change() {
    let d = vault();
    // None when there is no vector table at all, which is what a database
    // with no embedder should look like: not an empty store, no store.
    let vectors = |e: &Engine| -> Option<i64> {
        e.conn.query_row("SELECT COUNT(*) FROM chunk_vec", [], |r| r.get(0)).ok()
    };
    let count = |e: &Engine| -> i64 { vectors(e).expect("vector table exists") };
    {
        let mut e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        let r = e.reindex().unwrap();
        assert!(r.embed_error.is_none());
        assert!(r.embedded > 0 && r.embedded == r.chunks, "every chunk got a vector");
        assert_eq!(count(&e), r.chunks as i64);

        let res = e
            .search_with(&SearchParams { query: "welcome".into(), limit: 10, ..Default::default() })
            .unwrap();
        assert_eq!(res.modalities, ["bm25", "vector"], "both arms ran and are reported");
        assert!(!res.hits.is_empty());

        let h = e.health().unwrap();
        assert_eq!(h.embedder, "onnx");
        assert_eq!(h.embed_model.as_deref(), Some("bag-v1"));
        assert_eq!(h.embed_dim, Some(8));
    }
    // Same model, reopened: vectors survive.
    {
        let e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 8 })))
            .unwrap();
        assert!(count(&e) > 0, "reopening with the same space keeps vectors");
    }
    // Different dimension: the space changed, so stored vectors are dropped.
    {
        let e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 12 })))
            .unwrap();
        assert_eq!(count(&e), 0, "a dimension change wipes incomparable rows");
    }
    // A keyword-only open is not a change of space: it searches BM25 alone
    // and leaves the stored vectors for the next open that has an embedder.
    {
        let mut e =
            Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 12 })))
                .unwrap();
        let stored = e.reindex().unwrap().embedded as i64;
        drop(e);
        let e = Engine::open(&d).unwrap();
        assert_eq!(vectors(&e), Some(stored), "opening without an embedder keeps every vector");
        assert_eq!(e.health().unwrap().embed_dim, Some(12), "and the recorded space");
        let res =
            e.search_with(&SearchParams { query: "welcome".into(), limit: 5, ..Default::default() }).unwrap();
        assert_eq!(res.modalities, ["bm25"]);
        drop(e);
        let e = Engine::open_with(&d, Some(std::sync::Arc::new(BagEmbedder { model: "bag-v1", dim: 12 })))
            .unwrap();
        assert_eq!(count(&e), stored, "the same space reopened finds them");
    }
    let _ = std::fs::remove_dir_all(&d);
}

/// The producer stamp is what keeps this file from ever being an agent-memory store.
#[test]
fn producer_stamp_refuses_a_foreign_database() {
    let d = vault();
    {
        let e = Engine::open(&d).unwrap();
        assert!(e.db_path().exists());
    }
    let db = d.join(".lapis/lattice.sqlite");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('producer','mnemopi')", []).unwrap();
    drop(conn);
    match Engine::open(&d) {
        Err(Error::Usage(m)) => assert!(m.contains("mnemopi") && m.contains("foreign")),
        Err(e) => panic!("expected a usage refusal, got {e:?}"),
        Ok(_) => panic!("expected a refusal, opened a foreign database"),
    }
    let _ = std::fs::remove_dir_all(&d);
}
