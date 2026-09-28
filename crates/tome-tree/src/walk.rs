//! Beam-2 walk.
//!
//! The root pass has its own call budget and scores roots in batches. Descent
//! spends `Budget::max_judge_calls` and batches sibling sets the same way.
//! [`DESCENT_RESERVE`](crate::types::DESCENT_RESERVE) stays exported; frontiers
//! are no longer cut to it.
//!
//! Rank is the raw score. Confidence is recorded and never gates or reorders a
//! candidate. `judge_unavailable` is a transport or model failure: a missing
//! score, a score outside 0..=3 on a one-at-a-time call, or a dead transport.
//! A malformed batch is not that failure. On the root pass it switches to a
//! lexical pre-rank. The failed batch counts in `judge_calls`. The fallback
//! singles are a separate `root_top_k` allowance, so that path reports at most
//! `1 + root_top_k` and does not borrow from `root_calls`. Ids a batch left
//! out are judged one at a time and each of those calls counts too. Roots the
//! pass does not score are listed on [`Walk::roots_skipped`](crate::Walk::roots_skipped).

use crate::error::{Result, TomeError};
use crate::types::{
    Assessment, BEAM, Budget, Candidate, ChildTitle, Judged, Node, NodeId, OPEN_BYTE_CAP, OPEN_PAGE_CAP,
    RootPath, STOP_PAGES,
};

/// Score one candidate from 0 to 3, or a batch of them.
///
/// A missing transport, a missing score, or a score outside 0..=3 on
/// [`assess`](Judge::assess) is `judge_unavailable`. Low confidence is not:
/// it is stored on the assessment and the walk ignores it when ranking.
///
/// [`score_batch`](Judge::score_batch) returns one slot per candidate, in
/// order. `Some` is a score in 0..=3. `None` is an id this call did not score:
/// the walk judges that id with [`assess`](Judge::assess) and counts the call.
/// `Err(judge_unavailable)` aborts the walk. Any other error is a malformed
/// batch. The root pass counts that failed call, then pre-ranks and judges
/// `root_top_k` roots on the separate fallback allowance.
///
/// The Jev adapter blocks on a tokio runtime. That runtime must be multi-thread:
/// `block_in_place` panics on a current-thread runtime, and calling `walk` from
/// inside an existing `block_on` can deadlock.
pub trait Judge {
    fn score(&self, query: &str, candidate: &Candidate) -> Result<u8>;

    /// Score plus confidence. The default calls [`score`](Judge::score) and
    /// reports no confidence.
    fn assess(&self, query: &str, candidate: &Candidate) -> Result<Assessment> {
        Ok(Assessment { score: self.score(query, candidate)?, confidence: None })
    }

    /// One model call scores every candidate. `None` means that id was not
    /// scored. The default fans out through [`assess`](Judge::assess) and
    /// wraps each result in `Some`, which costs one call per candidate.
    fn score_batch(&self, query: &str, candidates: &[Candidate]) -> Result<Vec<Option<Assessment>>> {
        candidates.iter().map(|c| self.assess(query, c).map(Some)).collect()
    }

    /// Calls charged for a [`score_batch`](Judge::score_batch) that scores
    /// every id. A fan-out judge costs one per candidate. A batching judge
    /// costs 1 for the batch request. Ids left as `None` cost one more each,
    /// charged by the walk when it fills them in.
    fn batch_cost(&self, candidates: &[Candidate]) -> u32 {
        candidates.len().max(1) as u32
    }
}

/// Offline judge. A node id with no scripted score and no default is unavailable.
#[derive(Debug, Clone)]
pub struct FakeJudge {
    scores: std::collections::BTreeMap<String, u8>,
    confidence: std::collections::BTreeMap<String, f64>,
    /// `batch_cost` is 1 and `score_batch` returns every candidate from one call.
    batch: bool,
    /// `score_batch` returns a parse error. `assess` still works, so the walk
    /// can take the lexical fallback.
    fail_batch: bool,
    default_score: Option<u8>,
}

impl FakeJudge {
    pub fn new(scores: impl IntoIterator<Item = (impl Into<String>, u8)>) -> Self {
        Self {
            scores: scores.into_iter().map(|(id, score)| (id.into(), score)).collect(),
            confidence: std::collections::BTreeMap::new(),
            batch: false,
            fail_batch: false,
            default_score: None,
        }
    }

