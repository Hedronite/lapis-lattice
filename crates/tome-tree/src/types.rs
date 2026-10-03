//! Public tree types. Field names and JSON spellings are frozen for MCP.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Result, TomeError};

/// Lowercase hex SHA-256 of the PDF bytes. This is the doc id.
///
/// The inner string is private. [`DocId::parse`] accepts 64 hex characters
/// (case-insensitive) and stores them lowercase. Anything else, including
/// `../`, is rejected so the id can be used as a single file name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DocId(String);

impl DocId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 64 hex characters. Uppercase is folded. Path characters are rejected.
    pub fn parse(raw: &str) -> Result<Self> {
        let s = raw.trim().to_ascii_lowercase();
        if s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) {
            Ok(DocId(s))
        } else {
            let shown: String = raw.chars().take(80).collect();
            Err(TomeError::Parse(format!("doc id must be 64 hex characters, got {shown}")))
        }
    }

    /// `sha256` from this crate's hasher. Debug-checked, not a user parser.
    pub(crate) fn from_verified(hex: String) -> Self {
        debug_assert!(
            hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "from_verified got {hex}"
        );
        DocId(hex)
    }

    pub(crate) fn checked(&self) -> Result<&str> {
        if self.0.len() == 64 && self.0.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            Ok(&self.0)
        } else {
            Err(TomeError::Parse("doc id must be 64 lowercase hex".into()))
        }
    }
}

impl<'de> Deserialize<'de> for DocId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        DocId::parse(&s).map_err(D::Error::custom)
    }
}

impl TryFrom<&str> for DocId {
    type Error = TomeError;

    fn try_from(s: &str) -> Result<Self> {
        DocId::parse(s)
    }
}

impl TryFrom<String> for DocId {
    type Error = TomeError;

    fn try_from(s: String) -> Result<Self> {
        DocId::parse(&s)
    }
}

impl std::fmt::Display for DocId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Dotted sibling path, 1-based, four digits per level: `0003.0002`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        NodeId(s.trim().to_string())
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        NodeId(s)
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a node's bounds came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeSource {
    Outline,
    Heading,
    Llm,
    Window,
}

impl std::fmt::Display for NodeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NodeSource::Outline => "outline",
            NodeSource::Heading => "heading",
            NodeSource::Llm => "llm",
            NodeSource::Window => "window",
        })
    }
}

/// One TOC node. `children` may be cut by `depth`; `child_count` is the full count.
///
/// Pages are 1-based physical PDF pages, inclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub title: String,
    /// Depth in the tree. Roots are 1.
    pub level: u8,
    pub page_start: u32,
    pub page_end: u32,
    /// First ~400 characters of the node's page text.
    pub lead: String,
    /// v0: the same text as `lead`. A later summary model may replace it.
    pub summary: String,
    pub source: NodeSource,
    pub child_count: usize,
    pub children: Vec<Node>,
}

/// One built document, as stored beside the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocMeta {
    pub doc_id: DocId,
    /// Vault-relative path when the PDF lives in the vault, otherwise the path
    /// that was passed to `build`.
    pub path: String,
    pub sha256: String,
    pub pages: u32,
    /// True when the PDF had an outline that survived front-matter cleanup.
    pub outline: bool,
    pub source: NodeSource,
    /// RFC3339 UTC, second precision.
    pub built_at: String,
    pub builder_version: String,
    /// `{provider}/{model}` recorded from config. v0 does not call it.
    #[serde(default)]
    pub summary_model: String,
    #[serde(default)]
    pub summary_temperature: f32,
}

/// One physical page of one node. `truncated` is always false: over-budget calls error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    pub node_id: NodeId,
    pub page: u32,
    pub text: String,
    pub truncated: bool,
}

/// Answer and summary model. The crate stores this; it does not pick a model.
///
/// Jev is the judge and is not configured here.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryModel {
    pub provider: String,
    pub model: String,
    pub temperature: f32,
}

impl SummaryModel {
    pub fn id(&self) -> String {
        format!("{}/{}", self.provider.trim(), self.model.trim())
    }

    pub(crate) fn check(&self) -> Result<()> {
        if self.provider.trim().is_empty() || self.model.trim().is_empty() {
            return Err(TomeError::Parse(
                "summary model is empty; set [tome] provider and model in config".into(),
            ));
        }
        Ok(())
    }
}

/// Hard caps for [`crate::TomeIndex::open`] (the passage read).
pub const OPEN_PAGE_CAP: u32 = 12;
pub const OPEN_BYTE_CAP: usize = 48 * 1024;

/// Beam width for [`crate::TomeIndex::walk`].
pub const BEAM: usize = 2;

/// Walk stops descending at a leaf or a node that spans this many pages.
pub const STOP_PAGES: u32 = 3;

pub const LEAD_CHARS: usize = 400;

/// A leaf longer than this is split.
pub const SPLIT_PAGES: u32 = 10;
/// Token estimate is `chars / 4`. A leaf is also split when its bytes exceed
/// [`OPEN_BYTE_CAP`], so one node can still be opened.
pub const SPLIT_TOKENS: usize = 20_000;

/// Descent budget. The root pass does not spend these calls.
pub const DEFAULT_JUDGE_CALLS: u32 = 24;

/// Root-pass call budget kept on [`Budget`] for callers. The walk does not
/// stop at this value: it scores every root up to [`ROOT_SCORE_CAP`].
pub const DEFAULT_ROOT_CALLS: u32 = 4;

