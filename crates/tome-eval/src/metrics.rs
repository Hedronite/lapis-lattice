//! Page precision/recall, percentiles, run summary and the Marci §4 win rule.

use std::collections::BTreeMap;

use crate::record::{
    Arm, ArmSummary, Correctness, CorrectnessCounts, DocSummary, PageMetrics, PerQuestion, ResultRecord,
    Rule, Verdict,
};

pub fn page_metrics(cited: &[u32], gold: &[u32]) -> PageMetrics {
    let inter = cited.iter().filter(|p| gold.contains(p)).count();
    let near = cited.iter().filter(|p| gold.iter().any(|g| g.abs_diff(**p) <= 1)).count();
    PageMetrics {
        precision: (!cited.is_empty()).then(|| inter as f64 / cited.len() as f64),
        recall: (!gold.is_empty()).then(|| inter as f64 / gold.len() as f64),
        hit: inter > 0,
        precision_tol1: (!cited.is_empty()).then(|| near as f64 / cited.len() as f64),
    }
}

/// Nearest-rank percentile (p in 0..=100). `None` on empty input.
pub fn percentile(values: &[f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * v.len() as f64).ceil().max(1.0) as usize;
    Some(v[rank.min(v.len()) - 1])
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let v: Vec<f64> = values.collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

pub fn summarize_arm(records: &[&ResultRecord]) -> ArmSummary {
    let scored: Vec<&&ResultRecord> = records.iter().filter(|r| r.scored).collect();
    let mut s = ArmSummary { n: scored.len() as u32, ..ArmSummary::default() };
    let mut counts = CorrectnessCounts::default();
    let mut per_doc: BTreeMap<String, (u32, u32, Vec<f64>)> = BTreeMap::new();
    for r in &scored {
        if let Some(e) = &r.error {
            s.n_errors += 1;
            *s.errors_by_kind.entry(e.kind.clone()).or_insert(0) += 1;
            if e.kind == "empty" {
                s.silent_empties += 1;
            }
        }
        let d = per_doc.entry(r.doc.clone()).or_default();
        d.0 += 1;
        match r.correctness {
            Some(c) => {
                match c {
                    Correctness::Exact => counts.exact += 1,
                    Correctness::Partial => counts.partial += 1,
                    Correctness::Wrong => counts.wrong += 1,
                    Correctness::Error => counts.error += 1,
                }
                if matches!(c, Correctness::Exact | Correctness::Partial) {
                    d.1 += 1;
                }
                d.2.push(c.score() as f64);
            }
            None => counts.ungraded += 1,
        }
    }
    s.n_graded = counts.exact + counts.partial + counts.wrong + counts.error;
    s.correctness_counts = counts;
    s.mean_correctness = mean(scored.iter().filter_map(|r| r.correctness.map(|c| c.score() as f64)));
    s.citation_hit_rate = mean(scored.iter().map(|r| if r.page_metrics.hit { 1.0 } else { 0.0 }));
    s.mean_page_precision = mean(scored.iter().filter_map(|r| r.page_metrics.precision));
    s.mean_page_recall = mean(scored.iter().filter_map(|r| r.page_metrics.recall));
    let lat: Vec<f64> = scored.iter().map(|r| r.latency_ms.total).collect();
    s.latency_p50_ms = percentile(&lat, 50.0);
    s.latency_p95_ms = percentile(&lat, 95.0);
    s.mean_prompt_tokens = mean(scored.iter().map(|r| r.tokens.prompt_tokens as f64));
    s.mean_completion_tokens = mean(scored.iter().map(|r| r.tokens.completion_tokens as f64));
    s.mean_opened = mean(scored.iter().map(|r| r.opened.count as f64));
    s.per_doc = per_doc
        .into_iter()
        .map(|(k, (n, ep, sc))| {
            (k, DocSummary { n, exact_or_partial: ep, mean_correctness: mean(sc.into_iter()) })
        })
        .collect();
    s
}

pub fn per_question(records: &[ResultRecord]) -> Vec<PerQuestion> {
    let mut ids: Vec<String> = records.iter().filter(|r| r.scored).map(|r| r.question_id.clone()).collect();
    ids.dedup();
    ids.into_iter()
        .map(|id| {
            let pick = |arm: Arm| {
                records.iter().find(|r| r.question_id == id && r.arm == arm).and_then(|r| r.correctness)
            };
            let (b, t) = (pick(Arm::Baseline), pick(Arm::Tome));
            let outcome = match (b, t) {
                (Some(b), Some(t)) if t.score() > b.score() => "tome_win",
                (Some(b), Some(t)) if t.score() < b.score() => "baseline_win",
                (Some(_), Some(_)) => "tie",
                _ => "ungraded",
            };
            PerQuestion { question_id: id, baseline: b, tome: t, outcome: outcome.into() }
        })
        .collect()
}

fn ratio(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) if b > 0.0 => Some(a / b),
        _ => None,
    }
}

