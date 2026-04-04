use std::collections::HashMap;

use crate::classification::{ClassifiedPassage, ImportanceCategory};
use crate::recurrence::RecurrenceResult;

/// Default importance recency half-life: 7 days in seconds.
const DEFAULT_IMPORTANCE_HALF_LIFE_SECS: f64 = 604_800.0;

/// Configuration for importance scoring.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ScoringConfig {
    /// Weight multiplier for Corrective passages (default: 1.5).
    pub corrective_weight: f64,
    /// Weight multiplier for Stateful passages (default: 1.2).
    pub stateful_weight: f64,
    /// Weight multiplier for Decisive passages (default: 1.3).
    pub decisive_weight: f64,
    /// Weight multiplier for Reinforcing passages (default: 1.0).
    pub reinforcing_weight: f64,
    /// Weight for uncategorized passages (default: 0.5).
    pub uncategorized_weight: f64,
    /// Half-life in seconds for importance recency decay (default: 604800 = 7 days).
    /// NOT the same as BM25's 72-hour half-life in core.
    /// Values ≤ 0 or non-finite fall back to `DEFAULT_IMPORTANCE_HALF_LIFE_SECS`.
    pub importance_half_life_secs: f64,
}

impl Default for ScoringConfig {
    fn default() -> Self {
        Self {
            corrective_weight: 1.5,
            stateful_weight: 1.2,
            decisive_weight: 1.3,
            reinforcing_weight: 1.0,
            uncategorized_weight: 0.5,
            importance_half_life_secs: DEFAULT_IMPORTANCE_HALF_LIFE_SECS,
        }
    }
}

/// A scored passage ready for context injection.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ImportanceSegment {
    /// The passage text for injection.
    pub text: String,
    /// Importance categories assigned to the passage.
    pub categories: Vec<ImportanceCategory>,
    /// Combined importance score.
    pub importance_score: f64,
    /// The highest recurrence score among triggering terms.
    pub recurrence_score: f64,
    /// Category weight used (max across categories).
    pub category_weight: f64,
    /// Recency factor applied.
    pub recency_factor: f64,
    /// High-recurrence terms that triggered extraction.
    pub triggering_terms: Vec<String>,
    /// Maximum number of sessions any triggering term appears in.
    pub session_frequency: usize,
    /// Session ID this passage belongs to.
    pub session_id: String,
    /// Source entry timestamp (Unix seconds).
    pub timestamp: i64,
    /// Estimated token count (`text.len().div_ceil(4)`).
    pub token_estimate: usize,
}

/// Score classified passages and produce ranked `ImportanceSegment` values.
///
/// 1. Filters out superseded passages
/// 2. Computes importance score for each remaining passage
/// 3. Sorts by score descending (ties broken by timestamp descending, then text ascending)
///
/// `recurrence_map`: maps term -> `RecurrenceResult` for score lookup.
/// `now_timestamp`: current time in Unix seconds (for recency calculation).
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "Timestamp/age conversion to f64 is intentional for scoring arithmetic"
)]
#[allow(
    clippy::implicit_hasher,
    reason = "HashMap default hasher is acceptable for in-memory scoring"
)]
pub fn score_passages(
    classified: &[ClassifiedPassage],
    recurrence_map: &HashMap<String, RecurrenceResult>,
    config: &ScoringConfig,
    now_timestamp: i64,
) -> Vec<ImportanceSegment> {
    let half_life =
        if config.importance_half_life_secs.is_finite() && config.importance_half_life_secs > 0.0 {
            config.importance_half_life_secs
        } else {
            DEFAULT_IMPORTANCE_HALF_LIFE_SECS
        };

    let mut segments: Vec<ImportanceSegment> = classified
        .iter()
        // NOTE: `superseded` means "superseded in at least one category" - a multi-category
        // passage may still be the latest representative of another category. Per-category
        // supersession tracking is a known improvement tracked separately.
        .filter(|passage| !passage.superseded)
        .map(|passage| {
            let recurrence_score = passage
                .triggering_terms
                .iter()
                .filter_map(|term| recurrence_map.get(term))
                .map(|result| result.recurrence_score)
                .max_by(f64::total_cmp)
                .unwrap_or(0.0);

            let session_frequency = passage
                .triggering_terms
                .iter()
                .filter_map(|term| recurrence_map.get(term))
                .map(|result| result.session_frequency)
                .max()
                .unwrap_or(0);

            let category_weight = category_weight(&passage.categories, config);

            let age_seconds = (now_timestamp - passage.timestamp).max(0) as f64;
            let recency_factor = recency_decay(age_seconds, half_life);
            let importance_score = recurrence_score * category_weight * recency_factor;

            ImportanceSegment {
                text: passage.text.clone(),
                categories: passage.categories.clone(),
                importance_score,
                recurrence_score,
                category_weight,
                recency_factor,
                triggering_terms: passage.triggering_terms.clone(),
                session_frequency,
                session_id: passage.session_id.clone(),
                timestamp: passage.timestamp,
                token_estimate: estimate_tokens(&passage.text),
            }
        })
        .collect();

    segments.sort_by(|left, right| {
        right
            .importance_score
            .total_cmp(&left.importance_score)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
            .then_with(|| left.text.cmp(&right.text))
    });

    segments
}

