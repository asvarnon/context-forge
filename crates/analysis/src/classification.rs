use std::collections::{HashMap, HashSet};

use crate::lexicon::Lexicons;

/// Classification configuration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ClassificationConfig {
    /// Word proximity window for corrective detection (default: 5).
    pub corrective_proximity: usize,
    /// Minimum sessions for reinforcing classification (default: 3).
    pub reinforcing_min_sessions: usize,
    /// Minimum bigram overlap ratio for reinforcing (default: 0.6).
    pub reinforcing_overlap_threshold: f64,
}

impl Default for ClassificationConfig {
    fn default() -> Self {
        Self {
            corrective_proximity: 5,
            reinforcing_min_sessions: 3,
            reinforcing_overlap_threshold: 0.6,
        }
    }
}

/// Context about a passage needed for classification.
/// Keeps classification decoupled from core types.
#[derive(Debug, Clone)]
pub struct PassageContext {
    /// The passage text.
    pub passage_text: String,
    /// High-recurrence terms that triggered extraction of this passage.
    pub triggering_terms: Vec<String>,
    /// Session ID this passage belongs to.
    pub session_id: String,
    /// Source entry timestamp (Unix seconds).
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImportanceCategory {
    Corrective,
    Stateful,
    Decisive,
    Reinforcing,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ClassifiedPassage {
    pub text: String,
    pub categories: Vec<ImportanceCategory>,
    pub triggering_terms: Vec<String>,
    pub session_id: String,
    pub timestamp: i64,
    /// Extracted entity for stateful/decisive categories.
    pub entity: Option<String>,
    /// Extracted value for stateful category.
    pub value: Option<String>,
    /// Second entity for decisive category (entity pair).
    pub entity_pair: Option<(String, String)>,
    /// Whether this passage has been superseded by a newer one.
    ///
    /// Supersession is evaluated per-category (stateful or decisive),
    /// but this flag represents "superseded in at least one category."
    /// A multi-category passage marked superseded may still be the
    /// latest representative of another category it belongs to.
    /// Consumers should check category-specific supersession if needed.
    pub superseded: bool,
}

/// Classify passages into importance categories and apply supersession.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    reason = "Overlap ratio uses f64 by design for threshold comparisons"
)]
pub fn classify_passages(
    passages: &[PassageContext],
    lexicons: &Lexicons,
    config: &ClassificationConfig,
) -> Vec<ClassifiedPassage> {
    if passages.is_empty() {
        return Vec::new();
    }

    let mut classified: Vec<ClassifiedPassage> = passages
        .iter()
        .map(|passage| {
            let mut categories: Vec<ImportanceCategory> = Vec::new();

            if is_corrective(passage, lexicons, config) {
                categories.push(ImportanceCategory::Corrective);
            }

            let state_match = detect_stateful(passage, lexicons);
            let (entity, value) = if let Some((state_entity, state_value)) = state_match {
                categories.push(ImportanceCategory::Stateful);
                (Some(state_entity), Some(state_value))
            } else {
                (None, None)
            };

            let entity_pair = if is_decisive(passage, lexicons) {
                categories.push(ImportanceCategory::Decisive);
                extract_entity_pair(passage)
            } else {
                None
            };

            ClassifiedPassage {
                text: passage.passage_text.clone(),
                categories,
                triggering_terms: passage.triggering_terms.clone(),
                session_id: passage.session_id.clone(),
                timestamp: passage.timestamp,
                entity,
                value,
                entity_pair,
                superseded: false,
            }
        })
        .collect();

    apply_reinforcing(&mut classified, lexicons, config);
    apply_supersession(&mut classified);
    classified
}

