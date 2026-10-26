//! `lapis reindex --all`: rebuild the whole embedded index in place.
//!
//! The same rebuild the TUI and the desktop run from their "build index" actions,
//! for a terminal or a script: progress on stderr while it runs, one report at the end.
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::error::Result;
use crate::ops::Ctx;

pub fn run(ctx: &Ctx) -> Result<()> {
    let backend = ctx.backend()?;
    let live = !ctx.json && std::io::stderr().is_terminal();
    let t0 = Instant::now();
    let mut shown = t0;
    // The rebuild is blocking SQLite work; keep it off the async workers.
    let report = tokio::task::block_in_place(|| {
        backend.reindex_all_with_progress(&mut |done, total| {
            if live && (done == total || shown.elapsed() >= Duration::from_millis(100)) {
                shown = Instant::now();
                eprint!("\rindexing {done}/{total}");
                let _ = std::io::stderr().flush();
            }
        })
    });
    if live {
        eprint!("\r\x1b[K");
    }
    let report = report?;
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if ctx.json {
        return crate::emit_json(&json!({
            "documents": report.documents,
            "chunks": report.chunks,
            "edges": report.edges,
            "embedded": report.embedded,
            "embedError": report.embed_error,
            "elapsedMs": elapsed_ms,
        }));
    }
    println!(
        "indexed {} notes, {} chunks, {} links  ({:.1} s)",
        report.documents,
        report.chunks,
        report.edges,
        elapsed_ms / 1000.0
    );
    if let Some(e) = &report.embed_error {
        eprintln!("lapis: vectors skipped, keyword search still works: {e}");
    }
    Ok(())
}
