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

    // `doctor` is one of the moments `auto` may settle, so it probes.
    let (backend, resolved) = ctx.backend_with(None, true)?;
    let health = backend.health().await;
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
                notes.push("`lapis vault info` lists stale notes; `lapis reindex PATH` refreshes one, `lapis reindex --all` rebuilds the index".into());
            }
            Err(e) => check!("index_fresh", "warn", e.to_string()),
        }
    }
    let (state, detail) = embedder_check(resolved.as_ref(), health.as_ref().ok(), &embedder, &mut notes);
    check!("embedder", state, detail);

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

/// The `embedder` row. A dead or missing embedder warns: BM25 still answers.
fn embedder_check(
    resolved: Option<&crate::embedder::Resolved>,
    health: Option<&crate::backend::Health>,
    recorded: &str,
    notes: &mut Vec<String>,
) -> (&'static str, String) {
    use lapis_lattice::Embedder;
    // HTTP: the service owns its embedder; report what it says.
    let Some(r) = resolved else {
        return if recorded == "none" {
            ("warn", "none - keyword search only".into())
        } else {
            ("ok", recorded.into())
        };
    };
    let Some(o) = &r.ollama else {
        let why = r.note.clone().unwrap_or_else(|| "none - keyword search only".into());
        return ("warn", why);
    };
    let (chunks, vectors) = health.map(|h| (h.chunks.unwrap_or(0), h.vectors.unwrap_or(0))).unwrap_or((0, 0));
    let space = format!("ollama {} {}d at {}", o.model(), o.dim(), o.endpoint());
    let mut state = "ok";
    if vectors < chunks {
        state = "warn";
        notes.push(format!(
            "{} of {chunks} chunks have no vector; `lapis reindex --vectors` embeds them",
            chunks - vectors
        ));
    }
    if let Some(n) = &r.note {
        state = "warn";
        notes.push(n.clone());
    }
    match o.check() {
        Ok(t) => (state, format!("{space}, {vectors}/{chunks} chunks embedded, query {} ms", t.as_millis())),
        Err(e) => ("warn", format!("{space} did not answer ({e}) - keyword search until it does")),
    }
}
