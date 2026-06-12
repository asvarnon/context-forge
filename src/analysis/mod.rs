//! Text analysis primitives for importance detection.
//!
//! This module is pure computation - no I/O, no storage, no network.
//! It provides tokenization, stopword filtering, n-gram extraction,
//! and term count computation.

use std::collections::HashMap;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub mod classification;
pub mod extraction;
pub mod frequency;
pub mod injection;
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
pub use injection::{adjust_weights, scale_budget, InjectionConfig};
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
#[allow(
    clippy::implicit_hasher,
    reason = "HashMap with the default hasher is the natural return type here; generalizing \
              over S: BuildHasher would add generic noise with no caller benefit"
)]
#[must_use]
pub fn build_session_term_maps(
    session_contents: &[Vec<&str>],
    tokenizer: &Tokenizer,
    prefilter_config: &PrefilterConfig,
) -> Vec<HashMap<String, usize>> {
    let session_term_map = |contents: &Vec<&str>| {
        let mut combined_tokens: Vec<String> = Vec::new();
        for content in contents {
            let clean = strip_execution_artifacts(content, prefilter_config);
            combined_tokens.extend(tokenizer.tokenize(&clean));
        }
        term_counts_with_ngrams(&combined_tokens)
    };

    #[cfg(feature = "parallel")]
    {
        session_contents.par_iter().map(session_term_map).collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        session_contents.iter().map(session_term_map).collect()
    }
}

/// Run `f` inside a scoped rayon thread pool capped at `thread_cap` threads,
/// or on the global pool when `thread_cap` is `None`.
///
/// This crate never configures rayon's *global* thread pool — the global
/// pool is process-wide and the host application may already own it (for
/// example, a local LLM server sharing the workstation). Callers that want
/// to bound CPU usage for a batch analysis pass should wrap that section in
/// `with_thread_cap(Some(n), || { .. })`. Passing `None` simply calls `f`
/// directly, running on whatever pool (global or otherwise) is already in
/// scope.
///
/// `Some(0)` is passed through to rayon, which treats zero as "choose the
/// thread count automatically" — it is not an error.
///
/// # Errors
///
/// Returns [`crate::Error::InvalidEntry`] if building the scoped thread pool
/// fails (e.g. the OS refuses to spawn threads).
#[cfg(feature = "parallel")]
pub fn with_thread_cap<R: Send>(
    thread_cap: Option<usize>,
    f: impl FnOnce() -> R + Send,
) -> crate::Result<R> {
    match thread_cap {
        Some(threads) => {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .map_err(|err| crate::Error::InvalidEntry(format!("rayon pool: {err}")))?;
            Ok(pool.install(f))
        }
        None => Ok(f()),
    }
}

#[cfg(all(test, feature = "parallel"))]
mod parallel_tests {
    use super::with_thread_cap;

    #[test]
    fn with_thread_cap_some_installs_scoped_pool() {
        let threads = with_thread_cap(Some(2), rayon::current_num_threads).unwrap();
        assert_eq!(
            threads, 2,
            "closure should run inside a 2-thread scoped pool"
        );
    }

    #[test]
    fn with_thread_cap_none_calls_directly() {
        let value = with_thread_cap(None, || 21 * 2).unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn with_thread_cap_zero_means_automatic() {
        // rayon treats num_threads(0) as "choose automatically", not an error.
        let threads = with_thread_cap(Some(0), rayon::current_num_threads).unwrap();
        assert!(threads >= 1);
    }

    #[test]
    fn with_thread_cap_returns_closure_result_through_pool() {
        let sum: usize = with_thread_cap(Some(2), || (1..=100).sum()).unwrap();
        assert_eq!(sum, 5050);
    }
}
