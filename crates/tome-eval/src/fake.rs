//! In-memory `TomeApi` over the real `tome_tree` types, for offline tests of the MCP
//! wiring and the harness. Fixtures are generated in code; no tome text is stored.

use std::collections::BTreeMap;

use crate::contract::{
    BEAM, Budget, Candidate, ChildTitle, DocId, DocMeta, Judge, Judged, Node, NodeId, NodeSource,
    OPEN_BYTE_CAP, OPEN_PAGE_CAP, Passage, Result, TomeApi, TomeError, Walk,
};

/// Doc id of the sample book (a fake sha256).
pub fn sample_doc() -> DocId {
    DocId::parse(&"a".repeat(64)).expect("valid hex")
}

/// Doc id that fails closed with `no_structure` (stands in for the Red Team Guide probe).
pub fn no_structure_doc() -> DocId {
    DocId::parse(&"b".repeat(64)).expect("valid hex")
}

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
    pub no_structure: Vec<DocId>,
}

fn node(id: &str, title: &str, level: u8, a: u32, b: u32, children: Vec<Node>) -> Node {
    Node {
        id: NodeId(id.into()),
        title: title.into(),
        level,
        page_start: a,
        page_end: b,
        lead: format!("{title} lead"),
        summary: format!("{title} lead"),
        source: NodeSource::Outline,
        child_count: children.len(),
        children,
    }
}

