//! Retrieval metrics scored against LongMemEval's gold session annotations.
//!
//! All metrics operate on **session ids**: CF entries carry a native
//! `session_id` that we set to the haystack session id at ingest, so a
//! retrieved entry maps back to the session it came from with no provenance
//! tracking. These metrics are fully deterministic — no reader or judge LLM.

use std::collections::HashSet;

/// Deduplicate session ids preserving first-seen rank order.
///
/// `query` returns entries, and several entries can share a session; for a
/// session-level metric we collapse them to the session's best (earliest) rank.
#[must_use]
pub fn distinct_in_order(ranked: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ranked {
        if seen.insert(id.clone()) {
            out.push(id.clone());
        }
    }
    out
}

/// Recall@k: fraction of gold sessions that appear in the top-k distinct
/// retrieved sessions.
///
/// Returns `None` when there is no gold set (abstention questions) — the caller
/// should exclude those from averaging rather than count them as 0 or 1.
#[must_use]
pub fn recall_at_k(ranked_sessions: &[String], gold: &HashSet<String>, k: usize) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let top_k: HashSet<String> = distinct_in_order(ranked_sessions)
        .into_iter()
        .take(k)
        .collect();
    let hits = gold.iter().filter(|g| top_k.contains(*g)).count();
    Some(hits as f64 / gold.len() as f64)
}

/// NDCG@k with binary relevance (a session is relevant iff it is gold).
///
/// DCG sums `1/log2(rank+1)` over relevant sessions in the top-k; IDCG is the
/// same for the ideal ordering (all gold sessions ranked first). Returns `None`
/// for abstention questions.
#[must_use]
pub fn ndcg_at_k(ranked_sessions: &[String], gold: &HashSet<String>, k: usize) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let ranked = distinct_in_order(ranked_sessions);
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, s)| gold.contains(*s))
        .map(|(i, _)| 1.0 / ((i as f64) + 2.0).log2())
        .sum();
    // Ideal: min(k, |gold|) relevant items packed at the top ranks.
    let ideal_hits = k.min(gold.len());
    let idcg: f64 = (0..ideal_hits)
        .map(|i| 1.0 / ((i as f64) + 2.0).log2())
        .sum();
    if idcg == 0.0 {
        return Some(0.0);
    }
    Some(dcg / idcg)
}

/// Recall@budget: fraction of gold sessions represented among the sessions that
/// survived a token-budgeted `query`. Order does not matter here — the question
/// is whether the evidence made it into the assembled context at that budget.
#[must_use]
pub fn recall_in_set(retrieved_sessions: &HashSet<String>, gold: &HashSet<String>) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let hits = gold
        .iter()
        .filter(|g| retrieved_sessions.contains(*g))
        .count();
    Some(hits as f64 / gold.len() as f64)
}

/// Running mean that ignores `None` samples (abstention questions).
#[derive(Debug, Default, Clone)]
pub struct Mean {
    sum: f64,
    n: usize,
}

impl Mean {
    pub fn push(&mut self, sample: Option<f64>) {
        if let Some(v) = sample {
            self.sum += v;
            self.n += 1;
        }
    }

    #[must_use]
    pub fn get(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gold(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    fn ranked(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn distinct_preserves_first_seen_order() {
        let r = ranked(&["a", "b", "a", "c", "b"]);
        assert_eq!(distinct_in_order(&r), ranked(&["a", "b", "c"]));
    }

    #[test]
    fn recall_all_gold_in_top_k() {
        let r = ranked(&["a", "b", "c", "d"]);
        assert_eq!(recall_at_k(&r, &gold(&["a", "c"]), 4), Some(1.0));
    }

    #[test]
    fn recall_partial_and_k_cutoff() {
        let r = ranked(&["x", "y", "a"]);
        // gold {a,b}; top-2 is {x,y} → 0 hits.
        assert_eq!(recall_at_k(&r, &gold(&["a", "b"]), 2), Some(0.0));
        // top-3 includes a → 1 of 2.
        assert_eq!(recall_at_k(&r, &gold(&["a", "b"]), 3), Some(0.5));
    }

    #[test]
    fn recall_dedups_before_cutoff() {
        // Duplicate sessions must not consume top-k slots twice.
        let r = ranked(&["a", "a", "a", "g"]);
        assert_eq!(recall_at_k(&r, &gold(&["g"]), 2), Some(1.0));
    }

    #[test]
    fn abstention_returns_none() {
        let r = ranked(&["a"]);
        assert_eq!(recall_at_k(&r, &gold(&[]), 5), None);
        assert_eq!(ndcg_at_k(&r, &gold(&[]), 5), None);
    }

    #[test]
    fn ndcg_ideal_ordering_is_one() {
        let r = ranked(&["a", "b", "c"]);
        // Both gold at the very top → perfect NDCG.
        assert_eq!(ndcg_at_k(&r, &gold(&["a", "b"]), 3), Some(1.0));
    }

    #[test]
    fn ndcg_penalizes_lower_rank() {
        let r = ranked(&["x", "y", "a"]);
        let score = ndcg_at_k(&r, &gold(&["a"]), 3).unwrap();
        // a at rank 3 → 1/log2(4) = 0.5, ideal 1/log2(2) = 1.0.
        assert!((score - 0.5).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn mean_ignores_none() {
        let mut m = Mean::default();
        m.push(Some(1.0));
        m.push(None);
        m.push(Some(0.0));
        // Mean of the two Some samples; the None must not count as a zero.
        assert!((m.get() - 0.5).abs() < 1e-9);
    }
}
