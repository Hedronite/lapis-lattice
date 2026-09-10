//! `lapis` — one Rust binary: CLI, MCP (`lapis mcp`), Ratatui TUI.
//!
//! Files on disk are the write source of truth. Search, list, and graph
//! currently read an HTTP lattice; an embedded index is the 0.2 path.
//! This binary never writes `lattice.db`.

mod backend;
mod cli;
mod config;
mod envelope;
mod error;
mod hal;
mod lattice;
mod mcp;
mod notes;
mod ops;
mod overlay;
mod resolve;
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

use cli::{AnalyticsArgs, ResolveArgs, TreeArgs};
use cli::{
    AppendArgs, CaptureArgs, Cli, Command, CreateArgs, DailyArgs, ListArgs, NeighborsArgs, ReadArgs,
    ReindexArgs, SearchArgs, TaskCommand, TaskListArgs, TaskToggleArgs, TemplateCommand, TrashArgs,
    VaultCommand,
};
use envelope::Meta;
use error::{LapisError, Result};
use lattice::{ListParams, SearchParams};
use ops::Ctx;

// Multi-thread so the TUI can block its thread while lattice requests run.
/// Process start, for `meta.latency_ms` in every envelope.
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn elapsed_ms() -> f64 {
    START.get().map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0)
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = START.set(std::time::Instant::now());
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
        let v = envelope::err(e, elapsed_ms());
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
    }
    eprintln!("lapis: {e}");
}

async fn run(cli: Cli) -> Result<()> {
    let cfg = config::load()?;
    let vault_flag = cli.global.vault.clone();
    let lattice_flag = cli.global.lattice.clone();
    let lattice_flag_present = lattice_flag.is_some();
    let json = cli.global.json;
    match cli.command() {
        Command::Init(args) => init_vault_cmd(json, args),
        cmd => {
            let vault = vault::resolve(vault_flag.as_deref(), &cfg)?;
            let lattice_url = lattice_flag
                .or_else(|| std::env::var("LAPIS_LATTICE_URL").ok().filter(|s| !s.trim().is_empty()))
                .unwrap_or_else(|| cfg.lattice.url.clone());
            // An explicit --lattice means the operator wants HTTP, whatever config says.
            let force_http = lattice_flag_present;
            let ctx = Ctx { json, vault, cfg, lattice_url, force_http };
            dispatch(ctx, cmd).await
        }
    }
}

async fn dispatch(ctx: Ctx, cmd: Command) -> Result<()> {
    match cmd {
        Command::Init(_) => unreachable!("init is handled before vault resolve"),
        Command::Vault { command: VaultCommand::Info } => vault_info(&ctx).await,
        Command::Search(args) => search(&ctx, args).await,
        Command::Read(args) => read(&ctx, args),
        Command::Neighbors(args) => neighbors(&ctx, args).await,
        Command::Resolve(args) => resolve_link(&ctx, args),
        Command::Analytics(args) => analytics(&ctx, args).await,
        Command::TreeRetrieve(args) => tree_retrieve(&ctx, args).await,
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
        Command::Doctor => doctor(&ctx).await,
        Command::Tui => tui::run(ctx).await,
        Command::Desktop(args) => desktop(&ctx, args),
    }
}

/// `--json` output: always the `{ok, data, error, meta}` envelope.
fn emit_json<T: Serialize>(v: &T) -> Result<()> {
    emit_with(v, Meta::default())
}

