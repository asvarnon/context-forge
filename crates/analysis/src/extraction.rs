use std::collections::HashSet;
use std::hash::{Hash, Hasher};

const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "vs.", "cf.", "approx.", "Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.",
    "Jr.", "Fig.", "Eq.", "Vol.", "No.",
];

/// Configuration for passage extraction around high-recurrence terms.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExtractionConfig {
    /// Number of surrounding sentences to include on each side.
    pub context_window: usize,
    /// Whether to deduplicate extracted passages by content hash.
    pub dedup_enabled: bool,
    /// Maximum number of sentences allowed in a single extracted passage.
    ///
    /// If a merged range exceeds this value, it is split into sentence-boundary
    /// chunks of at most this size. A value of `0` is treated as a per-sentence
    /// cap (each sentence becomes its own chunk).
    pub max_passage_sentences: usize,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            context_window: 1,
            dedup_enabled: true,
            max_passage_sentences: 6,
        }
    }
}

/// Input entry with pre-filtered content.
#[derive(Debug, Clone)]
pub struct ExtractionEntry {
    /// Source entry identifier.
    pub entry_id: String,
    /// Pre-filtered text content for extraction.
    pub content: String,
}

/// One extracted passage and its metadata.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExtractedPassage {
    /// Extracted passage text.
    pub text: String,
    /// Entry ID this passage came from.
    pub source_entry_id: String,
    /// High-recurrence terms present in this passage.
    ///
    /// For passages produced by splitting an over-long merged range, terms are
    /// re-evaluated: only terms whose lowercased form appears in the chunk's
    /// sentences are included, not all terms from the original merged window.
    pub triggering_terms: Vec<String>,
    /// Content hash for within-session deduplication only.
    /// Computed with [`std::collections::hash_map::DefaultHasher`],
    /// which is NOT stable across Rust versions. Do not persist.
    pub content_hash: String,
}

#[derive(Debug, Clone)]
struct WindowRange {
    start: usize,
    end: usize,
    terms: Vec<String>,
}

/// Extract passages by finding sentence windows around high-recurrence term matches.
#[must_use]
#[allow(
    clippy::implicit_hasher,
    reason = "HashSet with default hasher is acceptable for deterministic local dedup"
)]
pub fn extract_passages(
    entries: &[ExtractionEntry],
    high_recurrence_terms: &[String],
    config: &ExtractionConfig,
) -> Vec<ExtractedPassage> {
    if entries.is_empty() || high_recurrence_terms.is_empty() {
        return Vec::new();
    }

    let normalized_terms: Vec<(String, String)> = high_recurrence_terms
        .iter()
        .cloned()
        .map(|term| {
            let lower = term.to_lowercase();
            (term, lower)
        })
        .collect();

    let mut passages: Vec<ExtractedPassage> = Vec::new();

    for entry in entries {
        let sentences = split_into_sentences(&entry.content);
        if sentences.is_empty() {
            continue;
        }

        let merged_ranges =
            collect_merged_ranges(&sentences, &normalized_terms, config.context_window);
        let ranges = split_ranges_by_max_passage_sentences(
            &sentences,
            &normalized_terms,
            merged_ranges,
            config.max_passage_sentences,
        );
        for range in ranges {
            let passage_text = sentences[range.start..=range.end].join("\n");
            let trimmed_text = passage_text.trim().to_string();
            if trimmed_text.is_empty() {
                continue;
            }

            let content_hash = hash_text(&trimmed_text);
            passages.push(ExtractedPassage {
                text: trimmed_text,
                source_entry_id: entry.entry_id.clone(),
                triggering_terms: range.terms,
                content_hash,
            });
        }
    }

    if config.dedup_enabled {
        let mut seen_hashes: HashSet<String> = HashSet::new();
        passages.retain(|passage| seen_hashes.insert(passage.content_hash.clone()));
    }

    passages
}

fn split_into_sentences(content: &str) -> Vec<String> {
    let mut sentences = Vec::new();

    for line in content.split('\n') {
        let trimmed_line = line.trim();
        if trimmed_line.is_empty() {
            continue;
        }

        sentences.extend(split_line_into_sentences(trimmed_line));
    }

    sentences
}