fn is_corrective(
    passage: &PassageContext,
    lexicons: &Lexicons,
    config: &ClassificationConfig,
) -> bool {
    if passage.passage_text.trim_end().ends_with('?') {
        return false;
    }

    let words = tokenize_words(&passage.passage_text.to_lowercase());
    if words.is_empty() {
        return false;
    }

    let negation_positions: Vec<usize> = lexicons
        .negation_markers
        .iter()
        .flat_map(|marker| find_marker_positions(&words, &marker.to_lowercase()))
        .collect();

    if negation_positions.is_empty() {
        return false;
    }

    let triggering_positions: Vec<usize> = passage
        .triggering_terms
        .iter()
        .flat_map(|term| find_term_positions(&words, &term.to_lowercase()))
        .collect();

    if triggering_positions.is_empty() {
        return false;
    }

    for negation_pos in &negation_positions {
        for term_pos in &triggering_positions {
            if usize::abs_diff(*negation_pos, *term_pos) <= config.corrective_proximity {
                return true;
            }
        }
    }

    false
}

fn detect_stateful(passage: &PassageContext, lexicons: &Lexicons) -> Option<(String, String)> {
    let passage_lower = passage.passage_text.to_lowercase();
    let mut matches: Vec<(usize, usize, String)> = Vec::new();
    for operator in &lexicons.state_operators {
        let operator_lower = operator.to_lowercase();
        for (start_index, _) in passage_lower.match_indices(&operator_lower) {
            let end_index = start_index + operator_lower.len();
            let has_start_boundary = start_index == 0
                || !passage_lower.as_bytes()[start_index - 1].is_ascii_alphanumeric();
            let has_end_boundary = end_index >= passage_lower.len()
                || !passage_lower.as_bytes()[end_index].is_ascii_alphanumeric();

            if has_start_boundary && has_end_boundary {
                matches.push((start_index, operator_lower.len(), operator_lower.clone()));
            }
        }
    }

    matches.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)));

    for (start_index, operator_len, _) in matches {
        let before = &passage_lower[..start_index];
        let after = &passage_lower[start_index + operator_len..];

        let entity_words = tokenize_words(before);
        let value_words = tokenize_words(after);

        if entity_words.is_empty() || value_words.is_empty() {
            continue;
        }

        let entity_start = entity_words.len().saturating_sub(4);
        let entity = entity_words[entity_start..].join(" ");
        if !contains_ascii_alpha(&entity) {
            continue;
        }

        let value = value_words
            .into_iter()
            .take(6)
            .collect::<Vec<String>>()
            .join(" ");
        if value.is_empty() {
            continue;
        }

        return Some((entity, value));
    }

    None
}

fn is_decisive(passage: &PassageContext, lexicons: &Lexicons) -> bool {
    let passage_lower = passage.passage_text.to_lowercase();
    let has_comparison = has_marker_in_passage(&passage_lower, &lexicons.comparison_markers);
    let has_causal = has_marker_in_passage(&passage_lower, &lexicons.causal_connectors);

    has_comparison && has_causal
}

fn has_marker_in_passage(passage_lower: &str, markers: &[String]) -> bool {
    let words: Vec<String> = passage_lower
        .split_whitespace()
        .map(clean_for_comparison)
        .filter(|word| !word.is_empty())
        .collect();

    for marker in markers {
        let marker_lower = marker.to_lowercase();
        if marker_lower.contains(' ') {
            if passage_lower.contains(&marker_lower) {
                return true;
            }
        } else if words.iter().any(|word| word == &marker_lower) {
            return true;
        }
    }

    false
}

fn extract_entity_pair(passage: &PassageContext) -> Option<(String, String)> {
    let mut capitalized: Vec<String> = Vec::new();
    let mut at_sentence_start = true;

    for raw_word in passage.passage_text.split_whitespace() {
        let cleaned = clean_token(raw_word);
        if cleaned.is_empty() {
            at_sentence_start = raw_word_ends_sentence(raw_word);
            continue;
        }

        if !at_sentence_start
            && cleaned
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_uppercase())
            && !capitalized.contains(&cleaned)
        {
            capitalized.push(cleaned.clone());
        }

        at_sentence_start = raw_word_ends_sentence(raw_word);
    }

    if capitalized.len() >= 2 {
        return Some((capitalized[0].clone(), capitalized[1].clone()));
    }

    let mut proxies: Vec<String> = Vec::new();
    for term in &passage.triggering_terms {
        let trimmed = term.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !proxies.contains(&trimmed.to_string()) {
            proxies.push(trimmed.to_string());
        }
    }

    if proxies.len() >= 2 {
        return Some((proxies[0].clone(), proxies[1].clone()));
    }

    None
}

