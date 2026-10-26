//! A fake Ollama on loopback stands in for the real one: no model, no outside
//! network. It answers `/api/embed` with letter-bag vectors, or hangs.

use super::*;
use crate::ops::{self, Ctx, SearchQuery};
use std::io::{Read, Write as _};
use std::net::TcpListener;
use std::sync::Mutex;

const MODEL: &str = "fake-embed";

fn bag(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.01f32; dim];
    for c in text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()) {
        v[(c as usize - 'a' as usize) % dim] += 1.0;
    }
    v
}

struct Fake {
    url: String,
    bodies: Arc<Mutex<Vec<Value>>>,
}

impl Fake {
    fn requests(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
    fn inputs(&self) -> Vec<String> {
        let b = self.bodies.lock().unwrap();
        b.iter()
            .flat_map(|v| v["input"].as_array().cloned().unwrap_or_default())
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect()
    }
}

fn read_request(s: &mut std::net::TcpStream) -> Option<Value> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let len: usize = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&buf[head_end..]).ok()
}

/// `dim` wide vectors; `hang` reads the request and never answers.
fn fake(dim: usize, hang: bool) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = bodies.clone();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let seen = seen.clone();
            std::thread::spawn(move || {
                let Some(body) = read_request(&mut s) else { return };
                seen.lock().unwrap().push(body.clone());
                if hang {
                    std::thread::sleep(Duration::from_secs(5));
                    return;
                }
                let rows: Vec<Vec<f32>> = body["input"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| bag(t.as_str().unwrap_or(""), dim))
                    .collect();
                let reply = json!({ "model": body["model"], "embeddings": rows }).to_string();
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{reply}",
                    reply.len()
                );
            });
        }
    });
    Fake { url, bodies }
}

/// A loopback port with nothing listening on it.
fn dead_url() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", l.local_addr().unwrap())
}

fn cfg(url: &str, provider: Option<&str>) -> EmbedderConfig {
    EmbedderConfig {
        provider: provider.map(str::to_string),
        model: MODEL.into(),
        ollama_url: url.into(),
        timeout_ms: 300,
        index_timeout_ms: 2000,
        ..Default::default()
    }
}

fn vault() -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("lapis-embedder-{}-{n}-{seq}", std::process::id()));
    std::fs::create_dir_all(d.join("notes")).unwrap();
    std::fs::write(d.join("Welcome.md"), "# Welcome\n\nThe quokka is a small macropod.\n").unwrap();
    std::fs::write(d.join("notes/Tide.md"), "# Tide\n\nMoon pulls the ocean twice a day.\n").unwrap();
    d
}

fn ctx(root: &Path, e: EmbedderConfig) -> Ctx {
    let cfg = crate::config::Config { embedder: e, ..Default::default() };
    Ctx {
        json: false,
        vault: crate::vault::Vault { root: root.to_path_buf(), source: "test" },
        cfg,
        lattice_url: crate::config::DEFAULT_LATTICE_URL.into(),
        force_http: false,
    }
}

fn query(q: &str, embedder: Option<&str>) -> SearchQuery {
    SearchQuery { query: q.into(), limit: 10, embedder: embedder.map(str::to_string), ..Default::default() }
}

/// Build the index with vectors from a live fake, and return the chunk count.
async fn build(root: &Path, f: &Fake) -> u64 {
    let (b, _) = ctx(root, cfg(&f.url, Some("ollama"))).backend_with(None, true).unwrap();
    let r = b.reindex_all().await.unwrap();
    assert_eq!(r.embed_error, None);
    assert!(r.chunks > 0 && r.embedded == r.chunks, "every chunk embedded: {r:?}");
    r.chunks
}

async fn vectors(c: &Ctx) -> (u64, u64) {
    let h = c.backend().unwrap().health().await.unwrap();
    (h.vectors.unwrap(), h.chunks.unwrap())
}