fn emit_with<T: Serialize>(v: &T, meta: Meta) -> Result<()> {
    let env = envelope::ok(v, meta.with_latency(elapsed_ms()));
    let mut out = std::io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &env)?;
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
                "Index    {} {}  {}  documents={} links={} dangling={} embedder={}",
                info.lattice.mode,
                info.lattice.url,
                h.status,
                h.documents_indexed,
                h.graph.edges,
                h.graph.dangling_links,
                h.embedder
            ),
            None => println!("Index    {} {}  UNAVAILABLE", info.lattice.mode, info.lattice.url),
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
    let limit = args.limit.max(1);
    let offset = args.offset;
    let requested = offset + limit;
    if requested > 50 {
        return Err(LapisError::Usage(format!("offset + limit must be ≤ 50 (got {requested})")));
    }
    if args.embedder.as_deref() == Some("none") && matches!(args.mode, lattice::Mode::Vector) {
        return Err(LapisError::Usage(
            "vector mode needs an embedder; got --embedder none. Use --mode bm25|hybrid.".into(),
        ));
    }
    let params = SearchParams {
        query: args.query_text(),
        top_k: requested,
        domain: args.domain.clone(),
        mode: args.mode,
        per_doc: args.effective_per_doc(ctx.cfg.agent.per_doc),
        mmr: args.mmr,
        include_archives: args.include_archives,
        embedder: args.embedder.clone(),
    };
    let mut result = ctx.backend()?.search(&params).await?;
    // The lattice has no offset; ask for offset+limit and drop the head. Ranks stay absolute.
    let total = result.hits.len();
    result.hits = result.hits.into_iter().skip(offset as usize).collect();
    result.count = result.hits.len();
    let truncated = total as u32 >= requested && requested < 50;
    if ctx.json {
        let meta = Meta {
            truncated,
            next: if truncated { Some(requested) } else { None },
            latency: Some(result.latency),
            count: Some(result.count),
            limit: Some(limit),
            offset: Some(offset),
            ..Meta::default()
        };
        return emit_with(&result, meta);
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
    let mut note = notes::read(&ctx.vault.root, &args.path)?;
    let max = args.max_chars.or(if ctx.json { ctx.cfg.agent.read_max_chars } else { None });
    let (body, truncated) = notes::excerpt(&note, args.heading.as_deref(), args.chunk, max)?;
    note.body = body;
    if ctx.json {
        if args.meta {
            return emit_json(&json!({
                "path": note.path, "kind": note.kind, "title": note.title,
                "hal": note.hal, "halValid": note.hal_valid, "halError": note.hal_error,
                "tags": note.tags, "size": note.size, "updatedAt": note.updated_at,
            }));
        }
        let meta = Meta { truncated, ..Meta::default() };
        if args.body {
            return emit_with(&json!({ "path": note.path, "body": note.body }), meta);
        }
        return emit_with(&note, meta);
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
        return emit_with(&rows, Meta::page(rows.len(), args.limit, args.offset));
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
    let r = ctx.backend()?.reindex(&rel).await?;
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
        println!("{}{idx}{}", report.written.path, if report.written.dry_run { "  (dry run)" } else { "" });
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
        dry_run: args.dry_run,
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
    let w = write::append_with(&ctx.vault.root, &args.path, &text, &args.guard.guard())?;
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
    let scope = args.scope();
    let summary = tasks::wants_summary(scope.as_deref(), args.full || ctx.cfg.agent.task_unscoped_full());
    let f = tasks::Filter {
        status: args.status,
        due: args.due,
        tag: args.tag,
        prefix: scope,
        exclude: ctx.cfg.agent.task_exclude.clone(),
    };
    let list = tasks::list(&ctx.vault.root, &f)?;
    if summary {
        // Unscoped: the whole vault is hundreds of rows. Counts first; a PATH
        // (or --full) gets the rows.
        let s = tasks::summarize(&list);
        if ctx.json {
            return emit_json(&s);
        }
        println!("{} tasks  (pass a PATH or --full for rows)", s.n);
        for (k, v) in &s.by_status {
            println!("  {k:<12} {v:>5}");
        }
        println!("by folder:");
        for (k, v) in &s.by_folder {
            println!("  {k:<28} {v:>5}");
        }
        return Ok(());
    }
    let (page, meta) = envelope::slice_page(&list, args.limit, args.offset);
    if ctx.json {
        return emit_with(&page, meta);
    }
    if page.is_empty() {
        println!("No tasks match.");
        return Ok(());
    }
    if let Some(next) = meta.next {
        eprintln!("lapis: {} of {} tasks shown; --offset {next} for more", page.len(), list.len());
    }
    for t in &page {
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
    let r = ops::toggle_task_with(ctx, &args.id, args.no_reindex, &args.guard.guard()).await?;
    if ctx.json {
        emit_json(&r)?;
    } else {
        println!(
            "[{}] {}  {}{}",
            task_mark(&r.task),
            r.task.id,
            r.task.content,
            if r.dry_run { "  (dry run)" } else { "" }
        );
    }
    if let Some(e) = &r.reindex_error {
        eprintln!("lapis: toggled, but reindex failed: {e}");
    }
    Ok(())
}

// ---------------------------------------------------------- analytics / tree

async fn analytics(ctx: &Ctx, args: AnalyticsArgs) -> Result<()> {
    let a = ctx.backend()?.analytics(&args.query).await?;
    if ctx.json {
        let meta = Meta { truncated: a.truncated, count: Some(a.count), ..Meta::default() };
        return emit_with(&a, meta);
    }
    for (k, v) in &a.extra {
        println!("{k}: {v}");
    }
    if !a.columns.is_empty() {
        println!("{}", a.columns.join("\t"));
    }
    for row in &a.rows {
        let cells: Vec<String> = row
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            })
            .collect();
        println!("{}", cells.join("\t"));
    }
    if a.truncated {
        eprintln!("lapis: {} rows shown (truncated)", a.count);
    }
    Ok(())
}

