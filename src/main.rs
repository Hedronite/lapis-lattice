//! `lapis` — one Rust binary: CLI, MCP (`lapis mcp`), Ratatui TUI (L5).
//!
//! Read law: `foundry/lapis/SPEC.md`, `SHIP.md`, `CRATES.md`. Files are the
//! write source of truth, the lattice is the read source of truth, and this
//! binary never writes `lattice.db`.

mod cli;
mod config;
mod error;
mod hal;
mod lattice;
mod mcp;
mod notes;
mod ops;
mod overlay;
mod tasks;
mod taxonomy;
mod templates;
mod tui;
mod vault;
mod write;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use serde::Serialize;
use serde_json::json;

use cli::{
    AppendArgs, CaptureArgs, Cli, Command, CreateArgs, DailyArgs, ListArgs, NeighborsArgs, ReadArgs,
    ReindexArgs, SearchArgs, TaskCommand, TaskListArgs, TaskToggleArgs, TemplateCommand, TrashArgs,
    VaultCommand,
};
use error::{LapisError, Result};
use lattice::{ListParams, SearchParams};
use ops::Ctx;

// Multi-thread so the TUI can block its thread while lattice requests run.
#[tokio::main]
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

    match cli.command() {
        Command::Vault { command: VaultCommand::Info } => vault_info(&ctx).await,
        Command::Search(args) => search(&ctx, args).await,
        Command::Read(args) => read(&ctx, args),
        Command::Neighbors(args) => neighbors(&ctx, args).await,
        Command::List(args) => list(&ctx, args).await,
        Command::Reindex(args) => reindex(&ctx, args).await,
        Command::Create(args) => create(&ctx, args).await,
        Command::Append(args) => append(&ctx, args).await,
        Command::Capture(args) => capture(&ctx, args).await,
        Command::Task { command: TaskCommand::List(args) } => task_list(&ctx, args),
        Command::Task { command: TaskCommand::Toggle(args) } => task_toggle(&ctx, args).await,
        Command::Mcp => mcp::serve(ctx).await,
        Command::Daily(args) => periodic(&ctx, write::Period::Daily, args).await,
        Command::Weekly(args) => periodic(&ctx, write::Period::Weekly, args).await,
        Command::Monthly(args) => periodic(&ctx, write::Period::Monthly, args).await,
        Command::Trash(args) => trash(&ctx, args),
        Command::Restore(args) => restore(&ctx, args).await,
        Command::Template { command } => template(&ctx, command),
        Command::Tui => tui::run(ctx).await,
    }
}

fn emit_json<T: Serialize>(v: &T) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, v)?;
    out.write_all(b"\n")?;
    Ok(())
}

fn meta_line(parts: &[Option<String>]) -> String {
    let m: Vec<&str> = parts.iter().flatten().map(String::as_str).collect();
    if m.is_empty() { String::new() } else { format!("  ·  {}", m.join("  ")) }
}

// ---------------------------------------------------------------- vault info