/// Greedy token-budget bin-packing by descending importance score.
///
/// Iterates segments in order (assumed pre-sorted by score descending).
/// Skips any segment whose `token_estimate` exceeds remaining budget.
/// Does NOT stop on first skip - continues looking for smaller segments that fit.
#[must_use]
pub fn pack_segments(
    segments: &[ImportanceSegment],
    token_budget: usize,
) -> Vec<ImportanceSegment> {
    let mut packed: Vec<ImportanceSegment> = Vec::new();
    let mut remaining_budget = token_budget;

    for segment in segments {
        if segment.token_estimate > remaining_budget {
            continue;
        }

        remaining_budget -= segment.token_estimate;
        packed.push(segment.clone());
    }

    packed
}

fn category_weight(categories: &[ImportanceCategory], config: &ScoringConfig) -> f64 {
    if categories.is_empty() {
        return config.uncategorized_weight;
    }

    categories
        .iter()
        .map(|category| match category {
            ImportanceCategory::Corrective => config.corrective_weight,
            ImportanceCategory::Stateful => config.stateful_weight,
            ImportanceCategory::Decisive => config.decisive_weight,
            ImportanceCategory::Reinforcing => config.reinforcing_weight,
        })
        .max_by(f64::total_cmp)
        .unwrap_or(config.uncategorized_weight)
}