fn split_line_into_sentences(line: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut start = 0_usize;

    for (index, (byte_index, ch)) in chars.iter().copied().enumerate() {
        if !matches!(ch, '.' | '!' | '?') {
            continue;
        }

        let prev_is_dot = index > 0 && chars[index - 1].1 == '.';
        if ch == '.' && prev_is_dot {
            continue;
        }

        let punctuation_end = byte_index + ch.len_utf8();
        let is_end_of_line = index + 1 == chars.len();

        if ch == '.' && ends_with_known_abbreviation(&line[..punctuation_end]) {
            continue;
        }

        let should_split = is_end_of_line || {
            let mut lookahead = index + 1;
            let mut saw_whitespace = false;

            while lookahead < chars.len() && chars[lookahead].1.is_whitespace() {
                saw_whitespace = true;
                lookahead += 1;
            }

            saw_whitespace && lookahead < chars.len() && chars[lookahead].1.is_uppercase()
        };

        if should_split {
            let sentence = line[start..punctuation_end].trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
            start = punctuation_end;
        }
    }

    if start < line.len() {
        let tail = line[start..].trim();
        if !tail.is_empty() {
            sentences.push(tail.to_string());
        }
    }

    sentences
}

fn ends_with_known_abbreviation(text: &str) -> bool {
    let text_lower = text.to_lowercase();
    ABBREVIATIONS
        .iter()
        .any(|abbreviation| text_lower.ends_with(&abbreviation.to_lowercase()))
}

/// Collect sentence windows around term matches and merge overlapping or adjacent ranges.
///
/// Adjacent windows (those separated by a single sentence gap) are also merged
/// to avoid producing back-to-back passages with a one-sentence gap between them.
/// This means the effective context may be slightly wider than `context_window`
/// when matches are close together.
///
/// Term matching is substring-based: the term `"context forge"` will also match
/// `"context forged"`. Word-boundary matching is not implemented.
fn collect_merged_ranges(
    sentences: &[String],
    normalized_terms: &[(String, String)],
    context_window: usize,
) -> Vec<WindowRange> {
    let mut raw_ranges: Vec<WindowRange> = Vec::new();

    for (sentence_index, sentence) in sentences.iter().enumerate() {
        let sentence_lower = sentence.to_lowercase();
        let mut matched_terms: Vec<String> = normalized_terms
            .iter()
            .filter_map(|(original, lower)| {
                if sentence_lower.contains(lower) {
                    Some(original.clone())
                } else {
                    None
                }
            })
            .collect();

        if matched_terms.is_empty() {
            continue;
        }

        matched_terms.sort();
        matched_terms.dedup();

        let start = sentence_index.saturating_sub(context_window);
        let end = usize::min(
            sentences.len().saturating_sub(1),
            sentence_index + context_window,
        );
        raw_ranges.push(WindowRange {
            start,
            end,
            terms: matched_terms,
        });
    }

    if raw_ranges.is_empty() {
        return raw_ranges;
    }

    raw_ranges.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| left.end.cmp(&right.end))
    });

    let mut merged: Vec<WindowRange> = Vec::new();
    for range in raw_ranges {
        if let Some(last) = merged.last_mut() {
            if range.start <= last.end + 1 {
                last.end = usize::max(last.end, range.end);
                for term in range.terms {
                    if !last.terms.contains(&term) {
                        last.terms.push(term);
                    }
                }
                last.terms.sort();
                continue;
            }
        }

        merged.push(range);
    }

    merged
}

fn split_ranges_by_max_passage_sentences(
    sentences: &[String],
    normalized_terms: &[(String, String)],
    ranges: Vec<WindowRange>,
    max_passage_sentences: usize,
) -> Vec<WindowRange> {
    if ranges.is_empty() {
        return ranges;
    }

    let chunk_size = max_passage_sentences.max(1);

    let mut split_ranges: Vec<WindowRange> = Vec::new();

    for range in ranges {
        let span_len = range.end.saturating_sub(range.start) + 1;
        if span_len <= chunk_size {
            split_ranges.push(range);
            continue;
        }

        let mut chunk_start = range.start;
        while chunk_start <= range.end {
            let chunk_end = usize::min(
                range.end,
                chunk_start.saturating_add(chunk_size).saturating_sub(1),
            );

            let terms = collect_terms_in_range(sentences, normalized_terms, chunk_start, chunk_end);
            if !terms.is_empty() {
                split_ranges.push(WindowRange {
                    start: chunk_start,
                    end: chunk_end,
                    terms,
                });
            }

            chunk_start = chunk_end.saturating_add(1);
        }
    }

    split_ranges
}