async fn vault_info(ctx: &Ctx) -> Result<()> {
    let (info, err) = ops::vault_info(ctx).await?;
    if ctx.json {
        emit_json(&info)?;
    } else {
        let ov = &info.overlay.overlay;
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
    match err {
        Some(e) => Err(e),
        None => Ok(()),
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
    let result = ctx.client()?.search(&params).await?;
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
        let heading = h.heading.clone().filter(|hd| hd != &h.title);
        let meta = meta_line(&[
            heading,
            h.domain.as_ref().map(|d| format!("domain={d}")),
            h.score.map(|s| format!("score={s:.4}")),
        ]);
        println!("    {}{meta}", h.title);
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

// ----------------------------------------------------------------------- list

async fn list(ctx: &Ctx, args: ListArgs) -> Result<()> {
    let params = ListParams {
        domain: args.domain,
        doc_type: args.doc_type,
        status: args.status,
        tag: args.tag,
        prefix: args.prefix,
        limit: args.limit,
        offset: args.offset,
        include_archives: args.include_archives,
    };
    let rows = ops::list(ctx, params).await?;
    if ctx.json {
        return emit_json(&rows);
    }
    if rows.is_empty() {
        println!("No documents match.");
        return Ok(());
    }
    for r in &rows {
        println!("{}", r.path);
        println!("    {}{}", r.title, meta_line(&[r.doc_type.clone(), r.status.clone()]));
    }
    Ok(())
}

// -------------------------------------------------------------------- reindex

async fn reindex(ctx: &Ctx, args: ReindexArgs) -> Result<()> {
    // Validate locally first so an escape is exit 3 before any HTTP.
    let (rel, _abs) = notes::resolve(&ctx.vault.root, &args.path)?;
    let r = ctx.client()?.reindex(&rel).await?;
    if ctx.json {
        return emit_json(&r);
    }
    let what = match (r.changed, r.chunks) {
        (Some(true), Some(n)) => format!("re-indexed, {n} chunks"),
        (Some(false), _) => "unchanged".to_string(),
        _ => "done".to_string(),
    };
    println!("{}  {}{}", r.path, what, r.elapsed_ms.map(|ms| format!("  ({ms:.0} ms)")).unwrap_or_default());
    if !r.ok {
        for l in &r.stderr_tail {
            eprintln!("  {l}");
        }
        return Err(LapisError::LatticeDown(format!("indexer exit {}", r.indexer_exit.unwrap_or(-1))));
    }
    Ok(())
}

// ---------------------------------------------------------------------- write

fn read_stdin() -> Result<String> {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
    Ok(s)
}

fn print_write(ctx: &Ctx, report: &ops::WriteReport) -> Result<()> {
    if ctx.json {
        emit_json(report)?;
    } else {
        let idx = match (&report.reindex, &report.reindex_error) {
            (Some(r), _) if r.changed == Some(true) => {
                format!("  indexed ({} chunks)", r.chunks.unwrap_or(0))
            }
            (Some(_), _) => "  index unchanged".into(),
            (None, Some(_)) => "  (index kick failed)".into(),
            (None, None) => String::new(),
        };
        println!("{}{idx}", report.written.path);
    }
    if let Some(e) = &report.reindex_error {
        eprintln!("lapis: written, but reindex failed: {e}");
    }
    Ok(())
}

async fn create(ctx: &Ctx, args: CreateArgs) -> Result<()> {
    let body = if args.stdin { Some(read_stdin()?) } else { args.body.clone() };
    let opts = write::CreateOpts {
        title: args.title.clone(),
        path: args.path.clone(),
        template: args.template.clone(),
        doc_type: args.doc_type.clone(),
        domain: args.domain.clone(),
        tags: args.tags.clone(),
        body,
        operator: ctx.cfg.operator.name.clone(),
        inbox: ctx.inbox()?,
        director: args.director.clone(),
        template_date: None,
    };
    let w = write::create(&ctx.vault.root, &opts)?;
    print_write(ctx, &ops::kick(ctx, w, args.no_reindex).await?)
}

async fn append(ctx: &Ctx, args: AppendArgs) -> Result<()> {
    let text = match &args.text {
        Some(t) => t.clone(),
        None => read_stdin()?,
    };
    if text.trim().is_empty() {
        return Err(LapisError::Usage("nothing to append".into()));
    }
    let w = write::append(&ctx.vault.root, &args.path, &text)?;
    print_write(ctx, &ops::kick(ctx, w, args.no_reindex).await?)
}

async fn capture(ctx: &Ctx, args: CaptureArgs) -> Result<()> {
    let text = if args.text.is_empty() { read_stdin()? } else { args.text.join(" ") };
    let inbox = ctx.inbox()?;
    let w = write::capture(&ctx.vault.root, &text, &inbox, ctx.cfg.operator.name.clone())?;
    print_write(ctx, &ops::kick(ctx, w, args.no_reindex).await?)
}

// ------------------------------------------------------------ daily / trash

async fn periodic(ctx: &Ctx, period: write::Period, args: DailyArgs) -> Result<()> {
    let r = ops::periodic(ctx, period, args.date.as_deref(), args.no_reindex).await?;
    if ctx.json {
        emit_json(&r)?;
    } else {
        println!("{}{}", r.daily.path, if r.daily.created { "  (created)" } else { "" });
    }
    if let Some(e) = &r.reindex_error {
        eprintln!("lapis: created, but reindex failed: {e}");
    }
    Ok(())
}

async fn restore(ctx: &Ctx, args: TrashArgs) -> Result<()> {
    let (t, reindex, err) = ops::restore(ctx, &args.path, false).await?;
    if ctx.json {
        emit_json(
            &json!({ "path": t.path, "restoredFrom": t.trashed_to, "reindex": reindex, "reindexError": err }),
        )?;
    } else {
        println!("{}  <-  {}", t.path, t.trashed_to);
    }
    if let Some(e) = err {
        eprintln!("lapis: restored, but reindex failed: {e}");
    }
    Ok(())
}

fn template(ctx: &Ctx, cmd: TemplateCommand) -> Result<()> {
    match cmd {
        TemplateCommand::List => {
            let all = templates::list(&ctx.vault.root);
            if ctx.json {
                return emit_json(&all);
            }
            for t in all {
                println!(
                    "{:<24} {}{}",
                    t.id,
                    t.name,
                    if t.target.is_empty() { String::new() } else { format!("  ->  {}", t.target) }
                );
            }
            Ok(())
        }
        TemplateCommand::Show { id } => {
            let t = templates::find(&ctx.vault.root, &id)?;
            if ctx.json {
                return emit_json(
                    &json!({ "id": t.id, "name": t.name, "target": t.target, "builtin": t.builtin, "text": t.text }),
                );
            }
            print!("{}", t.text);
            Ok(())
        }
    }
}

fn trash(ctx: &Ctx, args: TrashArgs) -> Result<()> {
    let t = ops::trash(ctx, &args.path)?;
    if ctx.json {
        return emit_json(&t);
    }
    println!("{}  ->  {}", t.path, t.trashed_to);
    Ok(())
}

// ---------------------------------------------------------------------- tasks

fn task_mark(t: &tasks::Task) -> &'static str {
    match t.status.as_str() {
        "done" => "x",
        "in-progress" => "/",
        "cancelled" => "-",
        "forwarded" => ">",
        _ => " ",
    }
}

fn task_list(ctx: &Ctx, args: TaskListArgs) -> Result<()> {
    let f = tasks::Filter { status: args.status, due: args.due, tag: args.tag, prefix: args.path };
    let list = tasks::list(&ctx.vault.root, &f)?;
    if ctx.json {
        return emit_json(&list);
    }
    if list.is_empty() {
        println!("No tasks match.");
        return Ok(());
    }
    for t in &list {
        let meta = meta_line(&[
            t.due.as_ref().map(|d| format!("due:{d}")),
            t.priority.as_ref().map(|p| format!("!{p}")),
            if t.waiting { Some("@waiting".into()) } else { None },
        ]);
        println!("[{}] {}  {}{meta}", task_mark(t), t.id, t.content);
    }
    Ok(())
}

async fn task_toggle(ctx: &Ctx, args: TaskToggleArgs) -> Result<()> {
    let r = ops::toggle_task(ctx, &args.id, args.no_reindex).await?;
    if ctx.json {
        emit_json(&r)?;
    } else {
        println!("[{}] {}  {}", task_mark(&r.task), r.task.id, r.task.content);
    }
    if let Some(e) = &r.reindex_error {
        eprintln!("lapis: toggled, but reindex failed: {e}");
    }
    Ok(())
}

// ------------------------------------------------------------------ neighbors

async fn neighbors(ctx: &Ctx, args: NeighborsArgs) -> Result<()> {
    // Path escape rules apply even though the lattice, not the disk, answers.
    let rel = notes::clean_rel(&args.path)?;
    let rel = if std::path::Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel };
    let n = ctx.client()?.neighbors(&rel, &args.direction, !args.dangling).await?;
    if ctx.json {
        return emit_json(&n);
    }
    if n.neighbors.is_empty() {
        println!("No {} neighbors for {}.", n.direction, n.path);
        return Ok(());
    }
    for e in &n.neighbors {
        let arrow = if e.direction == "in" { "<-" } else { "->" };
        let flag = if e.resolved { "" } else { "  (dangling)" };
        println!("{arrow} {}{flag}", e.path);
    }
    Ok(())
}
