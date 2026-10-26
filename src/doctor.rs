//! `lapis doctor`, out of `main.rs` so that file stays under the source line cap.

use serde_json::json;

use crate::error::{LapisError, Result};
use crate::notes;
use crate::ops::{self, Ctx, SearchQuery};

/// `lapis doctor`: is this install usable? Per-check rows so CI can assert one
/// thing (`schema/v0.2/doctor.schema.json`), plus notes for a human. A missing
/// embedder is a warning, never a failure — BM25 still answers.
pub async fn run(ctx: &Ctx) -> Result<()> {
    let mut checks: Vec<serde_json::Value> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut ok = true;
    macro_rules! check {
        ($name:expr, $state:expr, $detail:expr) => {{
            let state: &str = $state;
            if state == "fail" {
                ok = false;
            }
            checks.push(json!({ "name": $name, "state": state, "detail": $detail }));
        }};
    }

    let root = ctx.vault.root.clone();
    let vault_ok = root.is_dir();
    check!(
        "vault",
        if vault_ok { "ok" } else { "fail" },
        format!("{} ({})", root.display(), ctx.vault.source)
    );
    let writable = vault_ok && std::fs::create_dir_all(root.join(".lapis")).is_ok();
    check!(
        "vault_writable",
        if writable { "ok" } else { "fail" },
        if writable { ".lapis is writable" } else { "cannot create .lapis" }
    );
    check!("backend", "ok", if ctx.force_http { "http".to_string() } else { ctx.cfg.lattice.mode.clone() });

    let health = ctx.backend()?.health().await;
    let (embedder, health_val) = match &health {
        Ok(h) => {
            check!(
                "index",
                if h.graph.built { "ok" } else { "warn" },
                format!(
                    "{} documents, {} links, {} dangling at {}",
                    h.documents_indexed, h.graph.edges, h.graph.dangling_links, h.db_path
                )
            );
            if !h.graph.built {
                notes.push("index is empty; run `lapis init <vault>` to build it".into());
            }
            (h.embedder.clone(), serde_json::to_value(h).unwrap_or(serde_json::Value::Null))
        }
        Err(e) => {
            check!("index", "fail", e.to_string());
            ("none".to_string(), serde_json::Value::Null)
        }
    };
    if health.is_ok() {
        match crate::stale::check(&ctx.backend()?, &root).await {
            Ok(s) if s.count == 0 => check!("index_fresh", "ok", "every indexed folder matches the disk"),
            Ok(s) => {
                check!("index_fresh", "warn", format!("{} notes changed since indexing", s.count));
                notes.push("`lapis vault info` lists stale notes; `lapis reindex PATH` refreshes one".into());
            }
            Err(e) => check!("index_fresh", "warn", e.to_string()),
        }
    }
    check!(
        "embedder",
        if embedder == "none" { "warn" } else { "ok" },
        if embedder == "none" {
            "none - keyword search only; vectors land in a later slice".to_string()
        } else {
            embedder.clone()
        }
    );

    // A probe that returns nothing is fine. A probe that errors is not.
    let probe = ops::search(
        ctx,
        SearchQuery {
            query: "lapis".into(),
            limit: 1,
            mode: lapis_lattice::Mode::Bm25,
            per_doc: true,
            embedder: Some("none".into()),
            ..Default::default()
        },
    )
    .await;
    match &probe {
        Ok(p) => check!("search", "ok", format!("{} hit(s) for a probe query", p.result.count)),
        Err(e) => check!("search", "fail", e.to_string()),
    }

    let path_ok = notes::clean_rel("../escape").is_err();
    check!("path_sandbox", if path_ok { "ok" } else { "fail" }, "`..` is rejected");

    let report = json!({
        "ok": ok,
        "version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "vault": root.display().to_string(),
        "lattice": ctx.cfg.lattice.mode,
        "embedder": embedder,
        "path_ok": path_ok,
        "checks": checks,
        "health": health_val,
        "notes": notes,
    });
    if ctx.json {
        crate::emit_json(&report)?;
    } else {
        for c in report["checks"].as_array().into_iter().flatten() {
            let mark = match c["state"].as_str() {
                Some("ok") => "ok  ",
                Some("warn") => "warn",
                _ => "FAIL",
            };
            println!(
                "{mark}  {:<16} {}",
                c["name"].as_str().unwrap_or(""),
                c["detail"].as_str().unwrap_or("")
            );
        }
        for n in &notes {
            println!("      note: {n}");
        }
        println!("{}", if ok { "doctor: ok" } else { "doctor: FAILED" });
    }
    if ok { Ok(()) } else { Err(LapisError::LatticeDown("doctor found a failing check".into())) }
}
