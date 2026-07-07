//! Retrieval metrics for PersonaMem, scored by **content equality**.
//!
//! Unlike LongMemEval (gold = session ids), PersonaMem's gold is the evidence
//! turn *content* (`related_conversation_snippet`), which matches an ingested
//! message's content exactly. So a retrieved entry is "gold" iff its content is
//! in the gold-content set. Fully deterministic — no reader or judge LLM.

use std::collections::HashSet;

/// Deduplicate strings preserving first-seen order (a retrieved list may repeat
/// content across entries; collapse to first occurrence for rank cutoffs).
#[must_use]
pub fn distinct_in_order(items: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in items {
        if seen.insert(s.clone()) {
            out.push(s.clone());
        }
    }
    out
}

/// Recall@k: fraction of gold contents present in the top-k distinct retrieved
/// contents. `None` when there is no gold set (row excluded from averaging).
#[must_use]
pub fn recall_at_k(ranked_contents: &[String], gold: &HashSet<String>, k: usize) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let top_k: HashSet<String> = distinct_in_order(ranked_contents)
        .into_iter()
        .take(k)
        .collect();
    let hits = gold.iter().filter(|g| top_k.contains(*g)).count();
    Some(hits as f64 / gold.len() as f64)
}

/// Recall@budget: fraction of gold contents present among the contents that
/// survived a token-budgeted `query`. Order-independent.
#[must_use]
pub fn recall_in_set(retrieved: &HashSet<String>, gold: &HashSet<String>) -> Option<f64> {
    if gold.is_empty() {
        return None;
    }
    let hits = gold.iter().filter(|g| retrieved.contains(*g)).count();
    Some(hits as f64 / gold.len() as f64)
}

/// Running mean that ignores `None` samples (rows with no parseable gold).
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

    #[must_use]
    pub fn count(&self) -> usize {
        self.n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn recall_hits_gold_in_top_k() {
        let ranked = list(&["msg-a", "msg-b", "gold-1"]);
        assert_eq!(recall_at_k(&ranked, &set(&["gold-1"]), 3), Some(1.0));
        // gold not in top-2 → 0.
        assert_eq!(recall_at_k(&ranked, &set(&["gold-1"]), 2), Some(0.0));
    }

    #[test]
    fn recall_partial_multi_gold() {
        let ranked = list(&["g1", "x", "y"]);
        // two gold turns, one retrieved in top-3 → 0.5.
        assert_eq!(recall_at_k(&ranked, &set(&["g1", "g2"]), 3), Some(0.5));
    }

    #[test]
    fn recall_dedups_before_cutoff() {
        let ranked = list(&["dup", "dup", "dup", "gold"]);
        assert_eq!(recall_at_k(&ranked, &set(&["gold"]), 2), Some(1.0));
    }

    #[test]
    fn empty_gold_is_none() {
        let ranked = list(&["a"]);
        assert_eq!(recall_at_k(&ranked, &set(&[]), 5), None);
        assert_eq!(recall_in_set(&set(&["a"]), &set(&[])), None);
    }

    #[test]
    fn recall_in_set_ignores_order() {
        let retrieved = set(&["b", "gold", "a"]);
        assert_eq!(recall_in_set(&retrieved, &set(&["gold"])), Some(1.0));
    }

    #[test]
    fn mean_ignores_none() {
        let mut m = Mean::default();
        m.push(Some(1.0));
        m.push(None);
        m.push(Some(0.0));
        assert_eq!(m.count(), 2);
        assert!((m.get() - 0.5).abs() < 1e-9);
    }
}