#[test]
fn parse_checks_count_and_width() {
    let v = json!({ "embeddings": [[0.5, 1.0], [0.25, 2.0]] });
    assert_eq!(parse(&v, 2, 2).unwrap(), vec![vec![0.5, 1.0], vec![0.25, 2.0]]);
    assert_eq!(parse(&v, 2, 0).unwrap().len(), 2, "width 0 accepts any width (the probe)");
    assert!(parse(&v, 3, 2).unwrap_err().contains("2 vectors for 3 inputs"));
    assert!(parse(&v, 2, 768).unwrap_err().contains("768"));
    assert!(parse(&json!({ "embedding": [1.0] }), 1, 0).is_err());
    assert!(parse(&json!({ "embeddings": [[]] }), 1, 0).is_err(), "an empty vector is not a vector");
    assert!(parse(&json!({ "embeddings": [["x"]] }), 1, 0).is_err());
}

#[test]
fn probe_learns_width_and_nomic_gets_task_prefixes() {
    let f = fake(6, false);
    let o = Ollama::probe(&cfg(&f.url, Some("ollama")), MODEL).unwrap();
    assert_eq!((o.dim(), o.provider(), o.model()), (6, "ollama", MODEL));
    o.embed_documents(&["alpha".into()]).unwrap();
    assert_eq!(f.inputs(), ["lapis", "alpha"], "no prefixes for a model that was not trained with them");

    let f = fake(6, false);
    let o = Ollama::probe(&cfg(&f.url, Some("ollama")), "nomic-embed-text:v1.5").unwrap();
    o.embed_documents(&["alpha".into()]).unwrap();
    o.embed_query("beta").unwrap();
    assert_eq!(f.inputs(), ["search_query: lapis", "search_document: alpha", "search_query: beta"]);
    assert_eq!(o.embedded(), 1, "only documents count toward the progress line");
}

/// The whole wire: config → resolve → engine → `reindex --all` → hybrid search.
#[tokio::test]
async fn configured_ollama_fills_vectors_and_hybrid_runs_the_vector_arm() {
    let d = vault();
    let f = fake(8, false);
    let chunks = build(&d, &f).await;
    let c = ctx(&d, cfg(&f.url, Some("ollama")));
    assert_eq!(vectors(&c).await, (chunks, chunks));

    let page = ops::search(&c, query("ocean moon", None)).await.unwrap();
    assert!(page.result.modalities.iter().any(|m| m == "vector"), "{:?}", page.result.modalities);
    assert_eq!(page.result.hits[0].path, "notes/Tide.md");

    // `--embedder none` is BM25 only and never calls the embedder.
    let before = f.requests();
    let page = ops::search(&c, query("ocean moon", Some("none"))).await.unwrap();
    assert!(!page.result.modalities.iter().any(|m| m == "vector"));
    assert_eq!(f.requests(), before, "`none` must not connect to Ollama");
    assert_eq!(vectors(&c).await, (chunks, chunks), "and must not drop the stored vectors");
    let _ = std::fs::remove_dir_all(&d);
}

