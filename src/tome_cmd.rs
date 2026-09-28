//! `lapis tome`. Compiled only with `--features tome`.

use std::path::{Path, PathBuf};

use tome_tree::{
    Budget, BuildOptions, DocId, JudgeConfidence, Node, NodeId, OpenPassages, SummaryModel, TomeIndex,
};

use crate::error::{LapisError, Result};
use crate::jev::JevJudge;
use crate::ops::Ctx;
use crate::tome_args::{TomeBuildArgs, TomeCommand, TomeOpenArgs, TomeSearchArgs, TomeTreeArgs};

pub fn run(ctx: &Ctx, command: TomeCommand) -> Result<()> {
    match command {
        TomeCommand::Build(args) => build(ctx, args),
        TomeCommand::Tree(args) => tree(ctx, args),
        TomeCommand::Search(args) => search(ctx, args),
        TomeCommand::Open(args) => open(ctx, args),
    }
}

fn index(ctx: &Ctx) -> Result<TomeIndex> {
    TomeIndex::open(&ctx.vault.root.join(".lapis").join("tomes")).map_err(LapisError::from)
}

fn judge_policy(ctx: &Ctx) -> Result<JudgeConfidence> {
    let min = ctx.cfg.tome.judge_min_confidence;
    match ctx.cfg.tome.judge_confidence.as_str() {
        "fail_closed" => Ok(JudgeConfidence::FailClosed { min }),
        "down_weight" => Ok(JudgeConfidence::DownWeight { min }),
        other => Err(LapisError::Usage(format!(
            "judge_confidence must be fail_closed or down_weight, got {other}"
        ))),
    }
}

fn summary_model(ctx: &Ctx) -> Result<SummaryModel> {
    let tome = &ctx.cfg.tome;
    if tome.provider.trim().is_empty() || tome.model.trim().is_empty() {
        return Err(LapisError::Usage(
            "tome model is empty; set [tome] provider and model in the config".into(),
        ));
    }
    Ok(SummaryModel {
        provider: tome.provider.clone(),
        model: tome.model.clone(),
        temperature: tome.temperature,
    })
}

fn build(ctx: &Ctx, args: TomeBuildArgs) -> Result<()> {
    if args.pdfs.is_empty() {
        return Err(LapisError::Usage("tome build needs at least one pdf".into()));
    }
    let model = summary_model(ctx)?;
    let index = index(ctx)?;
    let mut metas = Vec::with_capacity(args.pdfs.len());
    for pdf in &args.pdfs {
        let path = resolve_pdf(&ctx.vault.root, pdf);
        if !path.is_file() {
            return Err(LapisError::Path(format!("pdf not found: {pdf}")));
        }
        let title = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Pages").to_string();
        let opts = BuildOptions {
            allow_windows: args.allow_windows,
            force: args.force,
            llm_struct: args.llm_struct,
            summary_model: model.clone(),
            title,
        };
        let rel = vault_rel(&ctx.vault.root, &path);
        metas.push(index.build(&path, &rel, &opts).map_err(LapisError::from)?);
    }
    if ctx.json {
        crate::emit_json(&metas)?;
    } else {
        for meta in &metas {
            println!(
                "{}  {}  pages={}  source={}  outline={}",
                meta.doc_id, meta.path, meta.pages, meta.source, meta.outline
            );
        }
    }
    Ok(())
}

fn tree(ctx: &Ctx, args: TomeTreeArgs) -> Result<()> {
    let index = index(ctx)?;
    let doc = resolve_doc(&index, &ctx.vault.root, &args.doc)?;
    let node = args.node.as_deref().map(NodeId::from);
    let nodes = index.tree(&doc, node.as_ref(), args.depth).map_err(LapisError::from)?;
    let meta = index.meta(&doc).map_err(LapisError::from)?;
    if ctx.json {
        crate::emit_json(&serde_json::json!({ "doc": meta, "nodes": nodes }))?;
    } else {
        println!("{}  {}  pages={}", meta.doc_id, meta.path, meta.pages);
        print_nodes(&nodes, 0);
    }
    Ok(())
}

fn search(ctx: &Ctx, args: TomeSearchArgs) -> Result<()> {
    let index = index(ctx)?;
    let doc = resolve_doc(&index, &ctx.vault.root, &args.doc)?;
    let budget = Budget { max_judge_calls: args.max_judge_calls, max_pages: args.max_pages };
    let walked = index
        .walk(&doc, &args.query, &JevJudge::with_policy(judge_policy(ctx)?), budget)
        .map_err(LapisError::from)?;
    if ctx.json {
        crate::emit_json(&walked)?;
    } else {
        println!("nodes: {}", walked.nodes.iter().map(|id| id.as_str()).collect::<Vec<_>>().join(" "));
        println!("judge_calls: {}", walked.judge_calls);
        print_passages(&walked.passages);
    }
    Ok(())
}

fn open(ctx: &Ctx, args: TomeOpenArgs) -> Result<()> {
    if args.nodes.is_empty() {
        return Err(LapisError::Usage("tome open needs at least one node id".into()));
    }
    let index = index(ctx)?;
    let doc = resolve_doc(&index, &ctx.vault.root, &args.doc)?;
    let nodes: Vec<NodeId> = args.nodes.iter().map(|id| NodeId::from(id.as_str())).collect();
    let passages = OpenPassages::open(&index, &doc, &nodes).map_err(LapisError::from)?;
    if ctx.json {
        crate::emit_json(&serde_json::json!({ "doc_id": doc, "passages": passages }))?;
    } else {
        print_passages(&passages);
    }
    Ok(())
}

fn print_nodes(nodes: &[Node], indent: usize) {
    for node in nodes {
        println!(
            "{:indent$}{}  {}  pp. {}–{}  [{}]  children={}",
            "",
            node.id,
            node.title,
            node.page_start,
            node.page_end,
            node.source,
            node.child_count,
            indent = indent * 2
        );
        print_nodes(&node.children, indent + 1);
    }
}

fn print_passages(passages: &[tome_tree::Passage]) {
    for passage in passages {
        println!("--- {} p.{} ---", passage.node_id, passage.page);
        println!("{}", passage.text);
    }
}

fn resolve_doc(index: &TomeIndex, vault: &Path, arg: &str) -> Result<DocId> {
    let trimmed = arg.trim();
    if let Ok(id) = DocId::try_from(trimmed) {
        return Ok(id);
    }
    if let Ok(docs) = index.docs()
        && let Some(meta) = docs.into_iter().find(|meta| meta.path == trimmed)
    {
        return Ok(meta.doc_id);
    }
    let path = resolve_pdf(vault, trimmed);
    if path.is_file() {
        return tome_tree::content_id(&path).map_err(LapisError::from);
    }
    Err(LapisError::Path(format!("no tome doc or pdf: {trimmed}")))
}

fn resolve_pdf(vault: &Path, arg: &str) -> PathBuf {
    let path = PathBuf::from(arg);
    if path.is_absolute() {
        return path;
    }
    let in_vault = vault.join(&path);
    if in_vault.is_file() {
        return in_vault;
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(path)
}

fn vault_rel(vault: &Path, abs: &Path) -> String {
    abs.strip_prefix(vault).unwrap_or(abs).to_string_lossy().replace('\\', "/")
}

impl From<tome_tree::TomeError> for LapisError {
    fn from(err: tome_tree::TomeError) -> Self {
        LapisError::Tome { code: err.code(), message: err.to_string() }
    }
}
