//! Beam-2 walk. Roots are judged first; unavailable is an error, never a guess.

use crate::error::{Result, TomeError};
use crate::types::{
    BEAM, Budget, Candidate, DESCENT_RESERVE, Node, NodeId, OPEN_BYTE_CAP, OPEN_PAGE_CAP, STOP_PAGES,
};

/// Score one candidate from 0 to 3.
///
/// `Err(JudgeUnavailable)` (or any other error) stops the walk. A missing
/// transport, an empty answer, or a low-confidence score must not become a path.
///
/// The Jev adapter blocks on a tokio runtime. That runtime must be multi-thread:
/// `block_in_place` panics on a current-thread runtime, and calling `walk` from
/// inside an existing `block_on` can deadlock.
pub trait Judge {
    fn score(&self, query: &str, candidate: &Candidate) -> Result<u8>;
}

/// Offline judge. A node id with no scripted score is unavailable.
#[derive(Debug, Clone)]
pub struct FakeJudge {
    scores: std::collections::BTreeMap<String, u8>,
}

impl FakeJudge {
    pub fn new(scores: impl IntoIterator<Item = (impl Into<String>, u8)>) -> Self {
        Self { scores: scores.into_iter().map(|(id, score)| (id.into(), score)).collect() }
    }
}

impl Judge for FakeJudge {
    fn score(&self, _query: &str, candidate: &Candidate) -> Result<u8> {
        match self.scores.get(candidate.id.as_str()) {
            Some(&score) if score <= 3 => Ok(score),
            Some(&score) => {
                Err(TomeError::JudgeUnavailable { reason: format!("scripted score {score} outside 0..=3") })
            }
            None => {
                Err(TomeError::JudgeUnavailable { reason: format!("no scripted score for {}", candidate.id) })
            }
        }
    }
}

struct Pick {
    score: u8,
    node: Node,
}

/// Judge `roots` first, keep the top [`BEAM`], and descend only into those.
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
) -> Result<(Vec<NodeId>, Vec<NodeId>, u32)> {
    let mut terminals: Vec<Pick> = Vec::new();
    let mut frontier: Vec<Node> = roots.to_vec();
    let mut calls = 0u32;
    while !frontier.is_empty() {
        // A frontier wider than the calls left after `DESCENT_RESERVE` is cut
        // in reading order, so a deep outline still has calls left to descend.
        // When the reserve would leave nothing, the loop below errors once the
        // budget is actually spent (a caller-set budget of 1 still fails).
        let remaining = budget.max_judge_calls.saturating_sub(calls);
        let room = remaining.saturating_sub(DESCENT_RESERVE);
        if room > 0 && (frontier.len() as u32) > room {
            frontier.truncate(room as usize);
        }
        let mut scored: Vec<Pick> = Vec::with_capacity(frontier.len());
        for node in frontier {
            if calls >= budget.max_judge_calls {
                return Err(TomeError::OverBudget {
                    detail: format!("judge call budget {} exhausted", budget.max_judge_calls),
                });
            }
            let candidate = Candidate {
                id: node.id.clone(),
                title: node.title.clone(),
                lead: node.lead.clone(),
                page_start: node.page_start,
                page_end: node.page_end,
                level: node.level,
            };
            let score = judge.score(query, &candidate)?;
            if score > 3 {
                return Err(TomeError::JudgeUnavailable { reason: format!("score {score} outside 0..=3") });
            }
            calls += 1;
            scored.push(Pick { score, node });
        }
        scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.node.id.cmp(&b.node.id)));
        let mut next = Vec::new();
        for pick in scored.into_iter().take(BEAM) {
            let span = span_pages(&pick.node);
            if pick.node.children.is_empty() || span <= STOP_PAGES {
                terminals.push(pick);
            } else {
                next.extend(pick.node.children);
            }
        }
        frontier = next;
    }
    let page_cap = budget.max_pages.min(OPEN_PAGE_CAP);
    let (ids, skipped) = fit_whole_nodes(terminals, page_cap, OPEN_BYTE_CAP, &mut page_len)?;
    Ok((ids, skipped, calls))
}

/// Keep entire nodes, best score first, that fit in both caps.
fn fit_whole_nodes(
    mut terminals: Vec<Pick>,
    page_cap: u32,
    byte_cap: usize,
    page_len: &mut impl FnMut(u32) -> Result<usize>,
) -> Result<(Vec<NodeId>, Vec<NodeId>)> {
    terminals.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.node.id.cmp(&b.node.id)));
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
