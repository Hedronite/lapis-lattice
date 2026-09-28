//! `<index>/<sha256>.tree.json` and `<sha256>.pages.jsonl`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::error::{Result, TomeError, io, parse};
use crate::pdf::{self, LoadedPdf};
use crate::types::{
    BUILDER_VERSION, Budget, BuiltTree, DocId, DocMeta, Node, NodeId, NodeSource, OPEN_BYTE_CAP,
    OPEN_PAGE_CAP, Passage, RawNode, SummaryModel, Walk,
};
use crate::walk::Judge;

/// Options for [`TomeIndex::build`]. The summary model is whatever config supplied.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub allow_windows: bool,
    pub force: bool,
    /// When set, the llm-struct stub runs after heading detection fails.
    /// The stub refuses to invent a tree.
    pub llm_struct: bool,
    pub summary_model: SummaryModel,
    /// Title used for an explicit window tree. The PDF file stem is typical.
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TreeFile {
    meta: DocMeta,
    nodes: Vec<Node>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PageRec {
    page: u32,
    text: String,
}

/// mtime + length of a PDF whose bytes already matched `sha256`.
#[derive(Debug, Clone)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    sha256: String,
}

/// On-disk tome index. `open` takes the directory (`<vault>/.lapis/tomes`).
#[derive(Debug, Clone)]
pub struct TomeIndex {
    dir: PathBuf,
    vault_root: PathBuf,
    /// Skip re-hashing a PDF when path, mtime, and size are unchanged.
    stamps: Arc<Mutex<HashMap<PathBuf, Stamp>>>,
}

impl TomeIndex {
    pub fn open(index_dir: &Path) -> Result<Self> {
        fs::create_dir_all(index_dir).map_err(|e| io(format!("index dir: {e}")))?;
        Ok(Self {
            dir: index_dir.to_path_buf(),
            vault_root: vault_root_for(index_dir),
            stamps: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn docs(&self) -> Result<Vec<DocMeta>> {
        let mut out = Vec::new();
        let entries = fs::read_dir(&self.dir).map_err(|e| io(format!("index dir: {e}")))?;
        for entry in entries {
            let entry = entry.map_err(|e| io(format!("index dir: {e}")))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".tree.json") {
                continue;
            }
            let text = fs::read_to_string(entry.path()).map_err(io)?;
            let file: TreeFile = serde_json::from_str(&text).map_err(|e| parse(format!("{name}: {e}")))?;
            out.push(file.meta);
        }
        out.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
        Ok(out)
    }

    /// Stored meta. Refuses a stale tree, same as [`Self::tree`].
    pub fn meta(&self, doc: &DocId) -> Result<DocMeta> {
        Ok(self.load(doc)?.meta)
    }

    pub fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>> {
        let built = self.load(doc)?;
        let selected = if let Some(id) = node {
            let found = find(&built.nodes, id)
                .ok_or_else(|| TomeError::UnknownNode { doc: doc.to_string(), node: id.to_string() })?;
            vec![found.clone()]
        } else {
            built.nodes
        };
        Ok(selected.iter().map(|n| cut(n, depth)).collect())
    }

    /// Passage read. Same as [`crate::OpenPassages::open`], without importing the trait.
    pub fn passages(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        self.read_passages(doc, nodes)
    }

    pub(crate) fn read_passages(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        let built = self.load(doc)?;
        open_loaded(doc, &built.nodes, &built.pages, nodes, OPEN_PAGE_CAP, OPEN_BYTE_CAP)
    }

    pub fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk> {
        let built = self.load(doc)?;
        if built.nodes.is_empty() {
            return Err(TomeError::NoStructure {
                doc: doc.to_string(),
                detail: "stored tree is empty".into(),
            });
        }
        let pages = &built.pages;
        let choice =
            crate::walk::choose(
                &built.nodes,
                query,
                judge,
                budget,
                |page| Ok(page_text(pages, page)?.len()),
            )?;
        let page_cap = budget.max_pages.min(OPEN_PAGE_CAP);
        let passages = open_loaded(doc, &built.nodes, pages, &choice.ids, page_cap, OPEN_BYTE_CAP)?;
        Ok(Walk {
            doc_id: doc.clone(),
            query: query.to_string(),
            nodes: choice.ids,
            passages,
            judge_calls: choice.calls,
            skipped: choice.skipped,
            judged: choice.judged,
            root_judge_calls: choice.root_calls,
            root_path: choice.root_path,
            roots_skipped: choice.roots_skipped,
        })
    }

    pub fn build(&self, pdf: &Path, vault_path: &str, opts: &BuildOptions) -> Result<DocMeta> {
        opts.summary_model.check()?;
        let loaded = pdf::load(pdf)?;
        let id = DocId::from_verified(loaded.sha256.clone());
        if !opts.force
            && let Some(meta) = self.fresh_cached(&id)?
        {
            return Ok(meta);
        }
        let (mut raw, source, had_outline) = structure(&loaded, opts, id.as_str())?;
        let page_count = loaded.pages.len() as u32;
        crate::outline::assign_ends(&mut raw, page_count);
        crate::split::split_all(&mut raw, &loaded.pages)?;
        if raw.is_empty() {
            return Err(TomeError::NoStructure {
                doc: id.to_string(),
                detail: "refusing to store an empty tree".into(),
            });
        }
        let nodes = finalize(&raw, &loaded.pages, "", 1)?;
        if nodes.is_empty() {
            return Err(TomeError::NoStructure {
                doc: id.to_string(),
                detail: "refusing to store an empty tree".into(),
            });
        }
        let meta = DocMeta {
            doc_id: id.clone(),
            path: vault_path.to_string(),
            sha256: loaded.sha256.clone(),
            pages: page_count,
            outline: had_outline,
            source,
            built_at: pdf::now_rfc3339(),
            builder_version: BUILDER_VERSION.to_string(),
            summary_model: opts.summary_model.id(),
            summary_temperature: opts.summary_model.temperature,
        };
        self.write(&meta, &nodes, &loaded.pages)?;
        Ok(meta)
    }

    fn fresh_cached(&self, id: &DocId) -> Result<Option<DocMeta>> {
        let path = self.tree_path(id)?;
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).map_err(io)?;
        let file: TreeFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(_) => return Ok(None),
        };
        if file.meta.builder_version == BUILDER_VERSION
            && file.meta.sha256 == id.as_str()
            && !file.nodes.is_empty()
        {
            return Ok(Some(file.meta));
        }
        Ok(None)
    }

