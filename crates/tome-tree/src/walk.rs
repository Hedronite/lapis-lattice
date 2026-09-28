//! Beam-2 walk. The judge scores children; unavailable is an error, never a guess.

use crate::error::{Result, TomeError};
use crate::types::{BEAM, Budget, Candidate, Node, NodeId, STOP_PAGES};

/// Score one candidate from 0 to 3.
///
/// `Err(JudgeUnavailable)` (or any other error) stops the walk. A missing
/// transport, an empty answer, or a low-confidence score must not become a path.
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

pub(crate) fn choose(
    roots: &[Node],
    query: &str,
    judge: &dyn Judge,
    budget: Budget,
) -> Result<(Vec<NodeId>, u32)> {
    let mut chosen: Vec<Node> = Vec::new();
    let mut frontier: Vec<Node> = roots.to_vec();
    let mut calls = 0u32;
    loop {
        let mut pool: Vec<(u8, Node)> = Vec::new();
        let mut expandable = false;
        for node in frontier {
            let span = node.page_end.saturating_sub(node.page_start).saturating_add(1);
            if node.children.is_empty() || span <= STOP_PAGES {
                chosen.push(node);
                continue;
            }
            expandable = true;
            for child in &node.children {
                if calls >= budget.max_judge_calls {
                    return Err(TomeError::OverBudget {
                        detail: format!("judge call budget {} exhausted", budget.max_judge_calls),
                    });
                }
                let candidate = Candidate {
                    id: child.id.clone(),
                    title: child.title.clone(),
                    lead: child.lead.clone(),
                    page_start: child.page_start,
                    page_end: child.page_end,
                    level: child.level,
                };
                let score = judge.score(query, &candidate)?;
                if score > 3 {
                    return Err(TomeError::JudgeUnavailable {
                        reason: format!("score {score} outside 0..=3"),
                    });
                }
                calls += 1;
                pool.push((score, child.clone()));
            }
        }
        if !expandable || pool.is_empty() {
            break;
        }
        pool.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        frontier = pool.into_iter().take(BEAM).map(|(_, node)| node).collect();
    }
    chosen.sort_by(|a, b| a.page_start.cmp(&b.page_start).then(a.id.cmp(&b.id)));
    let ids = chosen.into_iter().map(|n| n.id).collect();
    Ok((ids, calls))
}