fn collect_terms_in_range(
    sentences: &[String],
    normalized_terms: &[(String, String)],
    start: usize,
    end: usize,
) -> Vec<String> {
    debug_assert!(
        end < sentences.len(),
        "chunk_end out of bounds: {end} >= {}",
        sentences.len()
    );

    // Pre-compute lowercased sentences once, then filter terms against the slice.
    let lower_sentences: Vec<String> = sentences[start..=end]
        .iter()
        .map(|sentence| sentence.to_lowercase())
        .collect();

    let mut matched_terms: Vec<String> = normalized_terms
        .iter()
        .filter_map(|(original, lower)| {
            let has_match = lower_sentences
                .iter()
                .any(|sentence| sentence.contains(lower));
            if has_match {
                Some(original.clone())
            } else {
                None
            }
        })
        .collect();

    matched_terms.sort();
    matched_terms
}

fn hash_text(text: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::{extract_passages, split_into_sentences, ExtractionConfig, ExtractionEntry};

    fn single_entry(content: &str) -> Vec<ExtractionEntry> {
        vec![ExtractionEntry {
            entry_id: "entry-1".to_string(),
            content: content.to_string(),
        }]
    }

    fn default_terms(term: &str) -> Vec<String> {
        vec![term.to_string()]
    }

    #[test]
    fn extract_empty_content() {
        let entries = single_entry("");
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert!(passages.is_empty());
    }

    #[test]
    fn extract_no_matching_terms() {
        let entries = single_entry("Alpha sentence. Beta sentence.");
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert!(passages.is_empty());
    }

    #[test]
    fn extract_single_sentence_entry() {
        let entries = single_entry("Context appears here.");
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].text, "Context appears here.");
    }

    #[test]
    fn extract_term_at_start() {
        let entries = single_entry("Context starts here. Second sentence. Third sentence.");
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].text, "Context starts here.\nSecond sentence.");
    }

    #[test]
    fn extract_term_at_end() {
        let entries = single_entry("First sentence. Context ends here.");
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].text, "First sentence.\nContext ends here.");
    }

    #[test]
    fn extract_window_zero() {
        let entries = single_entry("First sentence. Context middle. Last sentence.");
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].text, "Context middle.");
    }

    #[test]
    fn extract_overlapping_windows_merge() {
        let entries = single_entry("One. Alpha context. Three. Beta context. Five. Six.");
        let terms = vec!["alpha".to_string(), "beta".to_string()];
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(
            passages[0].text,
            "One.\nAlpha context.\nThree.\nBeta context.\nFive."
        );
    }

    #[test]
    fn extract_dedup_across_entries() {
        let entries = vec![
            ExtractionEntry {
                entry_id: "entry-1".to_string(),
                content: "Context is stable.".to_string(),
            },
            ExtractionEntry {
                entry_id: "entry-2".to_string(),
                content: "Context is stable.".to_string(),
            },
        ];
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(passages[0].source_entry_id, "entry-1");
    }

    #[test]
    fn extract_multiple_terms_one_sentence() {
        let entries = single_entry("Context forge appears in one sentence.");
        let terms = vec!["context".to_string(), "forge".to_string()];
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 1);
        assert_eq!(
            passages[0].triggering_terms,
            vec!["context".to_string(), "forge".to_string()]
        );
    }

    #[test]
    fn extract_newline_separated_messages() {
        let sentences = split_into_sentences("line one\nline two\nline three");
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn extract_punctuation_within_line() {
        let sentences = split_into_sentences("First sentence. Second sentence. Third.");
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn split_exclamation_mark_terminates_sentence() {
        let sentences = split_into_sentences("It works! Context is next.");
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0], "It works!");
        assert_eq!(sentences[1], "Context is next.");
    }

    #[test]
    fn split_question_mark_terminates_sentence() {
        let sentences = split_into_sentences("Is this correct? Context says yes.");
        assert_eq!(sentences.len(), 2);
        assert_eq!(sentences[0], "Is this correct?");
        assert_eq!(sentences[1], "Context says yes.");
    }

    #[test]
    fn split_abbreviation_not_split() {
        let sentences = split_into_sentences("Use e.g. This method works.");
        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn split_title_abbreviation_not_split() {
        let sentences = split_into_sentences("See Dr. Smith for details.");
        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn split_ellipsis_not_split() {
        let sentences = split_into_sentences("Well... Maybe next time.");
        assert_eq!(sentences.len(), 1);
    }

    #[test]
    fn extract_mixed_boundaries() {
        let sentences = split_into_sentences("Line with two sentences. Second here.\nNew line.");
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn extract_empty_lines_are_boundaries() {
        let sentences = split_into_sentences("Sentence one.\n\nSentence two.");
        assert_eq!(sentences.len(), 2);
    }

    #[test]
    fn extract_ngram_term_matching() {
        let entries = single_entry("We are validating context forge behavior.");
        let terms = vec!["context forge".to_string()];
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 1);
    }

    #[test]
    fn extract_substring_term_matches_derived_words() {
        let entries = single_entry("The code was context forged overnight.");
        let terms = vec!["context forge".to_string()];
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(
            passages.len(),
            1,
            "substring match should find derived words"
        );
    }

    #[test]
    fn extract_adjacent_windows_merge() {
        let entries = vec![ExtractionEntry {
            entry_id: "entry-1".to_string(),
            content: "Alpha context. Two. Three. Beta context. Five.".to_string(),
        }];
        let terms = vec!["alpha".to_string(), "beta".to_string()];
        let config = ExtractionConfig::default();

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 1, "adjacent windows should merge");
        assert!(passages[0].text.contains("Two."));
        assert!(passages[0].text.contains("Three."));
    }

    #[test]
    fn extract_case_insensitive_matching() {
        let entries = single_entry("The Context is important.");
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 1);
    }

    #[test]
    fn extract_dedup_disabled() {
        let entries = vec![
            ExtractionEntry {
                entry_id: "entry-1".to_string(),
                content: "Context is stable.".to_string(),
            },
            ExtractionEntry {
                entry_id: "entry-2".to_string(),
                content: "Context is stable.".to_string(),
            },
        ];
        let config = ExtractionConfig {
            context_window: 1,
            dedup_enabled: false,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 2);
    }

    #[test]
    fn extract_version_numbers_not_split() {
        let sentences = split_into_sentences("Using v0.3.1 in production.");
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0], "Using v0.3.1 in production.");
    }

    #[test]
    fn extract_file_extensions_not_split() {
        let sentences = split_into_sentences("Edit file.rs for changes.");
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0], "Edit file.rs for changes.");
    }

    #[test]
    fn extract_over_cap_splits_into_multiple_passages() {
        let entries = single_entry(
            "Context one. Context two. Context three. Context four. Context five. Context six. Context seven. Context eight. Context nine. Context ten.",
        );
        let config = ExtractionConfig {
            context_window: 0,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[0].text.lines().count(), 6);
        assert_eq!(passages[1].text.lines().count(), 4);
    }

    #[test]
    fn extract_cap_zero_emits_single_sentence_passages() {
        let entries = single_entry("Context one. Context two. Context three.");
        let config = ExtractionConfig {
            context_window: 1,
            dedup_enabled: true,
            max_passage_sentences: 0,
        };

        let passages = extract_passages(&entries, &default_terms("context"), &config);
        assert_eq!(passages.len(), 3);
        assert_eq!(passages[0].text, "Context one.");
        assert_eq!(passages[1].text, "Context two.");
        assert_eq!(passages[2].text, "Context three.");
    }

    #[test]
    fn extract_split_re_evaluates_triggering_terms_per_chunk() {
        let entries = single_entry(
            "One. Alpha signal. Three. Four. Five. Six. Seven. Eight. Nine. Beta signal. Eleven. Twelve.",
        );
        let terms = vec!["alpha".to_string(), "beta".to_string()];
        let config = ExtractionConfig {
            context_window: 5,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[0].triggering_terms, vec!["alpha".to_string()]);
        assert_eq!(passages[1].triggering_terms, vec!["beta".to_string()]);
    }

    #[test]
    fn extract_split_discards_chunks_without_matching_terms() {
        let entries = single_entry(
            "Alpha signal. Two. Three. Four. Five. Six. Seven. Eight. Nine. Ten. Eleven. Twelve. Thirteen. Fourteen. Fifteen. Sixteen. Seventeen. Beta signal.",
        );
        let terms = vec!["alpha".to_string(), "beta".to_string()];
        let config = ExtractionConfig {
            context_window: 8,
            dedup_enabled: true,
            max_passage_sentences: 6,
        };

        let passages = extract_passages(&entries, &terms, &config);
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[0].triggering_terms, vec!["alpha".to_string()]);
        assert_eq!(passages[1].triggering_terms, vec!["beta".to_string()]);
    }
}
