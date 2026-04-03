#![warn(clippy::pedantic)]

//! `cf-analysis` - text analysis primitives for importance detection.
//!
//! This crate is pure computation - no I/O, no storage, no network.
//! It provides tokenization, stopword filtering, n-gram extraction,
//! and term count computation.

pub mod frequency;
pub mod ngrams;
pub mod prefilter;
pub mod recurrence;
pub mod tokenizer;

// Re-export public API
pub use frequency::{term_counts, term_counts_with_ngrams};
pub use ngrams::{bigrams, extract, trigrams};
pub use prefilter::{strip_execution_artifacts, FilterToggle, PrefilterConfig};
pub use recurrence::{compute_recurrence, RecurrenceConfig, RecurrenceResult};
pub use tokenizer::{Tokenizer, TokenizerConfig};