async fn tree_retrieve(ctx: &Ctx, args: TreeArgs) -> Result<()> {
    let rel = match &args.path {
        Some(p) => {
            let r = notes::clean_rel(p)?;
            Some(if std::path::Path::new(&r).extension().is_none() { format!("{r}.md") } else { r })
        }
        None => None,
    };
    let t = ctx.backend()?.tree(rel.as_deref(), args.query.as_deref(), args.depth, args.max_nodes).await?;
    if ctx.json {
        let meta = Meta { truncated: t.truncated, count: Some(t.count), ..Meta::default() };
        return emit_with(&t, meta);
    }
    println!("seed: {}{}", t.seed, if t.ranked { "  (ranked by query)" } else { "" });
    for n in &t.nodes {
        let hub = if n.is_hub { " ◆" } else { "" };
        let score = n.score.map(|s| format!("  {s:.3}")).unwrap_or_default();
        println!("{}{}{hub}{score}", "  ".repeat(n.depth as usize), n.path);
    }
    if t.truncated {
        eprintln!("lapis: tree truncated at {} nodes (--max-nodes)", t.count);
    }
    Ok(())
}

fn init_vault_cmd(json: bool, args: cli::InitArgs) -> Result<()> {
    let w = write::init_vault(args.path.as_deref())?;
    // B2: a vault with no index is a vault whose first search fails. Build it
    // here so `init` then `search` works with no daemon and no second command.
    let indexed = lapis_lattice::Engine::open(&w.path).ok().and_then(|mut e| e.reindex().ok());
    if json {
        return emit_json(&json!({
            "path": w.path,
            "created": w.created,
            "indexed": indexed.as_ref().map(|r| json!({
                "documents": r.documents, "chunks": r.chunks, "edges": r.edges
            })),
        }));
    }
    let verb = if w.created { "created" } else { "already exists" };
    println!("vault {verb}: {}", w.path);
    match &indexed {
        Some(r) => println!("indexed: {} documents, {} chunks, {} links", r.documents, r.chunks, r.edges),
        None => println!("indexed: skipped (could not open the index)"),
    }
    println!("next: lapis --vault {}   or   export LAPIS_VAULT={}", w.path, w.path);
    Ok(())
}

// --------------------------------------------------------------------- doctor