#[allow(
    clippy::cast_precision_loss,
    clippy::implicit_hasher,
    reason = "Threshold math uses f64 and HashMap defaults are acceptable for local grouping"
)]
fn apply_reinforcing(
    passages: &mut [ClassifiedPassage],
    lexicons: &Lexicons,
    config: &ClassificationConfig,
) {
    let confirmation_sets: Vec<HashSet<String>> = passages
        .iter()
        .map(|passage| confirmation_tokens_in_passage(&passage.text, lexicons))
        .collect();

    let candidate_indices: Vec<usize> = confirmation_sets
        .iter()
        .enumerate()
        .filter_map(|(index, tokens)| if tokens.is_empty() { None } else { Some(index) })
        .collect();

    if candidate_indices.len() < config.reinforcing_min_sessions {
        return;
    }

    let mut graph: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in &candidate_indices {
        graph.insert(*index, Vec::new());
    }

    for left_index in 0..candidate_indices.len() {
        for right_index in (left_index + 1)..candidate_indices.len() {
            let left = candidate_indices[left_index];
            let right = candidate_indices[right_index];

            if !shares_confirmation_token(&confirmation_sets[left], &confirmation_sets[right]) {
                continue;
            }

            let overlap = triggering_overlap_ratio(
                &passages[left].triggering_terms,
                &passages[right].triggering_terms,
            );
            if overlap > config.reinforcing_overlap_threshold {
                if let Some(neighbors) = graph.get_mut(&left) {
                    neighbors.push(right);
                }
                if let Some(neighbors) = graph.get_mut(&right) {
                    neighbors.push(left);
                }
            }
        }
    }

    let mut visited: HashSet<usize> = HashSet::new();
    for index in candidate_indices {
        if visited.contains(&index) {
            continue;
        }

        let mut stack: Vec<usize> = vec![index];
        let mut component: Vec<usize> = Vec::new();

        while let Some(current) = stack.pop() {
            if !visited.insert(current) {
                continue;
            }

            component.push(current);
            if let Some(neighbors) = graph.get(&current) {
                for neighbor in neighbors {
                    if !visited.contains(neighbor) {
                        stack.push(*neighbor);
                    }
                }
            }
        }

        let distinct_sessions: HashSet<&str> = component
            .iter()
            .map(|component_index| passages[*component_index].session_id.as_str())
            .collect();

        if distinct_sessions.len() >= config.reinforcing_min_sessions {
            for component_index in component {
                if !passages[component_index]
                    .categories
                    .contains(&ImportanceCategory::Reinforcing)
                {
                    passages[component_index]
                        .categories
                        .push(ImportanceCategory::Reinforcing);
                }
            }
        }
    }
}

fn apply_supersession(passages: &mut [ClassifiedPassage]) {
    let mut stateful_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, passage) in passages.iter().enumerate() {
        if passage.categories.contains(&ImportanceCategory::Stateful) {
            if let Some(entity) = &passage.entity {
                let key = entity.trim().to_lowercase();
                if !key.is_empty() {
                    stateful_groups.entry(key).or_default().push(index);
                }
            }
        }
    }

    mark_group_superseded(passages, stateful_groups.values());

    let mut decisive_groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, passage) in passages.iter().enumerate() {
        if passage.categories.contains(&ImportanceCategory::Decisive) {
            if let Some((left, right)) = &passage.entity_pair {
                let mut pair = [left.trim().to_lowercase(), right.trim().to_lowercase()];
                pair.sort();
                decisive_groups
                    .entry((pair[0].clone(), pair[1].clone()))
                    .or_default()
                    .push(index);
            }
        }
    }

    mark_group_superseded(passages, decisive_groups.values());
}

