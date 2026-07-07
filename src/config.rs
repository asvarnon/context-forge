use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::scrub::ScrubConfig;

/// Default recency half-life in hours (72 hours = 3 days).
pub const DEFAULT_RECENCY_HALF_LIFE_HOURS: f64 = 72.0;

/// Default recency half-life in seconds (72 hours).
pub const DEFAULT_RECENCY_HALF_LIFE_SECS: f64 = DEFAULT_RECENCY_HALF_LIFE_HOURS * 3600.0;

/// Default maximum absolute lexicon boost applied to the score multiplier.
///
/// Deliberately small. The lexicon enters scoring as a multiplier
/// `1.0 + boost.clamp(-c, c)`, and fused RRF relevance scores are tightly
/// compressed (the rank-1-to-rank-2 gap is ~1–2%), so a wide bound lets a
/// query-*independent* importance signal overwhelm relevance and bury the
/// evidence. On a pure-relevance benchmark (`LongMemEval`) `0.05` recovered ~87%
/// of the no-lexicon baseline's Recall@1 while still expressing modest
/// importance; a larger value trades more relevance for more importance weight.
pub const DEFAULT_LEXICON_BOOST_CLAMP: f64 = 0.05;

/// Runtime configuration for the context engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Config {
    /// Maximum number of entries to retain.
    pub max_entries: usize,
    /// Total token budget for context injection.
    pub token_budget: usize,
    /// Path to the backing database file.
    ///
    /// The default (`:memory:`) is an in-memory `SQLite` database that is
    /// **ephemeral** — all data is lost when the connection pool is
    /// dropped. Set a real filesystem path for durable persistence.
    pub db_path: PathBuf,
    /// Strategy used when the store reaches capacity.
    pub eviction_policy: EvictionPolicy,
    /// Recency decay half-life in seconds.
    ///
    /// Controls how fast older entries lose relevance. A value of 259200 (72 hours)
    /// means an entry's score halves every 3 days.
    pub recency_half_life_secs: f64,
    /// Maximum absolute lexicon boost, i.e. the relevance↔importance dial.
    ///
    /// The lexicon scorer's output enters ranking as a multiplier
    /// `1.0 + boost.clamp(-c, c)` on the fused relevance score, where `c` is this
    /// value. `0.0` disables lexicon influence entirely (pure relevance);
    /// larger values give importance signals more weight at the cost of
    /// relevance precision. See [`DEFAULT_LEXICON_BOOST_CLAMP`] for the rationale
    /// behind the conservative default. Negative or non-finite values are
    /// treated as the default.
    pub lexicon_boost_clamp: f64,
    /// Secret-scrubbing configuration applied to entry content at save time.
    pub scrub: ScrubConfig,
}

impl Default for Config {
    /// Defaults: 10,000 max entries, an 8,192-token budget, an in-memory
    /// database (see [`Self::db_path`] for persistence caveats), LRU
    /// eviction, the default 72-hour recency half-life, and secret
    /// scrubbing enabled.
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            token_budget: 8192,
            db_path: PathBuf::from(":memory:"),
            eviction_policy: EvictionPolicy::Lru,
            recency_half_life_secs: DEFAULT_RECENCY_HALF_LIFE_SECS,
            lexicon_boost_clamp: DEFAULT_LEXICON_BOOST_CLAMP,
            scrub: ScrubConfig::default(),
        }
    }
}

/// Strategy for evicting entries when at capacity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub enum EvictionPolicy {
    /// Least-recently-used entries are evicted first.
    Lru,
}
