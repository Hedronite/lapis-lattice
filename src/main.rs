//! `lapis` — one Rust binary: CLI now, MCP (L3) and Ratatui TUI (L5) later.
//!
//! Read law: `foundry/lapis/SPEC.md`, `SHIP.md`, `CRATES.md`. Files are the
//! write source of truth, the lattice is the read source of truth, and this
//! binary never writes `lattice.db`.

mod cli;
mod config;
mod error;
mod hal;
mod lattice;
mod notes;
mod overlay;
mod vault;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;
use serde_json::json;

use cli::{Cli, Command, NeighborsArgs, ReadArgs, SearchArgs, VaultCommand};
use error::{LapisError, Result};
use lattice::{Client, SearchParams};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // clap exits 2 on usage errors by default; SPEC reserves 2 for "lattice
    // down", so route usage errors through our own code (1).
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind;
            let _ = e.print();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::from(1),
            };
        }
    };
    let json = cli.global.json;
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            report_error(&e, json);
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn report_error(e: &LapisError, json: bool) {
    if json {
        let v = json!({ "error": { "kind": e.kind(), "message": e.message(), "exit": e.exit_code() } });
        println!("{v}");
    }
    eprintln!("lapis: {e}");
}

struct Ctx {
    json: bool,
    vault: vault::Vault,
    cfg: config::Config,
    lattice_url: String,
}

impl Ctx {
    fn client(&self) -> Result<Client> {
        Client::new(&self.lattice_url, self.cfg.lattice.timeout())
    }
}

async fn run(cli: Cli) -> Result<()> {
    let cfg = config::load()?;
    let vault = vault::resolve(cli.global.vault.as_deref(), &cfg)?;
    let lattice_url = cli
        .global
        .lattice
        .clone()
        .or_else(|| std::env::var("LAPIS_LATTICE_URL").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| cfg.lattice.url.clone());
    let ctx = Ctx { json: cli.global.json, vault, cfg, lattice_url };

    match cli.command {
        Command::Vault { command: VaultCommand::Info } => vault_info(&ctx).await,
        Command::Search(args) => search(&ctx, args).await,
        Command::Read(args) => read(&ctx, args),
        Command::Neighbors(args) => neighbors(&ctx, args).await,
    }
}

fn emit_json<T: Serialize>(v: &T) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, v)?;
    out.write_all(b"\n")?;
    Ok(())
}

// ---------------------------------------------------------------- vault info

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultInfo<'a> {
    vault_root: String,
    vault_source: &'static str,
    overlay: OverlayInfo<'a>,
    lattice: LatticeInfo,
}