    fn load(&self, doc: &DocId) -> Result<Loaded> {
        let built = self.read_stored(doc)?;
        self.ensure_fresh(&built.meta)?;
        if built.nodes.is_empty() {
            return Err(TomeError::NoStructure {
                doc: doc.to_string(),
                detail: "stored tree is empty".into(),
            });
        }
        let pages = self.read_pages(doc)?;
        Ok(Loaded { meta: built.meta, nodes: built.nodes, pages })
    }

    fn read_stored(&self, doc: &DocId) -> Result<BuiltTree> {
        let path = self.tree_path(doc)?;
        if !path.is_file() {
            return Err(TomeError::UnknownDoc { doc: doc.to_string() });
        }
        let text = fs::read_to_string(&path).map_err(io)?;
        let file: TreeFile =
            serde_json::from_str(&text).map_err(|e| parse(format!("{}: {e}", path.display())))?;
        Ok(BuiltTree { meta: file.meta, nodes: file.nodes })
    }

    fn ensure_fresh(&self, meta: &DocMeta) -> Result<()> {
        if meta.builder_version != BUILDER_VERSION {
            return Err(TomeError::Stale {
                doc: meta.doc_id.to_string(),
                detail: format!("builder {} != {BUILDER_VERSION}", meta.builder_version),
            });
        }
        if meta.sha256 != meta.doc_id.as_str() {
            return Err(TomeError::Stale {
                doc: meta.doc_id.to_string(),
                detail: "doc id does not match sha256".into(),
            });
        }
        if let Some(path) = self.pdf_on_disk(meta) {
            let file_meta = fs::metadata(&path).map_err(|e| io(format!("{}: {e}", path.display())))?;
            let modified = file_meta.modified().ok();
            let len = file_meta.len();
            if self.stamp_matches(&path, modified, len, &meta.sha256) {
                return Ok(());
            }
            let hash = pdf::sha256_file(&path)?;
            if hash != meta.sha256 {
                return Err(TomeError::Stale {
                    doc: meta.doc_id.to_string(),
                    detail: "pdf bytes changed".into(),
                });
            }
            self.remember_stamp(path, modified, len, hash);
        }
        Ok(())
    }