    pub fn with_confidence(mut self, pairs: impl IntoIterator<Item = (impl Into<String>, f64)>) -> Self {
        self.confidence = pairs.into_iter().map(|(id, c)| (id.into(), c)).collect();
        self
    }

    /// Score a whole list in one call.
    pub fn batched(mut self) -> Self {
        self.batch = true;
        self
    }

    /// The next batched response is malformed. One-at-a-time `assess` is unchanged.
    pub fn fail_batch(mut self) -> Self {
        self.batch = true;
        self.fail_batch = true;
        self
    }

    /// Score used when a node id is not scripted. Lets a fallback judge the
    /// roots a batch did not name.
    pub fn with_default(mut self, score: u8) -> Self {
        self.default_score = Some(score);
        self
    }
}

impl Judge for FakeJudge {
    fn score(&self, _query: &str, candidate: &Candidate) -> Result<u8> {
        self.lookup(candidate.id.as_str())
    }

    fn assess(&self, query: &str, candidate: &Candidate) -> Result<Assessment> {
        Ok(Assessment {
            score: self.score(query, candidate)?,
            confidence: self.confidence.get(candidate.id.as_str()).copied(),
        })
    }

    fn score_batch(&self, query: &str, candidates: &[Candidate]) -> Result<Vec<Option<Assessment>>> {
        if self.fail_batch {
            return Err(TomeError::Parse("scripted malformed batch".into()));
        }
        if !self.batch {
            return candidates.iter().map(|c| self.assess(query, c).map(Some)).collect();
        }
        let mut out = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            out.push(Some(Assessment {
                score: self.lookup(candidate.id.as_str())?,
                confidence: self.confidence.get(candidate.id.as_str()).copied(),
            }));
        }
        Ok(out)
    }

    fn batch_cost(&self, candidates: &[Candidate]) -> u32 {
        if self.batch { 1 } else { candidates.len().max(1) as u32 }
    }
}

impl FakeJudge {
    fn lookup(&self, id: &str) -> Result<u8> {
        match self.scores.get(id).copied().or(self.default_score) {
            Some(score) if score <= 3 => Ok(score),
            Some(score) => {
                Err(TomeError::JudgeUnavailable { reason: format!("scripted score {score} outside 0..=3") })
            }
            None => Err(TomeError::JudgeUnavailable { reason: format!("no scripted score for {id}") }),
        }
    }
}

struct Pick {
    rank: u8,
    node: Node,
}

/// How many characters of the node's own lead go into a batched candidate.
const BATCH_LEAD: usize = 240;

/// One-at-a-time candidate text, including child titles.
const SINGLE_LEAD: usize = 2_000;

/// Judge the roots under `budget.root_calls`, then descend under
/// `budget.max_judge_calls`. Keep the top [`BEAM`] at each frontier.
///
/// A leaf, or a node of at most [`STOP_PAGES`] pages, is a terminal. Terminals
/// are then chosen whole, highest score first, until the page and byte caps
/// would be exceeded. A node is never split across pages.
pub(crate) fn choose(
    roots: &[Node],
    query: &str,
    judge: &dyn Judge,
    budget: Budget,
    mut page_len: impl FnMut(u32) -> Result<usize>,
) -> Result<Choice> {
    let batch_size = budget.root_batch_size.max(1) as usize;
    let mut root = score_roots(roots, query, judge, budget, batch_size)?;
    let mut terminals = Vec::new();
    let mut frontier = beam_next(root.picks, &mut terminals);
    let mut descent_calls = 0u32;
    // Once a batched response is unusable, later sibling sets skip the batch
    // call and pre-rank, so a wide chapter does not spend a call per frontier.
    let mut batch_broken = false;
    while !frontier.is_empty() {
        let remaining = budget.max_judge_calls.saturating_sub(descent_calls);
        let (picks, extra, used) = score_set(
            &frontier,
            query,
            judge,
            ScoreLimits {
                batch_size,
                remaining,
                budget_label: budget.max_judge_calls,
                top_k: budget.root_top_k,
            },
            &mut batch_broken,
        )?;
        descent_calls += used;
        root.judged.extend(extra);
        frontier = beam_next(picks, &mut terminals);
    }
    let page_cap = budget.max_pages.min(OPEN_PAGE_CAP);
    let (ids, skipped) = fit_whole_nodes(terminals, page_cap, OPEN_BYTE_CAP, &mut page_len)?;
    Ok(Choice {
        ids,
        skipped,
        calls: root.calls + descent_calls,
        judged: root.judged,
        root_calls: root.calls,
        root_path: root.path,
        roots_skipped: root.skipped,
    })
}