#[derive(Serialize)]
struct OverlayInfo<'a> {
    source: overlay::OverlaySource,
    path: String,
    #[serde(flatten)]
    overlay: &'a overlay::Overlay,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LatticeInfo {
    url: String,
    reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    health: Option<lattice::Health>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn vault_info(ctx: &Ctx) -> Result<()> {
    let (ov, ov_src) = overlay::load(&ctx.vault.root)?;
    let client = ctx.client()?;
    let health = client.health().await;
    let lattice = match &health {
        Ok(h) => LatticeInfo {
            url: client.base().to_string(),
            reachable: true,
            health: Some(h.clone()),
            error: None,
        },
        Err(e) => LatticeInfo {
            url: client.base().to_string(),
            reachable: false,
            health: None,
            error: Some(e.to_string()),
        },
    };
    let info = VaultInfo {
        vault_root: ctx.vault.root.display().to_string(),
        vault_source: ctx.vault.source,
        overlay: OverlayInfo { source: ov_src, path: overlay::OVERLAY_REL.to_string(), overlay: &ov },
        lattice,
    };

    if ctx.json {
        emit_json(&info)?;
    } else {
        println!("Vault    {}  ({})", info.vault_root, info.vault_source);
        println!(
            "Overlay  {}  inbox={} quick={} archive={} trash={}",
            match info.overlay.source {
                overlay::OverlaySource::File => overlay::OVERLAY_REL,
                overlay::OverlaySource::Default => "defaults",
            },
            ov.buckets.inbox,
            ov.buckets.quick,
            ov.buckets.archive,
            ov.buckets.trash
        );
        match &info.lattice.health {
            Some(h) => println!(
                "Lattice  {}  {}  documents={} edges={} dangling={}",
                info.lattice.url, h.status, h.documents_indexed, h.edges, h.dangling_links
            ),
            None => println!("Lattice  {}  DOWN", info.lattice.url),
        }
    }

    // Always print the report, then fail with exit 2 if the lattice is down.
    match health {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

// --------------------------------------------------------------------- search

async fn search(ctx: &Ctx, args: SearchArgs) -> Result<()> {
    let params = SearchParams {
        query: args.query_text(),
        top_k: args.limit,
        domain: args.domain.clone(),
        mode: args.mode,
        per_doc: args.per_doc,
        mmr: args.mmr,
        include_archives: args.include_archives,
    };
    let client = ctx.client()?;
    let result = client.search(&params).await?;

    if ctx.json {
        return emit_json(&result);
    }
    if result.hits.is_empty() {
        println!("No lattice hits for {:?}.", result.query);
        return Ok(());
    }
    for h in &result.hits {
        let rank = h.rank.map(|r| format!("{r:>2}.")).unwrap_or_else(|| "  ".into());
        println!("{rank} {}", h.path);
        let mut meta = Vec::new();
        if let Some(hd) = &h.heading
            && hd != &h.title
        {
            meta.push(hd.clone());
        }
        if let Some(d) = &h.domain {
            meta.push(format!("domain={d}"));
        }
        if let Some(s) = h.score {
            meta.push(format!("score={s:.4}"));
        }
        println!(
            "    {}{}",
            h.title,
            if meta.is_empty() { String::new() } else { format!("  ·  {}", meta.join("  ")) }
        );
    }
    if let Some(ms) = result.latency_ms {
        eprintln!("{} hits · lattice {:.0} ms · {}", result.count, ms, result.modalities.join("+"));
    }
    Ok(())
}

// ----------------------------------------------------------------------- read

fn read(ctx: &Ctx, args: ReadArgs) -> Result<()> {
    let note = notes::read(&ctx.vault.root, &args.path)?;
    if ctx.json {
        if args.meta {
            return emit_json(&json!({
                "path": note.path, "kind": note.kind, "title": note.title,
                "hal": note.hal, "halValid": note.hal_valid, "halError": note.hal_error,
                "tags": note.tags, "size": note.size, "updatedAt": note.updated_at,
            }));
        }
        if args.body {
            return emit_json(&json!({ "path": note.path, "body": note.body }));
        }
        return emit_json(&note);
    }
    if args.meta {
        return emit_json(&note.hal);
    }
    let mut out = std::io::stdout().lock();
    out.write_all(note.body.as_bytes())?;
    if !note.body.ends_with('\n') {
        out.write_all(b"\n")?;
    }
    if !note.hal_valid && !args.body {
        eprintln!(
            "lapis: {}: frontmatter is not valid YAML ({})",
            note.path,
            note.hal_error.unwrap_or_default()
        );
    }
    Ok(())
}

// ------------------------------------------------------------------ neighbors

async fn neighbors(ctx: &Ctx, args: NeighborsArgs) -> Result<()> {
    // Path escape rules apply even though the lattice, not the disk, answers.
    let rel = notes::clean_rel(&args.path)?;
    let rel = if std::path::Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel };
    let client = ctx.client()?;
    let n = client.neighbors(&rel, &args.direction, !args.dangling).await?;
    if ctx.json {
        return emit_json(&n);
    }
    if n.neighbors.is_empty() {
        println!("No {} neighbors for {}.", n.direction, n.path);
        return Ok(());
    }
    for e in &n.neighbors {
        let arrow = match e.direction.as_str() {
            "in" => "<-",
            _ => "->",
        };
        let flag = if e.resolved { "" } else { "  (dangling)" };
        println!("{arrow} {}{flag}", e.path);
    }
    Ok(())
}
