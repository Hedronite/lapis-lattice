//! `[embedder]`: the Ollama transport behind the embedded index's vector arm.
//!
//! `lapis-lattice` does no HTTP; it takes an [`Embedder`]. This is the one the
//! CLI hands it. Every failure here is one the engine already absorbs: indexing
//! reports it as `embedError` and search drops the vector arm, so keyword
//! search keeps answering.
//!
//! The engine wipes stored vectors when it is opened with a different model or
//! width. So [`resolve`] never swaps the space just because Ollama is slow or a
//! newly configured model does not answer: when the index already records an
//! Ollama space, it gets an embedder for that space, whose calls may fail but
//! whose vectors stay put. Opening with no embedder leaves the space alone.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use lapis_lattice::{Embedder, StoredEmbedder};
use serde_json::{Value, json};

use crate::config::EmbedderConfig;

/// What `lapis-lattice` records as `meta.embedder` for this transport.
pub const PROVIDER: &str = "ollama";

/// One model behind one Ollama `/api/embed` endpoint.
pub struct Ollama {
    url: String,
    model: String,
    dim: usize,
    query_timeout: Duration,
    index_timeout: Duration,
    /// Chunks embedded through this handle, for the `reindex --all` progress line.
    embedded: AtomicU64,
}

/// nomic-embed-text was trained with task prefixes; without them its query and
/// document vectors sit in different parts of the space.
fn prefixes(model: &str) -> (&'static str, &'static str) {
    if model.contains("nomic-embed") { ("search_document: ", "search_query: ") } else { ("", "") }
}

/// The source chain, because reqwest's top-level message is only "error
/// sending request" and the useful part ("connection refused") is underneath.
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    // HTTP client errors quote the URL they tried, credentials and all.
    crate::redact::text(&out)
}

/// One runtime and one client for every request, so a reindex reuses its
/// connection instead of building a client, a runtime and a thread per batch.
/// The engine calls embedders from sync code that may itself be inside a tokio
/// runtime (CLI, MCP, desktop), where `block_on` may not run, so requests run
/// on this runtime's own worker and the caller waits on a channel.
fn transport() -> Result<&'static (tokio::runtime::Runtime, reqwest::Client), String> {
    static T: OnceLock<Result<(tokio::runtime::Runtime, reqwest::Client), String>> = OnceLock::new();
    T.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("lapis-embed")
            .enable_all()
            .build()
            .map_err(|e| format!("runtime: {e}"))?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .user_agent(concat!("lapis/", env!("CARGO_PKG_VERSION")))
            // A redirect would send note text to a host nobody configured.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| chain(&e))?;
        Ok((rt, client))
    })
    .as_ref()
    .map_err(Clone::clone)
}