pub(crate) struct Choice {
    pub ids: Vec<NodeId>,
    pub skipped: Vec<NodeId>,
    pub calls: u32,
    pub judged: Vec<Judged>,
    pub root_calls: u32,
    pub root_path: RootPath,
    pub roots_skipped: Vec<NodeId>,
}

struct RootScore {
    picks: Vec<Pick>,
    judged: Vec<Judged>,
    calls: u32,
    path: RootPath,
    skipped: Vec<NodeId>,
}

/// Score every root that fits in the root budget. The first unusable batch,
/// when nothing has been accepted yet, switches the whole pass to the lexical
/// path. A later unusable batch keeps the prefix already scored.
fn score_roots(
    roots: &[Node],
    query: &str,
    judge: &dyn Judge,
    budget: Budget,
    batch_size: usize,
) -> Result<RootScore> {
    let limit = budget.root_calls;
    let mut calls = 0u32;
    let mut judged = Vec::new();
    let mut picks = Vec::new();
    let mut offset = 0usize;
    while offset < roots.len() {
        let remaining = limit.saturating_sub(calls);
        let count = affordable_count(&roots[offset..], batch_size, remaining, judge);
        if count == 0 {
            break;
        }
        let consumed =
            consume_batch(&roots[offset..offset + count], query, judge, remaining, &mut picks, &mut judged)?;
        if consumed.broken {
            // The failed batch is a real call. The lexical singles use
            // `root_top_k`, not whatever `root_calls` has left, so the reported
            // total stays within `1 + root_top_k`.
            calls += consumed.calls;
            if picks.is_empty() {
                let (fallback, extra, used) =
                    lexical_fallback(roots, query, judge, budget.root_top_k, budget.root_top_k)?;
                judged.extend(extra);
                return Ok(RootScore {
                    skipped: unscored_roots(roots, &fallback),
                    picks: fallback,
                    judged,
                    calls: calls + used,
                    path: RootPath::LexicalFallback,
                });
            }
            break;
        }
        calls += consumed.calls;
        offset += consumed.advance;
    }
    if picks.is_empty() {
        return Err(TomeError::OverBudget {
            detail: format!("root pass scored nothing within {limit} calls"),
        });
    }
    Ok(RootScore { skipped: unscored_roots(roots, &picks), picks, judged, calls, path: RootPath::Batch })
}

/// Roots that received no score, in tree order.
fn unscored_roots(roots: &[Node], picks: &[Pick]) -> Vec<NodeId> {
    let scored: std::collections::BTreeSet<&str> = picks.iter().map(|pick| pick.node.id.as_str()).collect();
    roots.iter().filter(|node| !scored.contains(node.id.as_str())).map(|node| node.id.clone()).collect()
}

/// Pre-rank every root on title and lead, then judge the top `k` one at a time.
fn lexical_fallback(
    roots: &[Node],
    query: &str,
    judge: &dyn Judge,
    remaining: u32,
    top_k: u32,
) -> Result<(Vec<Pick>, Vec<Judged>, u32)> {
    let k = (top_k.min(remaining) as usize).min(roots.len());
    if k == 0 {
        return Err(TomeError::OverBudget { detail: "root fallback has no judge calls left".into() });
    }
    let ranked = lexical_order(roots, query);
    let mut picks = Vec::new();
    let mut judged = Vec::new();
    for node in ranked.into_iter().take(k) {
        let assessment = judge.assess(query, &candidate_of(&node, SINGLE_LEAD))?;
        if assessment.score > 3 {
            return Err(TomeError::JudgeUnavailable {
                reason: format!("score {} outside 0..=3", assessment.score),
            });
        }
        push_scored(std::slice::from_ref(&node), std::slice::from_ref(&assessment), &mut picks, &mut judged);
    }
    Ok((picks, judged, k as u32))
}