/// `lapis doctor`: is this install usable? Per-check rows so CI can assert one
/// thing (`schema/v0.2/doctor.schema.json`), plus notes for a human. A missing
/// embedder is a warning, never a failure — BM25 still answers.
async fn doctor(ctx: &Ctx) -> Result<()> {
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
    check!("backend", "ok", ctx.cfg.lattice.mode.clone());

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
    let probe = ctx
        .backend()?
        .search(&SearchParams {
            query: "lapis".into(),
            top_k: 1,
            domain: None,
            mode: lattice::Mode::Bm25,
            per_doc: true,
            mmr: false,
            include_archives: false,
            embedder: Some("none".into()),
        })
        .await;
    match &probe {
        Ok(r) => check!("search", "ok", format!("{} hit(s) for a probe query", r.count)),
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
        emit_json(&report)?;
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

// -------------------------------------------------------------------- desktop

/// `lapis desktop`: the GPUI shell (N23). Built without the `desktop` feature
/// this reports exactly that instead of pretending.
fn desktop(ctx: &Ctx, args: cli::DesktopArgs) -> Result<()> {
    let opts = lapis_desktop::Options {
        vault_root: ctx.vault.root.clone(),
        lattice_url: ctx.lattice_url.clone(),
        seed: args.path,
        title: "Lapis".into(),
    };
    if args.check {
        let report = lapis_desktop::preflight(&opts);
        if ctx.json {
            return emit_json(&report);
        }
        println!(
            "desktop: gpui={} sidecar={}",
            report.gpui_available,
            report.gitnexus.as_deref().unwrap_or("none")
        );
        return Ok(());
    }
    lapis_desktop::run(opts).map_err(|e| match e {
        lapis_desktop::DesktopError::NotBuilt(m) => LapisError::Usage(m),
        lapis_desktop::DesktopError::Runtime(m) => LapisError::Internal(m),
    })
}

// -------------------------------------------------------------------- resolve

fn resolve_link(ctx: &Ctx, args: ResolveArgs) -> Result<()> {
    let r = resolve::resolve(&ctx.vault.root, &args.link)?;
    if ctx.json {
        return emit_json(&r);
    }
    match (&r.path, r.how) {
        (Some(p), how) => println!("{p}  ({how})"),
        (None, "collision") => {
            println!("collision: {}", r.candidates.join(", "));
        }
        (None, _) => println!("dangling: {}", r.target),
    }
    Ok(())
}

// ------------------------------------------------------------------ neighbors

async fn neighbors(ctx: &Ctx, args: NeighborsArgs) -> Result<()> {
    // Path escape rules apply even though the lattice, not the disk, answers.
    let rel = notes::clean_rel(&args.path)?;
    let rel = if std::path::Path::new(&rel).extension().is_none() { format!("{rel}.md") } else { rel };
    let dir = args.direction.clone().unwrap_or_else(|| ctx.cfg.agent.direction().to_string());
    if args.hop == 2 {
        let e = ctx.backend()?.ego(&rel, 2, &dir, !args.dangling).await?;
        if ctx.json {
            let meta = Meta { truncated: e.truncated, count: Some(e.count), ..Meta::default() };
            return emit_with(&e, meta);
        }
        if e.rows.is_empty() {
            println!("No {} neighbors within 2 hops of {}.", e.direction, e.path);
            return Ok(());
        }
        for r in &e.rows {
            let arrow = if r.direction == "in" { "<-" } else { "->" };
            let shown = r.path.clone().or_else(|| r.dst_raw.clone()).unwrap_or_else(|| "?".into());
            let flag = if r.resolved { "" } else { "  (dangling)" };
            let via = if r.depth > 1 {
                format!("  via {}", r.via.clone().unwrap_or_default())
            } else {
                String::new()
            };
            println!("{}{arrow} {shown}{flag}{via}", "  ".repeat(r.depth as usize - 1));
        }
        if e.truncated {
            eprintln!("lapis: ego graph truncated at {} rows", e.count);
        }
        return Ok(());
    }
    let n = ctx.backend()?.neighbors(&rel, &dir, !args.dangling).await?;
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
        println!("{arrow} {}{flag}", e.label());
    }
    Ok(())
}
