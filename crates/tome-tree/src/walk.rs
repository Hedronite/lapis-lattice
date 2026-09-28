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
//! A malformed or partial batch is not that failure. On the root pass it
//! switches to a lexical pre-rank.

use crate::error::{Result, TomeError};
use crate::types::{
    Assessment, BEAM, Budget, Candidate, Judged, Node, NodeId, OPEN_BYTE_CAP, OPEN_PAGE_CAP, RootPath,
    STOP_PAGES,
};

/// Score one candidate from 0 to 3, or a batch of them.
///
/// A missing transport, a missing score, or a score outside 0..=3 on
/// [`assess`](Judge::assess) is `judge_unavailable`. Low confidence is not:
/// it is stored on the assessment and the walk ignores it when ranking.
///
/// [`score_batch`](Judge::score_batch) should return one assessment per
/// candidate, in order. `Err(judge_unavailable)` aborts the walk. Any other
/// error, or an `Ok` that does not cover every candidate with a score in
/// 0..=3, is a malformed batch. The root pass then pre-ranks on title and lead.
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

    /// One model call scores every candidate. The default fans out through
    /// [`assess`](Judge::assess), which costs one call per candidate.
    fn score_batch(&self, query: &str, candidates: &[Candidate]) -> Result<Vec<Assessment>> {
        candidates.iter().map(|c| self.assess(query, c)).collect()
    }

    /// Calls charged for one [`score_batch`](Judge::score_batch) of this slice.
    /// A fan-out judge costs one per candidate. A batching judge costs 1.
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

    fn score_batch(&self, query: &str, candidates: &[Candidate]) -> Result<Vec<Assessment>> {
        if self.fail_batch {
            return Err(TomeError::Parse("scripted malformed batch".into()));
        }
        if !self.batch {
            return candidates.iter().map(|c| self.assess(query, c)).collect();
        }
        let mut out = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            out.push(Assessment {
                score: self.lookup(candidate.id.as_str())?,
                confidence: self.confidence.get(candidate.id.as_str()).copied(),
            });
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
    let (root_picks, mut judged, root_calls, root_path) =
        score_roots(roots, query, judge, budget, batch_size)?;
    let mut terminals = Vec::new();
    let mut frontier = beam_next(root_picks, &mut terminals);
    let mut descent_calls = 0u32;
    // Once a batched response is unusable, later sibling sets skip the batch
    // call and pre-rank. Real System One returns one score, so retrying the
    // batch at every level spends the descent budget before a page is opened.
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
        judged.extend(extra);
        frontier = beam_next(picks, &mut terminals);
    }
    let page_cap = budget.max_pages.min(OPEN_PAGE_CAP);
    let (ids, skipped) = fit_whole_nodes(terminals, page_cap, OPEN_BYTE_CAP, &mut page_len)?;
    Ok(Choice { ids, skipped, calls: root_calls + descent_calls, judged, root_calls, root_path })
}

pub(crate) struct Choice {
    pub ids: Vec<NodeId>,
    pub skipped: Vec<NodeId>,
    pub calls: u32,
    pub judged: Vec<Judged>,
    pub root_calls: u32,
    pub root_path: RootPath,
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
) -> Result<(Vec<Pick>, Vec<Judged>, u32, RootPath)> {
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
        let nodes = &roots[offset..offset + count];
        let candidates = candidates_of(nodes, BATCH_LEAD);
        let cost = judge.batch_cost(&candidates);
        match judge.score_batch(query, &candidates) {
            Err(TomeError::JudgeUnavailable { reason }) => {
                return Err(TomeError::JudgeUnavailable { reason });
            }
            Ok(answers) if usable(&answers, count) => {
                push_scored(nodes, &answers, &mut picks, &mut judged);
                calls += cost;
                offset += count;
            }
            _ => {
                calls += cost;
                if picks.is_empty() {
                    let (fallback, extra, used) = lexical_fallback(
                        roots,
                        query,
                        judge,
                        limit.saturating_sub(calls),
                        budget.root_top_k,
                    )?;
                    judged.extend(extra);
                    return Ok((fallback, judged, calls + used, RootPath::LexicalFallback));
                }
                break;
            }
        }
    }
    if picks.is_empty() {
        return Err(TomeError::OverBudget {
            detail: format!("root pass scored nothing within {limit} calls"),
        });
    }
    Ok((picks, judged, calls, RootPath::Batch))
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
        let slice = &nodes[offset..offset + count];
        let candidates = candidates_of(slice, BATCH_LEAD);
        let cost = judge.batch_cost(&candidates);
        match judge.score_batch(query, &candidates) {
            Err(TomeError::JudgeUnavailable { reason }) => {
                return Err(TomeError::JudgeUnavailable { reason });
            }
            Ok(answers) if usable(&answers, count) => {
                push_scored(slice, &answers, &mut picks, &mut judged);
                used += cost;
                offset += count;
            }
            _ => {
                *batch_broken = true;
                if count > 1 && cost == 1 {
                    used += 1;
                }
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
        }
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

fn usable(answers: &[Assessment], n: usize) -> bool {
    answers.len() == n && answers.iter().all(|answer| answer.score <= 3)
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
    let mut lead: String = node.lead.chars().take(lead_chars.min(400)).collect();
    if !node.children.is_empty() {
        lead.push_str("\n\nChild sections:\n");
        for child in node.children.iter().take(16) {
            let title: String = child.title.chars().take(80).collect();
            lead.push_str(&format!("- {title} (pp. {}–{})\n", child.page_start, child.page_end));
        }
        let extra = node.children.len().saturating_sub(16);
        if extra > 0 {
            lead.push_str(&format!("- … {extra} more\n"));
        }
    }
    let cap = if lead_chars <= BATCH_LEAD { 800 } else { SINGLE_LEAD };
    if lead.chars().count() > cap {
        lead = lead.chars().take(cap).collect();
    }
    Candidate {
        id: node.id.clone(),
        title: node.title.clone(),
        lead,
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