/// Estimate token count from text length.
///
/// Mirrors `crates/core/src/engine.rs::estimate_tokens`.
/// Do NOT import from core — analysis must remain layer-independent.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Exponential recency decay.
///
/// Intentionally mirrors `crates/core/src/engine.rs::recency_decay`.
/// Do NOT import from core — analysis must remain layer-independent.
fn recency_decay(age_seconds: f64, half_life: f64) -> f64 {
    0.5_f64.powf(age_seconds / half_life)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{pack_segments, score_passages, ImportanceSegment, ScoringConfig};
    use crate::classification::{ClassifiedPassage, ImportanceCategory};
    use crate::recurrence::RecurrenceResult;

    const NOW: i64 = 2_000_000_000;

    fn make_passage(
        text: &str,
        categories: Vec<ImportanceCategory>,
        triggering_terms: Vec<&str>,
        timestamp: i64,
        superseded: bool,
    ) -> ClassifiedPassage {
        ClassifiedPassage {
            text: text.to_string(),
            categories,
            triggering_terms: triggering_terms
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
            session_id: "session-1".to_string(),
            timestamp,
            entity: None,
            value: None,
            entity_pair: None,
            superseded,
        }
    }

    fn make_recurrence_map(values: &[(&str, f64)]) -> HashMap<String, RecurrenceResult> {
        values
            .iter()
            .map(|(term, score)| {
                (
                    (*term).to_string(),
                    RecurrenceResult {
                        term: (*term).to_string(),
                        session_frequency: 1,
                        recurrence_score: *score,
                    },
                )
            })
            .collect()
    }

    fn default_config() -> ScoringConfig {
        ScoringConfig::default()
    }

    fn make_segment(text: &str, token_estimate: usize) -> ImportanceSegment {
        ImportanceSegment {
            text: text.to_string(),
            categories: Vec::new(),
            importance_score: 0.0,
            recurrence_score: 0.0,
            category_weight: 0.5,
            recency_factor: 1.0,
            triggering_terms: Vec::new(),
            session_frequency: 0,
            session_id: "session-1".to_string(),
            timestamp: NOW,
            token_estimate,
        }
    }

    #[test]
    fn score_passages_returns_empty_for_empty_input() {
        let recurrence_map = make_recurrence_map(&[("term", 0.5)]);
        let result = score_passages(&[], &recurrence_map, &default_config(), NOW);
        assert!(result.is_empty());
    }

    #[test]
    fn score_passages_returns_empty_when_all_passages_are_superseded() {
        let passages = vec![
            make_passage("a", Vec::new(), vec!["x"], NOW, true),
            make_passage("b", Vec::new(), vec!["x"], NOW, true),
            make_passage("c", Vec::new(), vec!["x"], NOW, true),
        ];

        let recurrence_map = make_recurrence_map(&[("x", 0.8)]);
        let result = score_passages(&passages, &recurrence_map, &default_config(), NOW);
        assert!(result.is_empty());
    }

    #[test]
    fn score_passages_single_uncategorized_uses_uncategorized_weight() {
        let passage = make_passage("abcd", Vec::new(), vec!["term"], NOW, false);
        let recurrence_map = make_recurrence_map(&[("term", 0.5)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert_eq!(result.len(), 1);

        let segment = &result[0];
        assert!((segment.recurrence_score - 0.5).abs() < 1e-12);
        assert!((segment.category_weight - 0.5).abs() < 1e-12);
        assert!((segment.recency_factor - 1.0).abs() < 1e-12);
        assert!((segment.importance_score - 0.25).abs() < 1e-12);
    }

    #[test]
    fn score_passages_multi_category_uses_max_weight() {
        let passage = make_passage(
            "text",
            vec![ImportanceCategory::Corrective, ImportanceCategory::Decisive],
            vec!["term"],
            NOW,
            false,
        );
        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        let segment = &result[0];
        assert!((segment.category_weight - 1.5).abs() < 1e-12);
    }

    #[test]
    fn score_passages_multi_term_uses_max_recurrence() {
        let passage = make_passage("text", Vec::new(), vec!["low", "high"], NOW, false);
        let recurrence_map = make_recurrence_map(&[("low", 0.5), ("high", 0.667)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        let segment = &result[0];
        assert!((segment.recurrence_score - 0.667).abs() < 1e-12);
    }

    #[test]
    fn score_passages_session_frequency_uses_max_across_terms() {
        let passage = make_passage("text", Vec::new(), vec!["term_a", "term_b"], NOW, false);

        let recurrence_map = HashMap::from([
            (
                "term_a".to_string(),
                RecurrenceResult {
                    term: "term_a".to_string(),
                    session_frequency: 2,
                    recurrence_score: 0.4,
                },
            ),
            (
                "term_b".to_string(),
                RecurrenceResult {
                    term: "term_b".to_string(),
                    session_frequency: 5,
                    recurrence_score: 0.6,
                },
            ),
        ]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].session_frequency, 5);
    }

    #[test]
    fn score_passages_session_frequency_zero_when_terms_absent_from_map() {
        let passage = make_passage("text", Vec::new(), vec!["missing_term"], NOW, false);
        let recurrence_map = make_recurrence_map(&[("other", 0.7)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].session_frequency, 0);
    }

    #[test]
    fn score_passages_missing_term_uses_zero_recurrence() {
        let passage = make_passage("text", Vec::new(), vec!["foo"], NOW, false);
        let recurrence_map = make_recurrence_map(&[("other", 0.9)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        let segment = &result[0];
        assert!((segment.recurrence_score - 0.0).abs() < 1e-12);
        assert!((segment.importance_score - 0.0).abs() < 1e-12);
    }

    #[test]
    fn score_passages_recency_factor_is_one_at_now() {
        let passage = make_passage("text", Vec::new(), vec!["term"], NOW, false);
        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert!((result[0].recency_factor - 1.0).abs() < 1e-12);
    }

    #[test]
    fn score_passages_recency_factor_is_half_at_half_life() {
        let half_life_secs = 604_800_i64;
        let passage = make_passage(
            "text",
            Vec::new(),
            vec!["term"],
            NOW - half_life_secs,
            false,
        );
        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert!((result[0].recency_factor - 0.5).abs() < 1e-12);
    }

    #[test]
    fn score_passages_recency_factor_is_quarter_at_two_half_lives() {
        let half_life_secs = 604_800_i64;
        let passage = make_passage(
            "text",
            Vec::new(),
            vec!["term"],
            NOW - (2 * half_life_secs),
            false,
        );
        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);

        let result = score_passages(&[passage], &recurrence_map, &default_config(), NOW);
        assert!((result[0].recency_factor - 0.25).abs() < 1e-12);
    }

    #[test]
    fn score_passages_uses_default_half_life_for_invalid_config() {
        let passage = make_passage("text", Vec::new(), vec!["term"], NOW - 604_800, false);
        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);

        let mut config = default_config();
        config.importance_half_life_secs = 0.0;
        let result_zero = score_passages(
            std::slice::from_ref(&passage),
            &recurrence_map,
            &config,
            NOW,
        );
        assert!((result_zero[0].recency_factor - 0.5).abs() < 1e-12);

        config.importance_half_life_secs = f64::NAN;
        let result_nan = score_passages(&[passage], &recurrence_map, &config, NOW);
        assert!((result_nan[0].recency_factor - 0.5).abs() < 1e-12);
    }

    #[test]
    fn score_passages_text_tiebreak_alphabetical_order() {
        // Both passages have identical scores (0.0 × 0.5 × recency = 0.0) and same timestamp
        // -> text ascending is the final tiebreaker
        let a = make_passage("alpha", Vec::new(), vec!["absent"], NOW - 10, false);
        let b = make_passage("beta", Vec::new(), vec!["absent"], NOW - 10, false);
        let result = score_passages(&[b, a], &make_recurrence_map(&[]), &default_config(), NOW);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "alpha");
        assert_eq!(result[1].text, "beta");
    }

    #[test]
    fn score_passages_timestamp_tiebreak_newer_wins() {
        // Both passages score 0.0 (term absent from map) -> tie on score -> timestamp decides
        let newer = make_passage("text", Vec::new(), vec!["absent"], NOW, false);
        let older = make_passage("text", Vec::new(), vec!["absent"], NOW - 100, false);
        let result = score_passages(
            &[older, newer],
            &make_recurrence_map(&[]),
            &default_config(),
            NOW,
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].timestamp, NOW);
        assert_eq!(result[1].timestamp, NOW - 100);
    }

    #[test]
    fn pack_segments_basic_greedy_pack() {
        let segments = vec![
            make_segment("one", 100),
            make_segment("two", 200),
            make_segment("three", 150),
        ];

        let packed = pack_segments(&segments, 350);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].text, "one");
        assert_eq!(packed[1].text, "two");
    }

    #[test]
    fn pack_segments_skip_then_fit() {
        let segments = vec![
            make_segment("first", 300),
            make_segment("second", 250),
            make_segment("third", 50),
        ];

        let packed = pack_segments(&segments, 350);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].text, "first");
        assert_eq!(packed[1].text, "third");
    }

    #[test]
    fn pack_segments_zero_budget_returns_empty() {
        let segments = vec![make_segment("one", 10)];
        let packed = pack_segments(&segments, 0);
        assert!(packed.is_empty());
    }

    #[test]
    fn score_passages_mixed_superseded_keeps_only_active() {
        let passages = vec![
            make_passage("s1", Vec::new(), vec!["term"], NOW, true),
            make_passage("a1", Vec::new(), vec!["term"], NOW, false),
            make_passage("s2", Vec::new(), vec!["term"], NOW, true),
            make_passage("a2", Vec::new(), vec!["term"], NOW - 1, false),
            make_passage("a3", Vec::new(), vec!["term"], NOW - 2, false),
        ];

        let recurrence_map = make_recurrence_map(&[("term", 1.0)]);
        let result = score_passages(&passages, &recurrence_map, &default_config(), NOW);

        assert_eq!(result.len(), 3);
        assert!(result.iter().all(|segment| !segment.text.starts_with('s')));
    }

    #[test]
    fn score_passages_excludes_superseded_even_if_it_would_score_highest() {
        let passages = vec![
            make_passage("highest", Vec::new(), vec!["hot"], NOW, true),
            make_passage("active", Vec::new(), vec!["cool"], NOW - 1_000, false),
        ];
        let recurrence_map = make_recurrence_map(&[("hot", 1.0), ("cool", 0.1)]);

        let result = score_passages(&passages, &recurrence_map, &default_config(), NOW);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].text, "active");
    }
}
