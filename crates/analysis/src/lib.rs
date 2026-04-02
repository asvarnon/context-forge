#![warn(clippy::pedantic)]

//! `cf-analysis` - text analysis primitives for importance detection.
//!
//! This crate is pure computation - no I/O, no storage, no network.
//! It provides tokenization, stopword filtering, n-gram extraction,
//! and term frequency computation.

pub mod frequency;
pub mod ngrams;
pub mod tokenizer;

// Re-export primary types
pub use frequency::{term_counts, term_counts_with_ngrams};
pub use ngrams::{bigrams, extract, trigrams};
pub use tokenizer::{Tokenizer, TokenizerConfig};