fn lexical_order(roots: &[Node], query: &str) -> Vec<Node> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(|word| word.to_lowercase())
        .collect();
    let mut ranked: Vec<(u32, Node)> = roots
        .iter()
        .map(|node| {
            let hay = format!("{} {}", node.title, node.lead).to_lowercase();
            let overlap = terms.iter().filter(|term| hay.contains(term.as_str())).count() as u32;
            (overlap, node.clone())
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    ranked.into_iter().map(|(_, node)| node).collect()
}

/// Score a sibling set. A usable batch is charged `batch_cost`. A malformed
/// batch pre-ranks the set on title and lead and judges the top `top_k` one
/// at a time, same as the root pass, so a wide chapter does not exhaust the
/// descent budget.
struct ScoreLimits {
    batch_size: usize,
    remaining: u32,
    budget_label: u32,
    top_k: u32,
}

fn score_set(
    nodes: &[Node],
    query: &str,
    judge: &dyn Judge,
    limits: ScoreLimits,
    batch_broken: &mut bool,
) -> Result<(Vec<Pick>, Vec<Judged>, u32)> {
    if *batch_broken {
        return lexical_fallback(nodes, query, judge, limits.remaining, limits.top_k);
    }
    let mut offset = 0usize;
    let mut used = 0u32;
    let mut picks = Vec::new();
    let mut judged = Vec::new();
    while offset < nodes.len() {
        let left = limits.remaining.saturating_sub(used);
        let count = affordable_count(&nodes[offset..], limits.batch_size, left, judge);
        if count == 0 {
            return Err(TomeError::OverBudget {
                detail: format!("judge call budget {} exhausted", limits.budget_label),
            });
        }
        let consumed =
            consume_batch(&nodes[offset..offset + count], query, judge, left, &mut picks, &mut judged)?;
        if consumed.broken {
            *batch_broken = true;
            used += consumed.calls;
            let (fallback, extra, fallback_used) = lexical_fallback(
                &nodes[offset..],
                query,
                judge,
                limits.remaining.saturating_sub(used),
                limits.top_k,
            )?;
            picks.extend(fallback);
            judged.extend(extra);
            return Ok((picks, judged, used + fallback_used));
        }
        used += consumed.calls;
        offset += consumed.advance;
    }
    Ok((picks, judged, used))
}

fn affordable_count(nodes: &[Node], batch_size: usize, remaining: u32, judge: &dyn Judge) -> usize {
    let cap = batch_size.min(nodes.len());
    let mut n = 0usize;
    while n < cap {
        let candidates = candidates_of(&nodes[..=n], BATCH_LEAD);
        if judge.batch_cost(&candidates) > remaining {
            break;
        }
        n += 1;
    }
    n
}

struct Consumed {
    calls: u32,
    advance: usize,
    /// The batch scored nothing. The caller lexical-falls-back this slice.
    broken: bool,
}

/// Score one slice. A complete batch costs [`Judge::batch_cost`]. A partial
/// batch costs that plus one [`Judge::assess`] per missing id that still fits
/// in `remaining`. A malformed batching call costs 1.
fn consume_batch(
    nodes: &[Node],
    query: &str,
    judge: &dyn Judge,
    remaining: u32,
    picks: &mut Vec<Pick>,
    judged: &mut Vec<Judged>,
) -> Result<Consumed> {
    let candidates = candidates_of(nodes, BATCH_LEAD);
    let cost = judge.batch_cost(&candidates);
    let count = nodes.len();
    let slots = match judge.score_batch(query, &candidates) {
        Err(TomeError::JudgeUnavailable { reason }) => return Err(TomeError::JudgeUnavailable { reason }),
        Ok(slots) => slots,
        Err(_) => {
            let calls = if cost == 1 { 1 } else { 0 };
            return Ok(Consumed { calls, advance: 0, broken: true });
        }
    };
    let kept = |slot: &Option<Assessment>| slot.as_ref().is_some_and(|answer| answer.score <= 3);
    if slots.len() == count && slots.iter().all(&kept) {
        let answers: Vec<Assessment> = slots.into_iter().flatten().collect();
        push_scored(nodes, &answers, picks, judged);
        return Ok(Consumed { calls: cost, advance: count, broken: false });
    }
    if slots.len() != count || !slots.iter().any(&kept) {
        let calls = if cost == 1 { 1 } else { 0 };
        return Ok(Consumed { calls, advance: 0, broken: true });
    }
    let filled = slots.iter().filter(|slot| kept(slot)).count() as u32;
    let mut spent = if cost == 1 { 1 } else { filled };
    for (node, slot) in nodes.iter().zip(slots) {
        if let Some(assessment) = slot.filter(|answer| answer.score <= 3) {
            push_scored(std::slice::from_ref(node), std::slice::from_ref(&assessment), picks, judged);
            continue;
        }
        if remaining.saturating_sub(spent) == 0 {
            continue;
        }
        let assessment = judge.assess(query, &candidate_of(node, SINGLE_LEAD))?;
        if assessment.score > 3 {
            return Err(TomeError::JudgeUnavailable {
                reason: format!("score {} outside 0..=3", assessment.score),
            });
        }
        spent += 1;
        push_scored(std::slice::from_ref(node), std::slice::from_ref(&assessment), picks, judged);
    }
    Ok(Consumed { calls: spent, advance: count, broken: false })
}

fn push_scored(nodes: &[Node], answers: &[Assessment], picks: &mut Vec<Pick>, judged: &mut Vec<Judged>) {
    for (node, assessment) in nodes.iter().zip(answers) {
        judged.push(Judged {
            node_id: node.id.clone(),
            title: node.title.clone(),
            page_start: node.page_start,
            page_end: node.page_end,
            score: assessment.score,
            confidence: assessment.confidence,
            rank: assessment.score,
        });
        picks.push(Pick { rank: assessment.score, node: node.clone() });
    }
}

fn beam_next(mut picks: Vec<Pick>, terminals: &mut Vec<Pick>) -> Vec<Node> {
    picks.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.node.id.cmp(&b.node.id)));
    let mut next = Vec::new();
    for pick in picks.into_iter().take(BEAM) {
        let span = span_pages(&pick.node);
        if pick.node.children.is_empty() || span <= STOP_PAGES {
            terminals.push(pick);
        } else {
            next.extend(pick.node.children);
        }
    }
    next
}