impl FakeTome {
    /// Two chapters, the second with two sections; 20 pages of generated text.
    pub fn sample() -> Self {
        let sha = sample_doc();
        let meta = DocMeta {
            doc_id: sha.clone(),
            path: "Archmagus-Stack/09-Tomes/fake/Fake Book.pdf".into(),
            sha256: sha.as_str().to_string(),
            pages: 20,
            outline: true,
            source: NodeSource::Outline,
            built_at: "2026-09-28T00:00:00Z".into(),
            builder_version: "fake-0".into(),
            summary_model: "fake/lead".into(),
            summary_temperature: 0.0,
        };
        let roots = vec![
            node(
                "0001",
                "Chapter 1. Alpha",
                1,
                1,
                8,
                vec![
                    node("0001.0001", "Alpha origins", 2, 1, 4, vec![]),
                    node("0001.0002", "Alpha practice", 2, 5, 8, vec![]),
                ],
            ),
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
        Self { docs: vec![FakeDoc { meta, roots, pages }], no_structure: vec![no_structure_doc()] }
    }

    fn doc(&self, doc: &DocId) -> Result<&FakeDoc> {
        if self.no_structure.contains(doc) {
            return Err(TomeError::NoStructure {
                doc: doc.as_str().to_string(),
                detail: "no outline, no headings".into(),
            });
        }
        self.docs
            .iter()
            .find(|d| &d.meta.doc_id == doc)
            .ok_or_else(|| TomeError::UnknownDoc { doc: doc.as_str().to_string() })
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
    c.child_count = n.children.len();
    c.children = if depth <= 1 { vec![] } else { n.children.iter().map(|k| cut(k, depth - 1)).collect() };
    c
}

impl TomeApi for FakeTome {
    fn docs(&self) -> Result<Vec<DocMeta>> {
        Ok(self.docs.iter().map(|d| d.meta.clone()).collect())
    }

    fn meta(&self, doc: &DocId) -> Result<DocMeta> {
        Ok(self.doc(doc)?.meta.clone())
    }

    fn tree(&self, doc: &DocId, node: Option<&NodeId>, depth: Option<u8>) -> Result<Vec<Node>> {
        let d = self.doc(doc)?;
        let depth = depth.unwrap_or(2).max(1);
        match node {
            None => Ok(d.roots.iter().map(|n| cut(n, depth)).collect()),
            Some(id) => {
                let n = find(&d.roots, id).ok_or_else(|| TomeError::UnknownNode {
                    doc: doc.as_str().to_string(),
                    node: id.0.clone(),
                })?;
                Ok(n.children.iter().map(|k| cut(k, depth)).collect())
            }
        }
    }

    fn open(&self, doc: &DocId, nodes: &[NodeId]) -> Result<Vec<Passage>> {
        let d = self.doc(doc)?;
        if nodes.is_empty() {
            return Err(TomeError::UnknownNode { doc: doc.as_str().to_string(), node: "(none)".into() });
        }
        let mut want = Vec::new();
        for id in nodes {
            let n = find(&d.roots, id).ok_or_else(|| TomeError::UnknownNode {
                doc: doc.as_str().to_string(),
                node: id.0.clone(),
            })?;
            for p in n.page_start..=n.page_end {
                want.push((id.clone(), p));
            }
        }
        let bytes: usize = want.iter().map(|(_, p)| d.pages.get(p).map_or(0, String::len)).sum();
        if want.len() as u32 > OPEN_PAGE_CAP || bytes > OPEN_BYTE_CAP {
            return Err(TomeError::OverBudget {
                detail: format!("{} pages / {bytes} bytes > {OPEN_PAGE_CAP} / {OPEN_BYTE_CAP}", want.len()),
            });
        }
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

    /// Same contract as the library walk: beam `BEAM`, stop at a leaf or ≤ 3 pages,
    /// judge errors stop the walk, over the page budget is `over_budget`, and every
    /// assessed child is in `judged`. Roots are not scored (no root pass), so
    /// `root_judge_calls` is 0. The real root pass is covered by the harness test
    /// over a built `TomeIndex`.
    fn walk(&self, doc: &DocId, query: &str, judge: &dyn Judge, budget: Budget) -> Result<Walk> {
        let d = self.doc(doc)?;
        let mut frontier: Vec<&Node> = d.roots.iter().collect();
        let mut chosen: Vec<&Node> = Vec::new();
        let mut calls = 0u32;
        let mut judged = Vec::new();
        loop {
            let mut pool: Vec<(u8, &Node)> = Vec::new();
            let mut expandable = false;
            for n in frontier {
                if n.children.is_empty() || n.page_end - n.page_start < 3 {
                    chosen.push(n);
                    continue;
                }
                expandable = true;
                for c in &n.children {
                    if calls >= budget.max_judge_calls {
                        return Err(TomeError::OverBudget { detail: "judge call budget exhausted".into() });
                    }
                    let cand = Candidate {
                        id: c.id.clone(),
                        title: c.title.clone(),
                        lead: c.lead.clone(),
                        child_titles: c
                            .children
                            .iter()
                            .map(|k| ChildTitle {
                                title: k.title.clone(),
                                page_start: k.page_start,
                                page_end: k.page_end,
                            })
                            .collect(),
                        page_start: c.page_start,
                        page_end: c.page_end,
                        level: c.level,
                    };
                    let a = judge.assess(query, &cand)?;
                    calls += 1;
                    judged.push(Judged {
                        node_id: c.id.clone(),
                        title: c.title.clone(),
                        page_start: c.page_start,
                        page_end: c.page_end,
                        score: a.score,
                        confidence: a.confidence,
                        rank: a.score,
                    });
                    pool.push((a.score, c));
                }
            }
            if !expandable || pool.is_empty() {
                break;
            }
            pool.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
            frontier = pool.into_iter().take(BEAM).map(|(_, n)| n).collect();
        }
        let pages: u32 = chosen.iter().map(|n| n.page_end - n.page_start + 1).sum();
        if pages > budget.max_pages {
            return Err(TomeError::OverBudget {
                detail: format!("walk chose {pages} pages > {}", budget.max_pages),
            });
        }
        let ids: Vec<NodeId> = chosen.iter().map(|n| n.id.clone()).collect();
        let passages = self.open(doc, &ids)?;
        Ok(Walk {
            doc_id: doc.clone(),
            query: query.into(),
            nodes: ids,
            passages,
            judge_calls: calls,
            skipped: vec![],
            judged,
            root_judge_calls: 0,
            root_path: tome_tree::RootPath::Batch,
        })
    }

    fn backend_name(&self) -> &'static str {
        "fake"
    }
}

/// Deterministic judge for tests: scores a candidate by keyword overlap with the query.
pub struct KeywordJudge;

impl Judge for KeywordJudge {
    fn score(&self, query: &str, c: &Candidate) -> Result<u8> {
        let q = query.to_lowercase();
        let t = format!("{} {}", c.title, c.lead).to_lowercase();
        Ok(t.split_whitespace().filter(|w| w.len() > 3 && q.contains(*w)).count().min(3) as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_cuts_at_depth_and_reports_child_count() {
        let t = FakeTome::sample();
        let roots = t.tree(&sample_doc(), None, Some(1)).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots[1].children.is_empty());
        assert_eq!(roots[1].child_count, 2);
        let sub = t.tree(&sample_doc(), Some(&NodeId("0002".into())), Some(1)).unwrap();
        assert_eq!(sub[1].id.0, "0002.0002");
    }

    #[test]
    fn errors_are_distinguishable_from_empty() {
        let t = FakeTome::sample();
        assert_eq!(
            t.tree(&DocId::parse(&"c".repeat(64)).unwrap(), None, None).unwrap_err().code(),
            "unknown_doc"
        );
        assert_eq!(
            t.tree(&sample_doc(), Some(&NodeId("0009".into())), None).unwrap_err().code(),
            "unknown_node"
        );
        assert_eq!(t.tree(&no_structure_doc(), None, None).unwrap_err().code(), "no_structure");
        let both = [NodeId("0001".into()), NodeId("0002".into())];
        assert_eq!(t.open(&sample_doc(), &both).unwrap_err().code(), "over_budget");
    }

    #[test]
    fn walk_descends_to_the_matching_leaf() {
        let t = FakeTome::sample();
        let w = t.walk(&sample_doc(), "gamma details of beta", &KeywordJudge, Budget::default()).unwrap();
        assert!(w.nodes.iter().any(|n| n.0 == "0002.0002"), "{:?}", w.nodes);
        assert!(w.passages.iter().all(|p| (9..=20).contains(&p.page)));
        assert!(w.judge_calls >= 2);
    }

    #[test]
    fn walk_fails_closed_when_the_judge_is_unavailable() {
        let t = FakeTome::sample();
        let judge = tome_tree::FakeJudge::new(Vec::<(String, u8)>::new());
        let e = t.walk(&sample_doc(), "q", &judge, Budget::default()).unwrap_err();
        assert_eq!(e.code(), "judge_unavailable");
    }
}
