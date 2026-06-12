use std::collections::{HashMap, HashSet};

/// Configuration for cross-session recurrence scoring.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RecurrenceConfig {
    /// Minimum number of sessions that must contain a term.
    pub min_session_frequency: usize,
    /// Maximum allowed recurrence ratio (`SF / total_sessions`).
    pub max_session_ratio: f64,
    /// Terms always excluded from recurrence output.
    pub term_blocklist: HashSet<String>,
}

impl Default for RecurrenceConfig {
    fn default() -> Self {
        Self {
            min_session_frequency: 2,
            max_session_ratio: 0.8,
            term_blocklist: HashSet::new(),
        }
    }
}

/// Recurrence metrics for a term that passed filtering.
#[derive(Debug, Clone, PartialEq)]
pub struct RecurrenceResult {
    /// The term (unigram, bigram, or trigram).
    pub term: String,
    /// Number of sessions containing this term.
    pub session_frequency: usize,
    /// Session frequency normalized by total session count.
    pub recurrence_score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FilteredRecurrenceMetrics {
    session_frequency: usize,
    recurrence_score: f64,
}

/// Compute cross-session recurrence for terms.
///
/// Input is one term-count map per session. Presence is binary per session;
/// term count values inside each map are ignored.
#[must_use]
#[allow(
    clippy::implicit_hasher,
    clippy::cast_precision_loss,
    reason = "Public API and recurrence score type are intentionally fixed by design"
)]
pub fn compute_recurrence(
    session_term_maps: &[HashMap<String, usize>],
    config: &RecurrenceConfig,
) -> Vec<RecurrenceResult> {
    if session_term_maps.is_empty() {
        return Vec::new();
    }

    let total_sessions = session_term_maps.len();
    let mut session_frequency: HashMap<String, usize> = HashMap::new();

    for term_map in session_term_maps {
        for term in term_map.keys() {
            *session_frequency.entry(term.clone()).or_insert(0) += 1;
        }
    }

    let mut filtered: HashMap<String, FilteredRecurrenceMetrics> = HashMap::new();
    for (term, sf) in session_frequency {
        if config.term_blocklist.contains(&term) {
            continue;
        }

        if sf < config.min_session_frequency {
            continue;
        }

        let recurrence_score = sf as f64 / total_sessions as f64;
        if recurrence_score > config.max_session_ratio {
            continue;
        }

        filtered.insert(
            term,
            FilteredRecurrenceMetrics {
                session_frequency: sf,
                recurrence_score,
            },
        );
    }

    let mut suppressed_unigrams: HashSet<String> = HashSet::new();
    for term in filtered.keys() {
        if term.contains(' ') {
            for unigram in term.split_whitespace() {
                suppressed_unigrams.insert(unigram.to_string());
            }
        }
    }

    for unigram in &suppressed_unigrams {
        filtered.remove(unigram);
    }

    let mut results: Vec<RecurrenceResult> = filtered
        .into_iter()
        .map(|(term, metrics)| RecurrenceResult {
            term,
            session_frequency: metrics.session_frequency,
            recurrence_score: metrics.recurrence_score,
        })
        .collect();
    results.sort_by(|left, right| {
        right
            .recurrence_score
            .total_cmp(&left.recurrence_score)
            .then_with(|| left.term.cmp(&right.term))
    });

    results
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::{compute_recurrence, RecurrenceConfig};

    fn term_map(terms: &[&str]) -> HashMap<String, usize> {
        let mut map = HashMap::new();
        for term in terms {
            map.insert((*term).to_string(), 1);
        }
        map
    }

    fn default_config() -> RecurrenceConfig {
        RecurrenceConfig::default()
    }

    #[test]
    fn compute_recurrence_returns_empty_for_empty_input() {
        let config = default_config();
        let results = compute_recurrence(&[], &config);
        assert!(results.is_empty());
    }

    #[test]
    fn compute_recurrence_returns_empty_for_single_session_with_defaults() {
        let sessions = vec![term_map(&["context", "forge", "context forge"])];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results.is_empty());
    }

    #[test]
    fn compute_recurrence_counts_session_presence_not_raw_frequency() {
        let mut session_a = HashMap::new();
        session_a.insert("context".to_string(), 100);
        session_a.insert("signal".to_string(), 1);

        let mut session_b = HashMap::new();
        session_b.insert("context".to_string(), 1);

        let session_c = term_map(&["signal"]);

        let sessions = vec![session_a, session_b, session_c];
        let config = RecurrenceConfig {
            min_session_frequency: 1,
            max_session_ratio: 1.0,
            term_blocklist: HashSet::new(),
        };

        let results = compute_recurrence(&sessions, &config);

        let context = results
            .iter()
            .find(|result| result.term == "context")
            .expect("context should be present");
        let signal = results
            .iter()
            .find(|result| result.term == "signal")
            .expect("signal should be present");

        assert_eq!(context.session_frequency, 2);
        assert_eq!(signal.session_frequency, 2);
        assert!((context.recurrence_score - (2.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn compute_recurrence_excludes_terms_below_low_cutoff() {
        let sessions = vec![
            term_map(&["alpha"]),
            term_map(&["beta"]),
            term_map(&["alpha"]),
        ];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().all(|result| result.term != "beta"));
        assert!(results.iter().any(|result| result.term == "alpha"));
    }

    #[test]
    fn compute_recurrence_excludes_terms_above_high_cutoff() {
        let sessions = vec![
            term_map(&["context", "alpha"]),
            term_map(&["context", "beta"]),
            term_map(&["context", "gamma"]),
            term_map(&["context", "delta"]),
            term_map(&["context", "epsilon"]),
        ];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().all(|result| result.term != "context"));
    }

    #[test]
    fn compute_recurrence_includes_term_at_exact_high_cutoff() {
        let sessions = vec![
            term_map(&["threshold"]),
            term_map(&["threshold"]),
            term_map(&["threshold"]),
            term_map(&["threshold", "other"]),
            term_map(&["other"]),
        ];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        let threshold = results
            .iter()
            .find(|result| result.term == "threshold")
            .expect("threshold should pass at exact 0.8");
        assert_eq!(threshold.session_frequency, 4);
        assert!((threshold.recurrence_score - 0.8).abs() < 1e-12);
    }

    #[test]
    fn compute_recurrence_can_return_empty_when_band_pass_filters_everything() {
        let sessions = vec![term_map(&["always"]), term_map(&["always"])];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results.is_empty());
    }

    #[test]
    fn compute_recurrence_excludes_blocklisted_terms() {
        let sessions = vec![
            term_map(&["context", "signal"]),
            term_map(&["context"]),
            term_map(&["context"]),
        ];
        let config = RecurrenceConfig {
            min_session_frequency: 1,
            max_session_ratio: 1.0,
            term_blocklist: HashSet::from(["context".to_string()]),
        };

        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().all(|result| result.term != "context"));
    }

    #[test]
    fn compute_recurrence_blocklisted_ngram_does_not_suppress_unigrams() {
        let sessions = vec![
            term_map(&["home", "ausvar", "home ausvar"]),
            term_map(&["home", "ausvar", "home ausvar"]),
            term_map(&["home", "ausvar"]),
            term_map(&["home", "ausvar"]),
            term_map(&["home"]),
        ];

        let config = RecurrenceConfig {
            min_session_frequency: 1,
            max_session_ratio: 1.0,
            term_blocklist: HashSet::from(["home ausvar".to_string()]),
        };

        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().any(|result| result.term == "home"));
        assert!(results.iter().any(|result| result.term == "ausvar"));
        assert!(results.iter().all(|result| result.term != "home ausvar"));
    }

    #[test]
    fn compute_recurrence_suppresses_unigrams_when_trigram_survives() {
        let sessions = vec![
            term_map(&["context", "forge", "hub", "context forge hub"]),
            term_map(&["context", "forge", "hub", "context forge hub"]),
            term_map(&["context", "forge", "hub", "context forge hub"]),
            term_map(&["context", "forge", "hub"]),
            term_map(&["other"]),
        ];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results
            .iter()
            .any(|result| result.term == "context forge hub"));
        assert!(results.iter().all(|result| result.term != "context"));
        assert!(results.iter().all(|result| result.term != "forge"));
        assert!(results.iter().all(|result| result.term != "hub"));
    }

    #[test]
    fn compute_recurrence_keeps_bigrams_when_trigram_survives() {
        let sessions = vec![
            term_map(&["a", "b", "c", "a b", "b c", "a b c"]),
            term_map(&["a", "b", "c", "a b", "b c", "a b c"]),
            term_map(&["a", "b", "c", "a b", "b c", "a b c"]),
            term_map(&["a", "b", "c", "a b", "b c"]),
            term_map(&["other"]),
        ];
        let config = default_config();

        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().any(|result| result.term == "a b c"));
        assert!(results.iter().any(|result| result.term == "a b"));
        assert!(results.iter().any(|result| result.term == "b c"));
    }

    #[test]
    fn compute_recurrence_bigram_suppresses_constituent_unigrams() {
        let sessions = vec![
            term_map(&["token", "model", "token model"]),
            term_map(&["token", "model", "token model"]),
            term_map(&["token", "model"]),
            term_map(&["other"]),
        ];
        let config = default_config();
        let results = compute_recurrence(&sessions, &config);
        assert!(results.iter().any(|r| r.term == "token model"));
        assert!(results.iter().all(|r| r.term != "token"));
        assert!(results.iter().all(|r| r.term != "model"));
    }

    #[test]
    fn compute_recurrence_honors_configurable_thresholds() {
        let sessions = vec![
            term_map(&["rare", "common"]),
            term_map(&["common"]),
            term_map(&["common"]),
            term_map(&["common"]),
            term_map(&["common"]),
        ];

        let config = RecurrenceConfig {
            min_session_frequency: 1,
            max_session_ratio: 1.0,
            term_blocklist: HashSet::new(),
        };

        let results = compute_recurrence(&sessions, &config);
        let rare = results
            .iter()
            .find(|result| result.term == "rare")
            .expect("rare should be included with min_session_frequency=1");
        assert_eq!(rare.session_frequency, 1);
        assert!((rare.recurrence_score - 0.2).abs() < 1e-12);
    }

    #[test]
    fn compute_recurrence_sorts_by_score_desc_then_term_asc() {
        let sessions = vec![
            term_map(&["banana", "apple", "carrot"]),
            term_map(&["banana", "apple"]),
            term_map(&["banana"]),
            term_map(&["apple"]),
            term_map(&["carrot"]),
        ];

        let config = RecurrenceConfig {
            min_session_frequency: 1,
            max_session_ratio: 1.0,
            term_blocklist: HashSet::new(),
        };

        let results = compute_recurrence(&sessions, &config);
        let terms: Vec<&str> = results.iter().map(|result| result.term.as_str()).collect();
        assert_eq!(terms, vec!["apple", "banana", "carrot"]);
    }
}
