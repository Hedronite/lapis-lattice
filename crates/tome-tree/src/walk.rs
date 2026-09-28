//! Beam-2 walk. Roots are judged first; unavailable is an error, never a guess.

use crate::error::{Result, TomeError};
use crate::types::{
    Assessment, BEAM, Budget, Candidate, DESCENT_RESERVE, JudgeConfidence, Judged, Node, NodeId,
    OPEN_BYTE_CAP, OPEN_PAGE_CAP, STOP_PAGES,
};

/// Score one candidate from 0 to 3.
///
/// A missing transport, a missing score, or a score outside 0..=3 is
/// `judge_unavailable`. Low confidence is not, by itself, a failure: [`assess`]
/// reports it and [`Judge::confidence_policy`] decides whether to abort or
/// down-weight.
///
/// The Jev adapter blocks on a tokio runtime. That runtime must be multi-thread:
/// `block_in_place` panics on a current-thread runtime, and calling `walk` from
/// inside an existing `block_on` can deadlock.
///
/// [`assess`]: Judge::assess
pub trait Judge {
    fn score(&self, query: &str, candidate: &Candidate) -> Result<u8>;

    /// Score plus confidence. The default calls [`score`](Judge::score) and
    /// reports no confidence, which neither policy treats as a failure.
    fn assess(&self, query: &str, candidate: &Candidate) -> Result<Assessment> {
        Ok(Assessment { score: self.score(query, candidate)?, confidence: None })
    }

    /// From `[tome] judge_confidence` / `judge_min_confidence`. Default is
    /// fail-closed at 0.6, which is the historical floor, not a chosen spike policy.
    fn confidence_policy(&self) -> JudgeConfidence {
        JudgeConfidence::default()
    }
}

/// Offline judge. A node id with no scripted score is unavailable.
#[derive(Debug, Clone)]
pub struct FakeJudge {
    scores: std::collections::BTreeMap<String, u8>,
    confidence: std::collections::BTreeMap<String, f64>,
    policy: JudgeConfidence,
}

impl FakeJudge {
    pub fn new(scores: impl IntoIterator<Item = (impl Into<String>, u8)>) -> Self {
        Self {
            scores: scores.into_iter().map(|(id, score)| (id.into(), score)).collect(),
            confidence: std::collections::BTreeMap::new(),
            policy: JudgeConfidence::default(),
        }
    }

    pub fn with_confidence(mut self, pairs: impl IntoIterator<Item = (impl Into<String>, f64)>) -> Self {
        self.confidence = pairs.into_iter().map(|(id, c)| (id.into(), c)).collect();
        self
    }

    pub fn with_policy(mut self, policy: JudgeConfidence) -> Self {
        self.policy = policy;
        self
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

    fn assess(&self, query: &str, candidate: &Candidate) -> Result<Assessment> {
        Ok(Assessment {
            score: self.score(query, candidate)?,
            confidence: self.confidence.get(candidate.id.as_str()).copied(),
        })
    }

    fn confidence_policy(&self) -> JudgeConfidence {
        self.policy
    }
}

struct Pick {
    rank: u8,
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
) -> Result<Choice> {
    let mut terminals: Vec<Pick> = Vec::new();
    let mut frontier: Vec<Node> = roots.to_vec();
    let mut judged: Vec<Judged> = Vec::new();
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
            let candidate = candidate_of(&node);
            let assessment = judge.assess(query, &candidate)?;
            let rank = rank_of(judge.confidence_policy(), &assessment)?;
            calls += 1;
            judged.push(Judged {
                node_id: node.id.clone(),
                title: node.title.clone(),
                page_start: node.page_start,
                page_end: node.page_end,
                score: assessment.score,
                confidence: assessment.confidence,
                rank,
            });
            scored.push(Pick { rank, node });
        }
        scored.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.node.id.cmp(&b.node.id)));
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
    Ok(Choice { ids, skipped, calls, judged })
}

pub(crate) struct Choice {
    pub ids: Vec<NodeId>,
    pub skipped: Vec<NodeId>,
    pub calls: u32,
    pub judged: Vec<Judged>,
}

fn candidate_of(node: &Node) -> Candidate {
    let mut lead = node.lead.clone();
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
    if lead.chars().count() > 2000 {
        lead = lead.chars().take(2000).collect();
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

fn rank_of(policy: JudgeConfidence, assessment: &Assessment) -> Result<u8> {
    if assessment.score > 3 {
        return Err(TomeError::JudgeUnavailable {
            reason: format!("score {} outside 0..=3", assessment.score),
        });
    }
    match policy {
        JudgeConfidence::FailClosed { min } => {
            if let Some(confidence) = assessment.confidence
                && confidence < min
            {
                return Err(TomeError::JudgeUnavailable {
                    reason: format!("confidence {confidence} below {min}"),
                });
            }
            Ok(assessment.score)
        }
        JudgeConfidence::DownWeight { min } => {
            let Some(confidence) = assessment.confidence else {
                return Ok(assessment.score);
            };
            if confidence >= min {
                return Ok(assessment.score);
            }
            let weighted = (f64::from(assessment.score) * confidence).round();
            if !weighted.is_finite() {
                return Ok(0);
            }
            Ok(weighted.clamp(0.0, 3.0) as u8)
        }
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