fn mark_group_superseded<'a>(
    passages: &mut [ClassifiedPassage],
    groups: impl Iterator<Item = &'a Vec<usize>>,
) {
    for indices in groups {
        if indices.len() <= 1 {
            continue;
        }

        let latest = indices.iter().copied().max_by(|left, right| {
            passages[*left]
                .timestamp
                .cmp(&passages[*right].timestamp)
                // Tie-broken by input order: later index wins.
                .then_with(|| left.cmp(right))
        });

        if let Some(latest_index) = latest {
            for index in indices {
                if *index != latest_index {
                    passages[*index].superseded = true;
                }
            }
        }
    }
}

fn confirmation_tokens_in_passage(text: &str, lexicons: &Lexicons) -> HashSet<String> {
    let words = tokenize_words(&text.to_lowercase());
    let mut matched: HashSet<String> = HashSet::new();

    for token in &lexicons.confirmation_tokens {
        let token_lower = token.to_lowercase();
        if words.iter().any(|word| word == &token_lower) {
            matched.insert(token_lower);
        }
    }

    matched
}

fn shares_confirmation_token(left: &HashSet<String>, right: &HashSet<String>) -> bool {
    left.iter().any(|token| right.contains(token))
}

#[allow(
    clippy::cast_precision_loss,
    reason = "Overlap ratio uses f64 thresholding by configuration contract"
)]
fn triggering_overlap_ratio(left_terms: &[String], right_terms: &[String]) -> f64 {
    let left: HashSet<String> = left_terms
        .iter()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    let right: HashSet<String> = right_terms
        .iter()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();

    let union_count = left.union(&right).count();
    if union_count == 0 {
        return 0.0;
    }

    let intersection_count = left.intersection(&right).count();
    intersection_count as f64 / union_count as f64
}

fn find_marker_positions(words: &[String], marker: &str) -> Vec<usize> {
    let marker_words: Vec<&str> = marker.split_whitespace().collect();
    if marker_words.is_empty() {
        return Vec::new();
    }

    if marker_words.len() == 1 {
        return words
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                if *word == marker_words[0] {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
    }

    find_phrase_positions(words, &marker_words)
}

fn find_term_positions(words: &[String], term: &str) -> Vec<usize> {
    let term_words: Vec<&str> = term.split_whitespace().collect();
    if term_words.is_empty() {
        return Vec::new();
    }

    if term_words.len() == 1 {
        return words
            .iter()
            .enumerate()
            .filter_map(|(index, word)| {
                if *word == term_words[0] {
                    Some(index)
                } else {
                    None
                }
            })
            .collect();
    }

    find_phrase_positions(words, &term_words)
}

fn find_phrase_positions(words: &[String], phrase_words: &[&str]) -> Vec<usize> {
    if words.len() < phrase_words.len() {
        return Vec::new();
    }

    let mut positions: Vec<usize> = Vec::new();
    for index in 0..=(words.len() - phrase_words.len()) {
        let mut matches = true;
        for (offset, phrase_word) in phrase_words.iter().enumerate() {
            if words[index + offset] != *phrase_word {
                matches = false;
                break;
            }
        }
        if matches {
            positions.push(index);
        }
    }

    positions
}

fn tokenize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(clean_token)
        .filter(|token| !token.is_empty())
        .collect()
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '\'' | '-' | '_' | ':' | '=')
        })
        .to_string()
}

fn clean_for_comparison(token: &str) -> String {
    clean_token(token).to_lowercase()
}

fn contains_ascii_alpha(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_ascii_alphabetic())
}

fn raw_word_ends_sentence(raw_word: &str) -> bool {
    raw_word
        .trim_end_matches(['"', '\'', ')', ']'])
        .ends_with(['.', '!', '?'])
}

#[cfg(test)]
mod tests {
    use super::{classify_passages, ClassificationConfig, ImportanceCategory, PassageContext};
    use crate::lexicon::Lexicons;

    fn passage(text: &str, terms: &[&str], session_id: &str, timestamp: i64) -> PassageContext {
        PassageContext {
            passage_text: text.to_string(),
            triggering_terms: terms.iter().map(|term| (*term).to_string()).collect(),
            session_id: session_id.to_string(),
            timestamp,
        }
    }