fn candidates_of(nodes: &[Node], lead_chars: usize) -> Vec<Candidate> {
    nodes.iter().map(|node| candidate_of(node, lead_chars)).collect()
}

fn candidate_of(node: &Node, lead_chars: usize) -> Candidate {
    let page_cap = if lead_chars <= BATCH_LEAD { lead_chars } else { lead_chars.min(400) };
    let lead: String = node.lead.chars().take(page_cap).collect();
    let child_titles = node
        .children
        .iter()
        .take(16)
        .map(|child| ChildTitle {
            title: child.title.chars().take(80).collect(),
            page_start: child.page_start,
            page_end: child.page_end,
        })
        .collect();
    Candidate {
        id: node.id.clone(),
        title: node.title.clone(),
        lead,
        child_titles,
        page_start: node.page_start,
        page_end: node.page_end,
        level: node.level,
    }
}

/// Keep entire nodes, best score first, that fit in both caps.
fn fit_whole_nodes(
    mut terminals: Vec<Pick>,
    page_cap: u32,
    byte_cap: usize,
    page_len: &mut impl FnMut(u32) -> Result<usize>,
) -> Result<(Vec<NodeId>, Vec<NodeId>)> {
    terminals.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.node.id.cmp(&b.node.id)));
    let mut taken: Vec<Node> = Vec::new();
    let mut skipped: Vec<NodeId> = Vec::new();
    let mut pages = 0u32;
    let mut bytes = 0usize;
    for pick in terminals {
        let span = span_pages(&pick.node);
        if span == 0 {
            continue;
        }
        let mut node_bytes = 0usize;
        for page in pick.node.page_start..=pick.node.page_end {
            node_bytes = node_bytes.saturating_add(page_len(page)?);
        }
        if pages.saturating_add(span) > page_cap || bytes.saturating_add(node_bytes) > byte_cap {
            skipped.push(pick.node.id);
            continue;
        }
        pages += span;
        bytes += node_bytes;
        taken.push(pick.node);
    }
    if taken.is_empty() {
        return Err(TomeError::OverBudget {
            detail: format!("no judged node fits {page_cap} pages / {byte_cap} bytes"),
        });
    }
    taken.sort_by(|a, b| a.page_start.cmp(&b.page_start).then(a.id.cmp(&b.id)));
    Ok((taken.into_iter().map(|node| node.id).collect(), skipped))
}

fn span_pages(node: &Node) -> u32 {
    if node.page_end < node.page_start { 0 } else { node.page_end - node.page_start + 1 }
}
