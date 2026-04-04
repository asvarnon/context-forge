#![warn(clippy::pedantic)]

//! `cf-analysis` - text analysis primitives for importance detection.
//!
//! This crate is pure computation - no I/O, no storage, no network.
//! It provides tokenization, stopword filtering, n-gram extraction,
//! and term count computation.

use std::collections::HashMap;

pub mod classification;
pub mod extraction;
pub mod frequency;
pub mod lexicon;
pub mod ngrams;
pub mod prefilter;
pub mod recurrence;
pub mod scoring;
pub mod tokenizer;

// Re-export public API
pub use classification::{
    classify_passages, ClassificationConfig, ClassifiedPassage, ImportanceCategory, PassageContext,
};
pub use extraction::{extract_passages, ExtractedPassage, ExtractionConfig, ExtractionEntry};
pub use frequency::{term_counts, term_counts_with_ngrams};
pub use lexicon::Lexicons;
pub use ngrams::{bigrams, extract, trigrams};
pub use prefilter::{strip_execution_artifacts, FilterToggle, PrefilterConfig};
pub use recurrence::{compute_recurrence, RecurrenceConfig, RecurrenceResult};
pub use scoring::{pack_segments, score_passages, ImportanceSegment, ScoringConfig};
pub use tokenizer::{Tokenizer, TokenizerConfig};

/// Build per-session term-count maps by pre-filtering, tokenizing, and
/// computing n-gram term counts for each session's entries.
///
/// Each inner `Vec<&str>` represents the raw content strings for one session.
/// The resulting maps are suitable as input to [`compute_recurrence`].
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn build_session_term_maps(
    session_contents: &[Vec<&str>],
    tokenizer: &Tokenizer,
    prefilter_config: &PrefilterConfig,
) -> Vec<HashMap<String, usize>> {
    session_contents
        .iter()
        .map(|contents| {
            let mut combined_tokens: Vec<String> = Vec::new();
            for content in contents {
                let clean = strip_execution_artifacts(content, prefilter_config);
                combined_tokens.extend(tokenizer.tokenize(&clean));
            }
            term_counts_with_ngrams(&combined_tokens)
        })
        .collect()
}
