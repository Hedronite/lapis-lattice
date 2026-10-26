//! `lapis reindex --all`: rebuild the whole embedded index in place.
//!
//! The same rebuild the TUI and the desktop run from their "build index" actions,
//! for a terminal or a script: progress on stderr while it runs, one report at the end.
//! With `[embedder]` set, every chunk is embedded after the walk; a failing
//! embedder is reported as `embedError` and the keyword index still lands.
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::embedder::Ollama;
use crate::error::{LapisError, Result};
use crate::ops::Ctx;

/// `embedding N` on stderr once the walk is done, until the rebuild returns.
fn tick(o: &Ollama, walked: &AtomicBool, done: &AtomicBool) {
    let mut shown = 0;
    while !done.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        let n = o.embedded();
        if walked.load(Ordering::Relaxed) && n != shown && !done.load(Ordering::Relaxed) {
            shown = n;
            eprint!("\r\x1b[Kembedding {n} chunks");
            let _ = std::io::stderr().flush();
        }
    }
}

pub fn run(ctx: &Ctx) -> Result<()> {
    let (backend, resolved) = ctx.backend_with(None, true)?;
    let ollama = resolved.as_ref().and_then(|r| r.ollama.clone());
    let live = !ctx.json && std::io::stderr().is_terminal();
    let t0 = Instant::now();
    let mut shown = t0;
    let (walked, done) = (AtomicBool::new(false), AtomicBool::new(false));
    let report = std::thread::scope(|s| {
        if live && let Some(o) = &ollama {
            s.spawn(|| tick(o, &walked, &done));
        }
        // The rebuild is blocking SQLite work; keep it off the async workers.
        let r = tokio::task::block_in_place(|| {
            backend.reindex_all_with_progress(&mut |n, total| {
                if live && (n == total || shown.elapsed() >= Duration::from_millis(100)) {
                    shown = Instant::now();
                    eprint!("\rindexing {n}/{total}");
                    let _ = std::io::stderr().flush();
                }
                if n == total {
                    walked.store(true, Ordering::Relaxed);
                }
            })
        });
        done.store(true, Ordering::Relaxed);
        r
    });
    if live {
        eprint!("\r\x1b[K");
    }
    let report = report?;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let embedder = ollama.as_ref().map(|o| {
        use lapis_lattice::Embedder;
        json!({ "provider": o.provider(), "model": o.model(), "dim": o.dim(), "url": o.endpoint() })
    });
    let note = resolved.and_then(|r| r.note);
    if ctx.json {
        return crate::emit_json(&json!({
            "documents": report.documents,
            "chunks": report.chunks,
            "edges": report.edges,
            "embedded": report.embedded,
            "embedError": report.embed_error,
            "embedder": embedder,
            "embedderNote": note,
            "elapsedMs": elapsed_ms,
        }));
    }
    println!(
        "indexed {} notes, {} chunks, {} links, {} embedded  ({:.1} s)",
        report.documents,
        report.chunks,
        report.edges,
        report.embedded,
        elapsed_ms / 1000.0
    );
    if let Some(e) = &report.embed_error {
        eprintln!("lapis: vectors skipped, keyword search still works: {e}");
    } else if let Some(n) = &note {
        eprintln!("lapis: embedder: {n}");
    }
    Ok(())
}

/// `lapis reindex --vectors`: embed what has no vector yet, and nothing else.
/// Resumes after an Ollama outage without re-embedding the whole vault.
pub fn vectors(ctx: &Ctx) -> Result<()> {
    let (backend, resolved) = ctx.backend_with(None, true)?;
    let Some(o) = resolved.as_ref().and_then(|r| r.ollama.clone()) else {
        let why = resolved.and_then(|r| r.note).unwrap_or_else(|| "the HTTP service owns its vectors".into());
        return Err(LapisError::Usage(format!("no embedder to fill vectors with: {why}")));
    };
    let live = !ctx.json && std::io::stderr().is_terminal();
    let t0 = Instant::now();
    let (walked, done) = (AtomicBool::new(true), AtomicBool::new(false));
    let r = std::thread::scope(|s| {
        if live {
            s.spawn(|| tick(&o, &walked, &done));
        }
        let r = tokio::task::block_in_place(|| backend.embed_pending());
        done.store(true, Ordering::Relaxed);
        r
    });
    if live {
        eprint!("\r\x1b[K");
    }
    let (embedded, embed_error) = match r {
        Ok(n) => (n, None),
        Err(e) => (o.embedded(), Some(e.message())),
    };
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if ctx.json {
        return crate::emit_json(&json!({
            "embedded": embedded,
            "embedError": embed_error,
            "elapsedMs": elapsed_ms,
        }));
    }
    println!("embedded {embedded} chunks  ({:.1} s)", elapsed_ms / 1000.0);
    if let Some(e) = &embed_error {
        eprintln!("lapis: stopped early, `lapis reindex --vectors` resumes: {e}");
    }
    Ok(())
}