/// Marci plan §4: keep only if all five rules hold. Incomplete when anything is ungraded.
pub fn verdict(base: &ArmSummary, tome: &ArmSummary, pq: &[PerQuestion], smoke: bool) -> Verdict {
    let wins = pq.iter().filter(|q| q.outcome == "tome_win").count() as u32;
    let losses = pq.iter().filter(|q| q.outcome == "baseline_win").count() as u32;
    let ties = pq.iter().filter(|q| q.outcome == "tie").count() as u32;
    let ungraded = pq.iter().any(|q| q.outcome == "ungraded");
    let mut rules = Vec::new();
    let cd = match (tome.mean_correctness, base.mean_correctness) {
        (Some(t), Some(b)) => Some((t - b, t - b >= 0.2)),
        _ => None,
    };
    rules.push(Rule {
        id: "correctness_delta".into(),
        pass: cd.map(|c| c.1),
        detail: cd
            .map_or("ungraded".into(), |c| format!("tome - baseline = {:+.2} (need ≥ +0.20 on 0–2)", c.0)),
    });
    let hd = match (tome.citation_hit_rate, base.citation_hit_rate) {
        (Some(t), Some(b)) => Some((t - b, t - b >= 0.10)),
        _ => None,
    };
    rules.push(Rule {
        id: "citation_hit_delta".into(),
        pass: hd.map(|h| h.1),
        detail: hd.map_or("n/a".into(), |h| format!("{:+.0} pp (need ≥ +10 pp)", h.0 * 100.0)),
    });
    let mut worst = 0i64;
    for (doc, b) in &base.per_doc {
        let t = tome.per_doc.get(doc).map_or(0, |d| d.exact_or_partial) as i64;
        worst = worst.max(b.exact_or_partial as i64 - t);
    }
    rules.push(Rule {
        id: "per_pdf_regression".into(),
        pass: (!ungraded).then_some(worst <= 1),
        detail: format!("worst per-PDF regression = {worst} question(s) (max 1)"),
    });
    let lr = ratio(tome.latency_p50_ms, base.latency_p50_ms);
    rules.push(Rule {
        id: "latency_ratio".into(),
        pass: lr.map(|r| r <= 3.0),
        detail: lr.map_or("n/a".into(), |r| format!("p50 tome/baseline = {r:.2}× (max 3×)")),
    });
    let tb = |s: &ArmSummary| match (s.mean_prompt_tokens, s.mean_completion_tokens) {
        (Some(p), Some(c)) => Some(p + c),
        _ => None,
    };
    let tr = ratio(tb(tome), tb(base));
    rules.push(Rule {
        id: "token_ratio".into(),
        pass: tr.map(|r| r <= 3.0),
        detail: tr.map_or("n/a".into(), |r| format!("tokens/question tome/baseline = {r:.2}× (max 3×)")),
    });
    rules.push(Rule {
        id: "silent_empties".into(),
        pass: Some(tome.silent_empties == 0 && base.silent_empties == 0),
        detail: format!("tome {} / baseline {}", tome.silent_empties, base.silent_empties),
    });
    let decision = if smoke || ungraded || rules.iter().any(|r| r.pass.is_none()) {
        "incomplete"
    } else if rules.iter().all(|r| r.pass == Some(true)) {
        "keep"
    } else {
        "shelve"
    };
    Verdict { decision: decision.into(), wins, losses, ties, rules }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precision_recall_and_tolerance() {
        let m = page_metrics(&[86, 87], &[86]);
        assert_eq!(m.precision, Some(0.5));
        assert_eq!(m.recall, Some(1.0));
        assert!(m.hit);
        assert_eq!(m.precision_tol1, Some(1.0));
        let none = page_metrics(&[], &[3]);
        assert_eq!((none.precision, none.recall, none.hit), (None, Some(0.0), false));
    }

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), Some(10.0));
        assert_eq!(percentile(&v, 95.0), Some(19.0));
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7.0], 95.0), Some(7.0));
    }

    #[test]
    fn smoke_runs_never_decide() {
        let a = ArmSummary::default();
        let v = verdict(&a, &a, &[], true);
        assert_eq!(v.decision, "incomplete");
        assert_eq!(v.rules.len(), 6);
    }
}