    fn default_config() -> ClassificationConfig {
        ClassificationConfig::default()
    }

    fn has_category(passage: &super::ClassifiedPassage, category: ImportanceCategory) -> bool {
        passage.categories.contains(&category)
    }

    #[test]
    fn empty_passages_returns_empty_vec() {
        let lexicons = Lexicons::default();
        let config = default_config();

        let result = classify_passages(&[], &lexicons, &config);
        assert!(result.is_empty());
    }

    #[test]
    fn no_category_match_has_empty_categories() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "General context discussion about implementation details.",
            &["context"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert_eq!(result.len(), 1);
        assert!(result[0].categories.is_empty());
        assert!(!result[0].superseded);
    }

    #[test]
    fn corrective_detection_matches_negation_near_trigger_term() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "We should not enable cache in production.",
            &["cache"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Corrective));
    }

    #[test]
    fn corrective_detection_skips_questions() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "Should we not use cache?",
            &["cache"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(!has_category(&result[0], ImportanceCategory::Corrective));
    }

    #[test]
    fn corrective_detection_proximity_boundary() {
        let lexicons = Lexicons::default();
        let config = ClassificationConfig {
            corrective_proximity: 3,
            ..ClassificationConfig::default()
        };

        let within = passage("not alpha beta cache", &["cache"], "session-a", 10);
        let beyond = passage("not alpha beta gamma cache", &["cache"], "session-b", 20);

        let result = classify_passages(&[within, beyond], &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Corrective));
        assert!(!has_category(&result[1], ImportanceCategory::Corrective));
    }

    #[test]
    fn stateful_detection_matches_explicit_operators() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![
            passage("Cache mode set to writeback", &["cache"], "session-a", 10),
            passage("Timeout changed to 30", &["timeout"], "session-a", 11),
            passage("PORT = 8080", &["port"], "session-a", 12),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Stateful));
        assert!(has_category(&result[1], ImportanceCategory::Stateful));
        assert!(has_category(&result[2], ImportanceCategory::Stateful));
    }

    #[test]
    fn stateful_rejects_bare_is() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "Context is important for robust agents.",
            &["context"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(!has_category(&result[0], ImportanceCategory::Stateful));
    }

    #[test]
    fn stateful_entity_requires_alphabetic_characters() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage("1234 = 5678", &["1234"], "session-a", 10)];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(!has_category(&result[0], ImportanceCategory::Stateful));
    }

    #[test]
    fn stateful_supersession_marks_older_passage() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![
            passage("Cache mode set to writeback", &["cache"], "session-a", 100),
            passage(
                "Cache mode set to writethrough",
                &["cache"],
                "session-b",
                200,
            ),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Stateful));
        assert!(has_category(&result[1], ImportanceCategory::Stateful));
        assert!(result[0].superseded);
        assert!(!result[1].superseded);
    }

    #[test]
    fn stateful_supersession_equal_timestamps() {
        let lexicons = Lexicons::default();
        let config = ClassificationConfig::default();
        let passages = vec![
            PassageContext {
                passage_text: "Server IP: changed to 10.0.0.1".to_string(),
                triggering_terms: vec!["server".to_string()],
                session_id: "s1".to_string(),
                timestamp: 100,
            },
            PassageContext {
                passage_text: "Server IP: changed to 10.0.0.2".to_string(),
                triggering_terms: vec!["server".to_string()],
                session_id: "s2".to_string(),
                timestamp: 100,
            },
        ];
        let result = classify_passages(&passages, &lexicons, &config);
        let stateful: Vec<_> = result
            .iter()
            .filter(|passage| passage.categories.contains(&ImportanceCategory::Stateful))
            .collect();
        assert_eq!(stateful.len(), 2);
        let superseded_count = result.iter().filter(|passage| passage.superseded).count();
        assert_eq!(superseded_count, 1);
    }

    #[test]
    fn decisive_detection_requires_comparison_and_causal() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "We switched from Redis to Memcached because latency dropped.",
            &["redis", "memcached"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Decisive));
    }

    #[test]
    fn decisive_requires_both_signals() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let comparison_only = passage(
            "We switched from Redis to Memcached yesterday.",
            &["redis", "memcached"],
            "session-a",
            10,
        );
        let causal_only = passage(
            "Latency dropped because pipeline tuning improved.",
            &["latency", "pipeline"],
            "session-b",
            11,
        );

        let result = classify_passages(&[comparison_only, causal_only], &lexicons, &config);
        assert!(!has_category(&result[0], ImportanceCategory::Decisive));
        assert!(!has_category(&result[1], ImportanceCategory::Decisive));
    }

    #[test]
    fn decisive_supersession_marks_older_entity_pair() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![
            passage(
                "We switched from Redis to Memcached because latency improved.",
                &["redis", "memcached"],
                "session-a",
                10,
            ),
            passage(
                "We switched from Memcached to Redis because cache misses rose.",
                &["memcached", "redis"],
                "session-b",
                20,
            ),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Decisive));
        assert!(has_category(&result[1], ImportanceCategory::Decisive));
        assert!(result[0].superseded);
        assert!(!result[1].superseded);
    }

    #[test]
    fn reinforcing_detection_with_three_sessions() {
        let lexicons = Lexicons::default();
        let config = ClassificationConfig {
            reinforcing_min_sessions: 3,
            reinforcing_overlap_threshold: 0.6,
            ..ClassificationConfig::default()
        };
        let inputs = vec![
            passage(
                "Yes confirmed cache policy works",
                &["cache", "policy"],
                "s1",
                1,
            ),
            passage("Yes cache policy still good", &["cache", "policy"], "s2", 2),
            passage(
                "Confirmed cache policy remains stable",
                &["cache", "policy"],
                "s3",
                3,
            ),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(result
            .iter()
            .all(|passage| has_category(passage, ImportanceCategory::Reinforcing)));
    }

    #[test]
    fn reinforcing_below_threshold_with_two_sessions() {
        let lexicons = Lexicons::default();
        let config = ClassificationConfig {
            reinforcing_min_sessions: 3,
            reinforcing_overlap_threshold: 0.6,
            ..ClassificationConfig::default()
        };
        let inputs = vec![
            passage("Yes cache policy is fine", &["cache", "policy"], "s1", 1),
            passage(
                "Confirmed cache policy is fine",
                &["cache", "policy"],
                "s2",
                2,
            ),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(result
            .iter()
            .all(|passage| !has_category(passage, ImportanceCategory::Reinforcing)));
    }

    #[test]
    fn multi_category_passage_can_be_corrective_and_decisive() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "We should not use Redis and switched to Memcached because costs dropped.",
            &["redis", "memcached"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Corrective));
        assert!(has_category(&result[0], ImportanceCategory::Decisive));
    }

    #[test]
    fn stateful_is_now_operator_matches() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![passage(
            "Primary database is now PostgreSQL 16",
            &["database"],
            "session-a",
            10,
        )];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(has_category(&result[0], ImportanceCategory::Stateful));
    }

    #[test]
    fn state_operators_are_phrases_set_alone_does_not_match() {
        let lexicons = Lexicons::default();
        let config = default_config();
        let inputs = vec![
            passage("We set cache flags manually", &["cache"], "session-a", 10),
            passage("Cache is set to strict mode", &["cache"], "session-b", 11),
        ];

        let result = classify_passages(&inputs, &lexicons, &config);
        assert!(!has_category(&result[0], ImportanceCategory::Stateful));
        assert!(has_category(&result[1], ImportanceCategory::Stateful));
    }

    #[test]
    fn stateful_reset_does_not_match_set_to_operator() {
        let lexicons = Lexicons::default();
        let config = ClassificationConfig::default();
        let passages = vec![PassageContext {
            passage_text: "Reset to factory defaults immediately".to_string(),
            triggering_terms: vec!["factory".to_string()],
            session_id: "s1".to_string(),
            timestamp: 10,
        }];
        let result = classify_passages(&passages, &lexicons, &config);
        assert!(!result[0].categories.contains(&ImportanceCategory::Stateful));
    }
}