    fn stamp_matches(&self, path: &Path, modified: Option<SystemTime>, len: u64, sha256: &str) -> bool {
        let Ok(guard) = self.stamps.lock() else {
            return false;
        };
        guard
            .get(path)
            .is_some_and(|stamp| stamp.modified == modified && stamp.len == len && stamp.sha256 == sha256)
    }

    fn remember_stamp(&self, path: PathBuf, modified: Option<SystemTime>, len: u64, sha256: String) {
        if let Ok(mut guard) = self.stamps.lock() {
            guard.insert(path, Stamp { modified, len, sha256 });
        }
    }

    fn pdf_on_disk(&self, meta: &DocMeta) -> Option<PathBuf> {
        let p = PathBuf::from(&meta.path);
        let full = if p.is_absolute() { p } else { self.vault_root.join(p) };
        full.is_file().then_some(full)
    }

    fn read_pages(&self, doc: &DocId) -> Result<BTreeMap<u32, String>> {
        let path = self.pages_path(doc)?;
        let text = fs::read_to_string(&path).map_err(|e| io(format!("{}: {e}", path.display())))?;
        let mut pages = BTreeMap::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let rec: PageRec = serde_json::from_str(line)
                .map_err(|e| parse(format!("{}:{}: {e}", path.display(), i + 1)))?;
            pages.insert(rec.page, rec.text);
        }
        Ok(pages)
    }

    fn write(&self, meta: &DocMeta, nodes: &[Node], pages: &[String]) -> Result<()> {
        let mut body = String::new();
        for (i, text) in pages.iter().enumerate() {
            let rec = PageRec { page: (i as u32) + 1, text: text.clone() };
            body.push_str(&serde_json::to_string(&rec).map_err(parse)?);
            body.push('\n');
        }
        // Pages first, so a tree file is never visible without its text.
        write_atomic(&self.pages_path(&meta.doc_id)?, body.as_bytes())?;
        let file = TreeFile { meta: meta.clone(), nodes: nodes.to_vec() };
        let json = serde_json::to_vec_pretty(&file).map_err(parse)?;
        write_atomic(&self.tree_path(&meta.doc_id)?, &json)?;
        Ok(())
    }

    fn tree_path(&self, doc: &DocId) -> Result<PathBuf> {
        Ok(self.dir.join(format!("{}.tree.json", doc.checked()?)))
    }

    fn pages_path(&self, doc: &DocId) -> Result<PathBuf> {
        Ok(self.dir.join(format!("{}.pages.jsonl", doc.checked()?)))
    }
}

struct Loaded {
    meta: DocMeta,
    nodes: Vec<Node>,
    pages: BTreeMap<u32, String>,
}

fn structure(
    loaded: &LoadedPdf,
    opts: &BuildOptions,
    label: &str,
) -> Result<(Vec<RawNode>, NodeSource, bool)> {
    if let Some(nodes) = crate::outline::extract(&loaded.doc)? {
        let cleaned = crate::outline::drop_front_matter(nodes);
        if !cleaned.is_empty() {
            return Ok((cleaned, NodeSource::Outline, true));
        }
    }
    let headings = crate::headings::detect(&loaded.pages, 1, loaded.pages.len() as u32);
    if !headings.is_empty() {
        return Ok((headings, NodeSource::Heading, false));
    }
    if opts.llm_struct {
        match crate::llm::stub(label) {
            Ok(nodes) if !nodes.is_empty() => return Ok((nodes, NodeSource::Llm, false)),
            Ok(_) => {}
            Err(_) if opts.allow_windows => {}
            Err(err) => return Err(err),
        }
    }
    if opts.allow_windows {
        let title = if opts.title.trim().is_empty() { "Pages" } else { opts.title.trim() };
        let nodes = crate::split::window_tree(loaded.pages.len() as u32, title);
        if nodes.is_empty() {
            return Err(TomeError::NoStructure { doc: label.into(), detail: "window tree was empty".into() });
        }
        return Ok((nodes, NodeSource::Window, false));
    }
    Err(TomeError::NoStructure {
        doc: label.into(),
        detail: "no outline and heading detection found nothing; pass --allow-windows for an explicit page-window tree"
            .into(),
    })
}