async fn request(req: reqwest::RequestBuilder) -> Result<Value, String> {
    let resp = req.send().await.map_err(|e| chain(&e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| chain(&e))?;
    if !status.is_success() {
        // The status only: a reply body is the server's text, not ours to echo.
        return Err(format!("HTTP {status}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("reply is not JSON: {e}"))
}

/// POST `body` and return the JSON reply, through [`transport`].
fn post(url: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    let (rt, client) = transport()?;
    let req = client.post(url).timeout(timeout).json(body);
    let (tx, rx) = std::sync::mpsc::channel();
    rt.spawn(async move {
        let _ = tx.send(request(req).await);
    });
    rx.recv().unwrap_or_else(|_| Err("embedder request was dropped".into()))
}

/// Whether `url` sends text off this machine unencrypted: plain `http` to a
/// host that is not loopback. The operator chose the URL, so this warns.
pub fn cleartext_off_box(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").trim_start_matches('[').trim_end_matches(']');
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    u.scheme() == "http" && !host.is_empty() && !loopback
}

/// `{"embeddings": [[..], ..]}`, one row per input, each `dim` wide (`0` = any).
fn parse(v: &Value, want: usize, dim: usize) -> Result<Vec<Vec<f32>>, String> {
    let rows = v.get("embeddings").and_then(Value::as_array).ok_or("reply has no `embeddings` array")?;
    if rows.len() != want {
        return Err(format!("{} vectors for {want} inputs", rows.len()));
    }
    rows.iter()
        .map(|row| {
            let v: Vec<f32> = row
                .as_array()
                .ok_or("an embedding is not an array")?
                .iter()
                .map(|x| x.as_f64().map(|f| f as f32))
                .collect::<Option<_>>()
                .ok_or("an embedding holds a non-number")?;
            if v.is_empty() || (dim != 0 && v.len() != dim) {
                return Err(format!("a {}-wide vector where {dim} was expected", v.len()));
            }
            Ok(v)
        })
        .collect()
}

impl Ollama {
    fn with(cfg: &EmbedderConfig, model: &str, dim: usize) -> Self {
        Self {
            url: format!("{}/api/embed", cfg.ollama_url.trim_end_matches('/')),
            model: model.to_string(),
            dim,
            query_timeout: Duration::from_millis(cfg.timeout_ms.max(50)),
            index_timeout: Duration::from_millis(cfg.index_timeout_ms.max(cfg.timeout_ms).max(50)),
            embedded: AtomicU64::new(0),
        }
    }

    /// Ask the model once, to learn how wide its vectors are.
    pub fn probe(cfg: &EmbedderConfig, model: &str) -> Result<Self, String> {
        let mut o = Self::with(cfg, model, 0);
        let v = o.call(&[format!("{}lapis", prefixes(model).1)], o.query_timeout)?;
        o.dim = v[0].len();
        Ok(o)
    }

    fn call(&self, input: &[String], timeout: Duration) -> Result<Vec<Vec<f32>>, String> {
        let body = json!({ "model": self.model, "input": input, "truncate": true });
        let reply = post(&self.url, &body, timeout)?;
        parse(&reply, input.len(), self.dim)
    }

    fn fail(&self, e: String) -> lapis_lattice::Error {
        lapis_lattice::Error::Io(std::io::Error::other(format!(
            "ollama {} at {}: {}",
            self.model,
            crate::redact::url(&self.url),
            crate::redact::text(&e)
        )))
    }

    /// One query round trip, for `doctor`.
    pub fn check(&self) -> Result<Duration, String> {
        let t0 = Instant::now();
        self.embed_query("lapis").map_err(|e| e.to_string())?;
        Ok(t0.elapsed())
    }

    pub fn embedded(&self) -> u64 {
        self.embedded.load(Ordering::Relaxed)
    }

    /// The embed URL as shown: no credentials, no query.
    pub fn endpoint(&self) -> String {
        crate::redact::url(&self.url)
    }

    /// Said by `doctor` and the reindex commands when note text would cross
    /// the network unencrypted.
    pub fn cleartext_warning(&self) -> Option<String> {
        cleartext_off_box(&self.url).then(|| {
            format!(
                "{} is plain http to another host: note text and queries cross the network unencrypted; use https or a loopback tunnel",
                self.endpoint()
            )
        })
    }
}

impl Embedder for Ollama {
    fn model(&self) -> &str {
        &self.model
    }
    fn dim(&self) -> usize {
        self.dim
    }
    fn provider(&self) -> &'static str {
        PROVIDER
    }
    fn embed_documents(&self, texts: &[String]) -> lapis_lattice::Result<Vec<Vec<f32>>> {
        let doc = prefixes(&self.model).0;
        let input: Vec<String> = texts.iter().map(|t| format!("{doc}{t}")).collect();
        let out = self.call(&input, self.index_timeout).map_err(|e| self.fail(e))?;
        self.embedded.fetch_add(out.len() as u64, Ordering::Relaxed);
        Ok(out)
    }
    fn embed_query(&self, text: &str) -> lapis_lattice::Result<Vec<f32>> {
        let input = [format!("{}{text}", prefixes(&self.model).1)];
        let mut v = self.call(&input, self.query_timeout).map_err(|e| self.fail(e))?;
        Ok(v.remove(0))
    }
}

/// The embedder a command gets, and why when it is not the configured one.
#[derive(Default)]
pub struct Resolved {
    pub ollama: Option<Arc<Ollama>>,
    pub note: Option<String>,
}

impl Resolved {
    pub fn embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.ollama.clone().map(|o| o as Arc<dyn Embedder>)
    }

    fn none(note: impl Into<String>) -> Self {
        Self { ollama: None, note: Some(note.into()) }
    }

    fn keep(cfg: &EmbedderConfig, s: &StoredEmbedder, note: Option<String>) -> Self {
        Self { ollama: Some(Arc::new(Ollama::with(cfg, &s.model, s.dim as usize))), note }
    }
}

/// Pick the embedder for one command.
///
/// `requested` is a per-call override (`search --embedder`); `none` there means
/// this call never touches the network. `settle` is set by `reindex --all`,
/// `reindex --vectors`, `doctor`, and the TUI and desktop build actions: the
/// moments `auto` or `ollama` may probe Ollama or switch to a new model.
/// Ordinary commands, MCP among them, use the space the index records instead.
pub fn resolve(cfg: &EmbedderConfig, requested: Option<&str>, vault: &Path, settle: bool) -> Resolved {
    let stored =
        lapis_lattice::Engine::stored_embedder(vault).filter(|s| s.provider == PROVIDER && s.dim > 0);
    if requested == Some("none") {
        // Never called on this path (the query skips the vector arm), but it
        // keeps the index in its own space so the vectors survive.
        return match &stored {
            Some(s) => Resolved::keep(cfg, s, None),
            None => Resolved::none("none for this call"),
        };
    }
    let Some(provider) = requested.or(cfg.provider.as_deref()) else {
        return match &stored {
            Some(s) => Resolved::keep(cfg, s, None),
            None => Resolved::none("none - keyword search only; set [embedder] provider = \"ollama\""),
        };
    };
    match provider {
        "none" => Resolved::none("none - keyword search only"),
        "ollama" => {
            if let Some(dim) = cfg.dim.filter(|d| *d > 0) {
                return Resolved { ollama: Some(Arc::new(Ollama::with(cfg, &cfg.model, dim))), note: None };
            }
            if let Some(s) = stored.as_ref().filter(|s| s.model == cfg.model) {
                return Resolved::keep(cfg, s, None);
            }
            if !settle {
                return match &stored {
                    Some(s) => Resolved::keep(
                        cfg,
                        s,
                        Some(format!(
                            "{} is configured; `lapis reindex --all` switches to it from {}",
                            cfg.model, s.model
                        )),
                    ),
                    None => Resolved::none(
                        "ollama: the index records no vectors yet; `lapis reindex --all` settles it",
                    ),
                };
            }
            match Ollama::probe(cfg, &cfg.model) {
                Ok(o) => Resolved { ollama: Some(Arc::new(o)), note: None },
                // A new model that does not answer must not cost the vectors
                // of the one the index already has.
                Err(e) => match &stored {
                    Some(s) => Resolved::keep(
                        cfg,
                        s,
                        Some(format!("{} did not answer ({e}); using the index's {}", cfg.model, s.model)),
                    ),
                    None => Resolved::none(format!(
                        "ollama {} at {} did not answer: {} - keyword search only",
                        cfg.model,
                        crate::redact::url(&cfg.ollama_url),
                        crate::redact::text(&e.to_string())
                    )),
                },
            }
        }
        "auto" => match &stored {
            Some(s) => Resolved::keep(cfg, s, None),
            None if settle => match Ollama::probe(cfg, &cfg.model) {
                Ok(o) => Resolved { ollama: Some(Arc::new(o)), note: None },
                Err(e) => Resolved::none(format!(
                    "auto: no Ollama at {} ({}) - keyword search only",
                    crate::redact::url(&cfg.ollama_url),
                    crate::redact::text(&e.to_string())
                )),
            },
            None => {
                Resolved::none("auto: the index records no embedder yet; `lapis reindex --all` settles it")
            }
        },
        "onnx" => Resolved::none("onnx is not built into this lapis; use ollama or none"),
        other => Resolved::none(format!("unknown embedder provider {other:?}; use none, auto or ollama")),
    }
}

#[cfg(test)]
#[path = "embedder_tests.rs"]
mod tests;
