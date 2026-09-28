//! In-memory `TomeApi` for offline tests of the MCP wiring and the harness.
//! Fixtures are generated in code; no tome text is ever stored in the repo.

use std::collections::BTreeMap;

use crate::contract::{
    Budget, Candidate, DocId, DocMeta, Judge, Node, NodeId, NodeSource, Passage, Result, TomeApi, TomeError,
    Walk, check_open_budget,
};

#[derive(Debug, Clone)]
pub struct FakeDoc {
    pub meta: DocMeta,
    pub roots: Vec<Node>,
    /// 1-based page → text.
    pub pages: BTreeMap<u32, String>,
}

#[derive(Debug, Clone, Default)]
pub struct FakeTome {
    pub docs: Vec<FakeDoc>,
    /// Docs that fail closed with `NoStructure` (the Red Team Guide probe).
    pub no_structure: Vec<String>,
}

fn node(id: &str, title: &str, level: u8, a: u32, b: u32, children: Vec<Node>) -> Node {
    Node {
        id: NodeId(id.into()),
        title: title.into(),
        level,
        page_start: a,
        page_end: b,
        page_label: None,
        summary: None,
        lead: format!("{title} lead"),
        source: NodeSource::Outline,
        child_count: children.len() as u32,
        children,
    }
}

impl FakeTome {
    /// Two chapters, the second with two sections; 20 pages of generated text.
    pub fn sample() -> Self {
        let sha = "a".repeat(64);
        let meta = DocMeta {
            doc_id: sha.clone(),
            path: "Archmagus-Stack/09-Tomes/fake/Fake Book.pdf".into(),
            sha256: sha,
            pages: 20,
            outline: true,
            source: NodeSource::Outline,
            built_at: "2026-09-28T00:00:00Z".into(),
            builder_version: "fake-0".into(),
        };
        let roots = vec![
            node("0001", "Chapter 1. Alpha", 1, 1, 8, vec![]),
            node(
                "0002",
                "Chapter 2. Beta",
                1,
                9,
                20,
                vec![
                    node("0002.0001", "Beta basics", 2, 9, 14, vec![]),
                    node("0002.0002", "Gamma details", 2, 15, 20, vec![]),
                ],
            ),
        ];
        let pages = (1..=20)
            .map(|p| (p, format!("page {p} text about {}", if p >= 15 { "gamma" } else { "alpha" })))
            .collect();
        Self {
            docs: vec![FakeDoc { meta, roots, pages }],
            no_structure: vec!["Archmagus-Stack/09-Tomes/fake/No Outline.pdf".into()],
        }
    }

    fn doc(&self, doc: &DocId) -> Result<&FakeDoc> {
        if self.no_structure.iter().any(|d| d == &doc.0) {
            return Err(TomeError::NoStructure { doc: doc.0.clone() });
        }
        self.docs
            .iter()
            .find(|d| d.meta.path == doc.0 || d.meta.sha256 == doc.0)
            .ok_or_else(|| TomeError::UnknownDoc(doc.0.clone()))
    }
}

fn find<'a>(nodes: &'a [Node], id: &NodeId) -> Option<&'a Node> {
    for n in nodes {
        if &n.id == id {
            return Some(n);
        }
        if let Some(f) = find(&n.children, id) {
            return Some(f);
        }
    }
    None
}

fn cut(n: &Node, depth: u8) -> Node {
    let mut c = n.clone();
    c.child_count = n.children.len() as u32;
    c.children = if depth <= 1 { vec![] } else { n.children.iter().map(|k| cut(k, depth - 1)).collect() };
    c
}

impl TomeApi for FakeTome {
    fn docs(&self) -> Result<Vec<DocMeta>> {
        Ok(self.docs.iter().map(|d| d.meta.clone()).collect())
    }

    fn doc_meta(&self, doc: &DocId) -> Result<DocMeta> {
        Ok(self.doc(doc)?.meta.clone())
    }

    fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>> {
        let d = self.doc(doc)?;
        let depth = depth.unwrap_or(2).max(1);
        match node {
            None => Ok(d.roots.iter().map(|n| cut(n, depth)).collect()),
            Some(id) => {
                let n = find(&d.roots, id)
                    .ok_or_else(|| TomeError::UnknownNode { doc: doc.0.clone(), node: id.0.clone() })?;
                Ok(n.children.iter().map(|k| cut(k, depth)).collect())
            }
        }
    }

    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        let d = self.doc(doc)?;
        let mut want = Vec::new();
        for id in nodes {
            let n = find(&d.roots, id)
                .ok_or_else(|| TomeError::UnknownNode { doc: doc.0.clone(), node: id.0.clone() })?;
            for p in n.page_start..=n.page_end {
                want.push((id.clone(), p));
            }
        }
        let bytes: usize = want.iter().map(|(_, p)| d.pages.get(p).map_or(0, String::len)).sum();
        check_open_budget(want.len(), bytes)?;
        Ok(want
            .into_iter()
            .map(|(node_id, page)| Passage {
                node_id,
                page,
                text: d.pages.get(&page).cloned().unwrap_or_default(),
                truncated: false,
            })
            .collect())
    }

    fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk> {
        let d = self.doc(doc)?;
        let mut frontier: Vec<&Node> = d.roots.iter().collect();
        let mut calls = 0u32;
        let mut visited = Vec::new();
        let mut chosen: Vec<NodeId> = Vec::new();
        while !frontier.is_empty() {
            if calls >= budget.max_judge_calls {
                return Err(TomeError::JudgeUnavailable("judge call budget exhausted".into()));
            }
            let cands: Vec<Candidate<'_>> = frontier
                .iter()
                .map(|n| Candidate {
                    id: &n.id,
                    title: &n.title,
                    lead: &n.lead,
                    page_start: n.page_start,
                    page_end: n.page_end,
                })
                .collect();
            let scores = judge.score(query, &cands)?;
            calls += 1;
            if scores.len() != frontier.len() {
                return Err(TomeError::JudgeUnavailable("judge returned a score count mismatch".into()));
            }
            let mut ranked: Vec<(u8, &Node)> = scores.into_iter().zip(frontier.iter().copied()).collect();
            ranked.sort_by_key(|r| std::cmp::Reverse(r.0));
            let mut next = Vec::new();
            for (s, n) in ranked.into_iter().take(budget.beam as usize) {
                if s == 0 {
                    continue;
                }
                visited.push(n.id.clone());
                if n.children.is_empty() || n.page_end - n.page_start < 3 {
                    chosen.push(n.id.clone());
                } else {
                    next.extend(n.children.iter());
                }
            }
            frontier = next;
        }
        if chosen.is_empty() {
            return Err(TomeError::JudgeUnavailable("judge scored every candidate 0".into()));
        }
        // Keep the best leaves that fit the open budget.
        let mut keep = Vec::new();
        let mut pages = 0u32;
        for id in chosen {
            let n = find(&d.roots, &id).expect("chosen node exists");
            let span = n.page_end - n.page_start + 1;
            if pages + span > budget.max_open_pages {
                continue;
            }
            pages += span;
            keep.push(id);
        }
        if keep.is_empty() {
            return Err(TomeError::OverBudget {
                requested_pages: budget.max_open_pages as usize + 1,
                requested_bytes: 0,
                max_pages: budget.max_open_pages as usize,
                max_bytes: crate::contract::OPEN_MAX_BYTES,
            });
        }
        let passages = self.open(doc, &keep)?;
        Ok(Walk { chosen: keep, passages, judge_calls: calls, judge_prompt_tokens: None, visited })
    }

    fn backend_name(&self) -> &'static str {
        "fake"
    }
}

/// Deterministic judge for tests: scores a candidate by keyword overlap with the query.
pub struct KeywordJudge;

impl Judge for KeywordJudge {
    fn name(&self) -> &str {
        "keyword"
    }
    fn score(&self, query: &str, candidates: &[Candidate<'_>]) -> Result<Vec<u8>> {
        let q = query.to_lowercase();
        Ok(candidates
            .iter()
            .map(|c| {
                let t = format!("{} {}", c.title, c.lead).to_lowercase();
                let hits = t.split_whitespace().filter(|w| w.len() > 3 && q.contains(*w)).count();
                hits.min(3) as u8
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> DocId {
        DocId("Archmagus-Stack/09-Tomes/fake/Fake Book.pdf".into())
    }

    #[test]
    fn tree_cuts_at_depth_and_reports_child_count() {
        let t = FakeTome::sample();
        let roots = t.tree(&doc(), None, Some(1)).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots[1].children.is_empty());
        assert_eq!(roots[1].child_count, 2);
        let sub = t.tree(&doc(), Some(&NodeId("0002".into())), Some(1)).unwrap();
        assert_eq!(sub[1].id.0, "0002.0002");
    }

    #[test]
    fn errors_are_distinguishable_from_empty() {
        let t = FakeTome::sample();
        assert_eq!(t.tree(&DocId("nope.pdf".into()), None, None).unwrap_err().code(), "unknown_doc");
        assert_eq!(t.tree(&doc(), Some(&NodeId("0009".into())), None).unwrap_err().code(), "unknown_node");
        let ns = DocId("Archmagus-Stack/09-Tomes/fake/No Outline.pdf".into());
        assert_eq!(t.tree(&ns, None, None).unwrap_err().code(), "no_structure");
        assert_eq!(
            t.open(&doc(), &[NodeId("0001".into()), NodeId("0002".into())]).unwrap_err().code(),
            "over_budget"
        );
    }

    #[test]
    fn walk_descends_to_the_matching_leaf() {
        let t = FakeTome::sample();
        let w = t.walk(&doc(), "gamma details of beta", &KeywordJudge, Budget::default()).unwrap();
        assert!(w.chosen.iter().any(|n| n.0 == "0002.0002"), "{:?}", w.chosen);
        assert!(w.passages.iter().all(|p| (9..=20).contains(&p.page)));
        assert!(w.judge_calls >= 2);
    }
}