fn finalize(nodes: &[RawNode], pages: &[String], prefix: &str, depth: u8) -> Result<Vec<Node>> {
    if depth > 64 {
        return Err(parse("tree deeper than 64"));
    }
    let mut out = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        let local = format!("{:04}", i + 1);
        let id = NodeId(if prefix.is_empty() { local } else { format!("{prefix}.{local}") });
        let children = finalize(&node.children, pages, id.as_str(), depth.saturating_add(1))?;
        let lead = pdf::lead_text(pages, node.page_start, node.page_end);
        out.push(Node {
            id,
            title: node.title.clone(),
            level: depth,
            page_start: node.page_start,
            page_end: node.page_end,
            lead: lead.clone(),
            summary: lead,
            source: node.source,
            child_count: children.len(),
            children,
        });
    }
    Ok(out)
}

fn cut(node: &Node, depth: Option<u8>) -> Node {
    cut_at(node, depth, 0)
}

fn cut_at(node: &Node, depth: Option<u8>, hops: u8) -> Node {
    let mut out = node.clone();
    if hops >= 64 {
        out.children.clear();
        return out;
    }
    match depth {
        None => {
            out.children = node.children.iter().map(|child| cut_at(child, None, hops + 1)).collect();
        }
        Some(0) => out.children.clear(),
        Some(d) => {
            out.children = node.children.iter().map(|child| cut_at(child, Some(d - 1), hops + 1)).collect();
        }
    }
    out.child_count = node.children.len();
    out
}

fn find<'a>(nodes: &'a [Node], id: &NodeId) -> Option<&'a Node> {
    let mut stack: Vec<&Node> = nodes.iter().rev().collect();
    let mut guard = 0u32;
    while let Some(node) = stack.pop() {
        guard += 1;
        if guard > 100_000 {
            return None;
        }
        if &node.id == id {
            return Some(node);
        }
        stack.extend(node.children.iter().rev());
    }
    None
}

fn open_loaded(
    doc: &DocId,
    tree: &[Node],
    pages: &BTreeMap<u32, String>,
    nodes: &[NodeId],
    page_cap: u32,
    byte_cap: usize,
) -> Result<Vec<Passage>> {
    let mut resolved = Vec::with_capacity(nodes.len());
    for id in nodes {
        let node = find(tree, id)
            .ok_or_else(|| TomeError::UnknownNode { doc: doc.to_string(), node: id.to_string() })?;
        resolved.push(node);
    }
    let mut out = Vec::new();
    let mut bytes = 0usize;
    for node in resolved {
        if node.page_end < node.page_start {
            continue;
        }
        for page in node.page_start..=node.page_end {
            let text = page_text(pages, page)?.to_string();
            let next_bytes = bytes.saturating_add(text.len());
            if u32::try_from(out.len()).unwrap_or(u32::MAX) + 1 > page_cap || next_bytes > byte_cap {
                return Err(TomeError::OverBudget {
                    detail: format!("cap is {page_cap} pages / {byte_cap} bytes"),
                });
            }
            bytes = next_bytes;
            out.push(Passage { node_id: node.id.clone(), page, text, truncated: false });
        }
    }
    Ok(out)
}

fn page_text(pages: &BTreeMap<u32, String>, page: u32) -> Result<&str> {
    pages
        .get(&page)
        .map(String::as_str)
        .ok_or_else(|| TomeError::Parse(format!("page {page} is missing from the page store")))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("partial");
    fs::write(&tmp, bytes).map_err(|e| io(format!("{}: {e}", tmp.display())))?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io(format!("{}: {e}", path.display()))
    })
}

fn vault_root_for(index_dir: &Path) -> PathBuf {
    let name = index_dir.file_name().and_then(|s| s.to_str());
    let parent_name = index_dir.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str());
    if name == Some("tomes") && parent_name == Some(".lapis") {
        index_dir
            .parent()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| index_dir.to_path_buf())
    } else {
        index_dir.to_path_buf()
    }
}