/// Most roots the root pass will score. At the default batch size of 16 that
/// is 32 batches. Later roots are listed on [`Walk::roots_skipped`]. The
/// descent budget stays [`DEFAULT_JUDGE_CALLS`].
pub const ROOT_SCORE_CAP: usize = 512;

/// Candidates in one batched judge call.
pub const DEFAULT_ROOT_BATCH: u32 = 16;

/// One-at-a-time judgments after a malformed batch, chosen by lexical overlap.
pub const DEFAULT_ROOT_TOP_K: u32 = 6;

/// Kept so older imports still compile. Frontiers are batched instead of truncated.
pub const DESCENT_RESERVE: u32 = 8;

/// Builder stamp stored on every tree. A mismatch is `stale`.
pub const BUILDER_VERSION: &str = "0.5.0";

/// One judge result. `score` is the model's 0..=3. `confidence` is recorded
/// and is not a gate: a low value does not drop the candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    pub score: u8,
    pub confidence: Option<f64>,
}

/// Which root-pass path produced the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RootPath {
    /// Batches of [`Budget::root_batch_size`] returned a score for every candidate.
    #[default]
    Batch,
    /// A batch was malformed. Roots were pre-ranked on title and lead, then the
    /// top [`Budget::root_top_k`] were judged one at a time. The failed batch
    /// counts in [`Walk::root_judge_calls`]. The singles are a separate
    /// allowance, so this path's cap is `1 + root_top_k`, not [`Budget::root_calls`].
    LexicalFallback,
}

/// Judge-call and open-page budget for one walk. The 48 KB cap always applies.
///
/// `max_judge_calls` is the descent budget and is not raised to cover more
/// roots. The root pass scores every root up to [`ROOT_SCORE_CAP`] (512), in
/// batches of `root_batch_size`. Its call allowance is that many roots, and
/// at least `root_calls`, so a configured `root_calls` of 4 does not stop the
/// pass after 64 roots. A complete batch still costs one call. Ids a batch
/// left out cost one fill-in each, taken from the same allowance. A malformed
/// root batch is reported as one call, then the lexical fallback may spend up
/// to `root_top_k` more (`1 + root_top_k` on [`Walk::root_judge_calls`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_judge_calls: u32,
    pub max_pages: u32,
    /// Passed through from config. The walk sizes the root allowance from the
    /// root count instead of stopping at this value. See [`ROOT_SCORE_CAP`].
    pub root_calls: u32,
    pub root_batch_size: u32,
    pub root_top_k: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_judge_calls: DEFAULT_JUDGE_CALLS,
            max_pages: OPEN_PAGE_CAP,
            root_calls: DEFAULT_ROOT_CALLS,
            root_batch_size: DEFAULT_ROOT_BATCH,
            root_top_k: DEFAULT_ROOT_TOP_K,
        }
    }
}

/// Result of a walk: the nodes that were opened, and their pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Walk {
    pub doc_id: DocId,
    pub query: String,
    pub nodes: Vec<NodeId>,
    pub passages: Vec<Passage>,
    pub judge_calls: u32,
    /// Beam terminals that were not opened because the page or byte cap would
    /// have been exceeded. Whole nodes only; nothing here was clipped.
    #[serde(default)]
    pub skipped: Vec<NodeId>,
    /// Every candidate the walk scored, in call order. Confidence is recorded
    /// and is not used to drop or reorder a candidate.
    #[serde(default)]
    pub judged: Vec<Judged>,
    /// Judge calls spent on the root pass, counted in [`Self::judge_calls`].
    ///
    /// On [`RootPath::Batch`] this counts every batch request and every fill-in.
    /// The allowance is the number of roots scored, at most [`ROOT_SCORE_CAP`],
    /// so the field does not grow past that cap on this path. A complete batch
    /// of 16 costs one call, not 16.
    /// On [`RootPath::LexicalFallback`] it is the failed batch plus at most
    /// [`Budget::root_top_k`] singles. That cap is `1 + root_top_k`.
    #[serde(default)]
    pub root_judge_calls: u32,
    /// `batch` or `lexical_fallback`.
    #[serde(default)]
    pub root_path: RootPath,
    /// Root ids the root pass did not score, in tree order. Empty when every
    /// root was judged. Roots past [`ROOT_SCORE_CAP`], holes a partial batch
    /// could not fill, and a lexical cut that judges only `root_top_k` are
    /// listed here. Nothing is dropped without an entry.
    #[serde(default)]
    pub roots_skipped: Vec<NodeId>,
}

/// One scored candidate. `rank` equals `score`: confidence is not applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judged {
    pub node_id: NodeId,
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    /// Model score, 0..=3. This is what the beam sorts on.
    pub score: u8,
    pub confidence: Option<f64>,
    /// Same as `score`. Kept so a caller can show the value the beam used.
    pub rank: u8,
}

/// A child heading shown to the judge next to the page lead, not inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildTitle {
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
}

/// What the judge sees for one node. Scores are 0–3.
///
/// `lead` is the node's own page text, already capped. `child_titles` is a
/// separate list. Callers must not reconstruct one by parsing the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: NodeId,
    pub title: String,
    pub lead: String,
    pub child_titles: Vec<ChildTitle>,
    pub page_start: u32,
    pub page_end: u32,
    pub level: u8,
}

/// In-progress node, before ids and leads are assigned.
#[derive(Debug, Clone)]
pub(crate) struct RawNode {
    pub title: String,
    pub page_start: u32,
    pub page_end: u32,
    pub source: NodeSource,
    pub children: Vec<RawNode>,
}

#[derive(Debug, Clone)]
pub(crate) struct BuiltTree {
    pub meta: DocMeta,
    pub nodes: Vec<Node>,
}
