//! Lexicon-based importance scoring.
//!
//! Provides [`LexiconScorer`] — a trait for domain-specific importance
//! weighting injected at [`crate::ContextForge`] construction time via the
//! builder API — and [`ConfigLexiconScorer`], the default TOML-driven
//! implementation.
//!
//! The scorer runs inside [`crate::engine::ContextEngine::assemble`] after
//! BM25 + recency decay, before the token-budget cut. Importance weighting
//! must happen at this point to influence which entries survive the budget
//! cut, not re-rank survivors after.
//!
//! ## Two-layer design
//!
//! Lexicon scoring is opt-in on the builder. [`DefaultEnglishScorer`] recognizes
//! plain-English importance signals ("confirmed", "never mind", etc.) and is
//! enabled via `with_default_english_scorer`. A persona scorer
//! ([`ConfigLexiconScorer`] loaded from a TOML file) is added via
//! `with_persona_scorer`. When both are set they stack via
//! [`CompositeLexiconScorer`] — additive, with the engine applying the `-1.0`
//! floor clamp after fusion.

pub use self::appender::{LexiconAppender, LexiconProposal};
pub use self::bootstrap::bootstrap_prompt;
pub use self::config::{ConfigLexiconScorer, LexiconConfig, LexiconPatterns};
pub use self::defaults::DefaultEnglishScorer;

mod appender;
mod bootstrap;
mod config;
mod defaults;

use std::sync::Arc;

use crate::entry::ContextEntry;

/// Domain-specific importance scorer, injected at construction time.
///
/// Returns an additive boost in the range `(-1.0, +∞)`. The engine
/// combines this with the BM25 + recency score as:
/// `final = base * (1.0 + boost.max(-1.0))`.
///
/// A boost of `0.0` (the default) leaves scores unchanged. A boost of
/// `1.0` doubles the base score. A boost of `-1.0` zeroes it (floor).
///
/// Implementations **must** be `Send + Sync` — the scorer runs inside
/// `tokio::task::spawn_blocking` on the hot `assemble` path.
pub trait LexiconScorer: Send + Sync {
    /// Score a single entry given the current query string.
    fn score(&self, entry: &ContextEntry, query: &str) -> f32;
}

/// Applies multiple [`LexiconScorer`]s in sequence and sums their boosts.
///
/// Used internally by the builder to combine the always-on
/// [`DefaultEnglishScorer`] with any caller-provided persona scorer. Callers
/// can also construct one directly when they need to compose scorers without
/// the builder.
///
/// The `-1.0` floor clamp happens at the engine level after fusion, not here.
pub struct CompositeLexiconScorer {
    scorers: Vec<Arc<dyn LexiconScorer>>,
}

impl CompositeLexiconScorer {
    /// Create a composite from a list of scorers.
    #[must_use]
    pub fn new(scorers: Vec<Arc<dyn LexiconScorer>>) -> Self {
        Self { scorers }
    }
}

impl LexiconScorer for CompositeLexiconScorer {
    fn score(&self, entry: &ContextEntry, query: &str) -> f32 {
        self.scorers.iter().map(|s| s.score(entry, query)).sum()
    }
}