/// Ollama gone, or hung past `timeout_ms`: search answers from BM25, the
/// stored vectors survive, and nothing re-probes.
#[tokio::test]
async fn a_dead_or_hung_ollama_degrades_search_to_bm25() {
    let d = vault();
    let chunks = build(&d, &fake(8, false)).await;
    let hung = fake(8, true);
    for url in [dead_url(), hung.url.clone()] {
        let c = ctx(&d, cfg(&url, Some("ollama")));
        let t0 = Instant::now();
        let page = ops::search(&c, query("quokka", None)).await.expect("search must not fail");
        assert!(t0.elapsed() < Duration::from_secs(3), "timeout_ms bounds the wait: {:?}", t0.elapsed());
        assert!(page.result.hits.iter().any(|h| h.path == "Welcome.md"));
        assert!(!page.result.modalities.iter().any(|m| m == "vector"), "{:?}", page.result.modalities);
        assert_eq!(vectors(&c).await, (chunks, chunks), "a dead embedder never costs the vectors");
    }
    assert_eq!(hung.requests(), 1, "one query reached the hung server; opening did not probe it");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn reindex_with_a_dead_ollama_reports_embed_error_and_still_indexes() {
    let d = vault();
    let mut e = cfg(&dead_url(), Some("ollama"));
    e.dim = Some(8);
    let (b, r) = ctx(&d, e).backend_with(None, true).unwrap();
    assert!(r.unwrap().ollama.is_some(), "a configured width needs no probe");
    let report = b.reindex_all().await.unwrap();
    assert_eq!(report.documents, 2);
    assert_eq!(report.embedded, 0);
    assert!(report.embed_error.as_deref().is_some_and(|m| m.contains("ollama")), "{report:?}");
    let _ = std::fs::remove_dir_all(&d);
}

#[tokio::test]
async fn resolution_rules() {
    let d = vault();
    let dead = dead_url();
    // Nothing stored yet.
    assert!(resolve(&cfg(&dead, None), None, &d, true).ollama.is_none(), "unset is none");
    let r = resolve(&cfg(&dead, Some("auto")), None, &d, false);
    assert!(r.ollama.is_none() && r.note.unwrap().contains("reindex --all"), "auto does not probe here");
    let r = resolve(&cfg(&dead, Some("ollama")), None, &d, false);
    assert!(r.ollama.is_none() && r.note.unwrap().contains("did not answer"));
    assert!(resolve(&cfg(&dead, Some("onnx")), None, &d, true).note.unwrap().contains("onnx"));
    let f = fake(8, false);
    assert!(resolve(&cfg(&f.url, Some("auto")), None, &d, true).ollama.is_some(), "settle may probe");

    let chunks = build(&d, &f).await;
    // With an Ollama space stored, every one of these keeps it,
    // and none of these touch the network.
    for (provider, requested) in [(None, None), (Some("auto"), None), (Some("ollama"), Some("none"))] {
        let r = resolve(&cfg(&dead, provider), requested, &d, false);
        let o = r.ollama.unwrap_or_else(|| panic!("{provider:?}/{requested:?} keeps the stored space"));
        assert_eq!((o.model(), o.dim()), (MODEL, 8));
    }
    let c = ctx(&d, crate::config::EmbedderConfig::default());
    assert_eq!(vectors(&c).await, (chunks, chunks), "a run without [embedder] keeps the vectors");
    // A new model that does not answer keeps the old space rather than wiping it.
    let mut other = cfg(&dead, Some("ollama"));
    other.model = "other-model".into();
    let r = resolve(&other, None, &d, false);
    assert_eq!(r.ollama.unwrap().model(), MODEL);
    assert!(r.note.unwrap().contains("other-model"));

    let c = ctx(&d, cfg(&dead, Some("none")));
    assert!(resolve(&cfg(&dead, Some("none")), None, &d, true).ollama.is_none());
    assert_eq!(vectors(&c).await, (chunks, chunks), "even an explicit none only stops using the vectors");
    let _ = std::fs::remove_dir_all(&d);
}

/// `reindex --vectors` fills what an outage left without vectors, and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn reindex_vectors_resumes_after_an_outage() {
    let d = vault();
    let mut down = cfg(&dead_url(), Some("ollama"));
    down.dim = Some(8);
    let (b, _) = ctx(&d, down).backend_with(None, true).unwrap();
    let r = b.reindex_all().await.unwrap();
    assert!(r.embed_error.is_some() && r.embedded == 0);
    drop(b);

    let f = fake(8, false);
    let c = ctx(&d, cfg(&f.url, Some("ollama")));
    assert_eq!(vectors(&c).await, (0, r.chunks));
    crate::reindex_all::vectors(&c).unwrap();
    assert_eq!(vectors(&c).await, (r.chunks, r.chunks));
    let before = f.requests();
    crate::reindex_all::vectors(&c).unwrap();
    assert_eq!(f.requests(), before, "nothing left to embed, so no request");

    let none = ctx(&d, cfg(&dead_url(), Some("none")));
    assert!(crate::reindex_all::vectors(&none).is_err(), "no embedder is a usage error, not a silent zero");
    let _ = std::fs::remove_dir_all(&d);
}
