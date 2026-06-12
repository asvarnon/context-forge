use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default recency half-life in hours (72 hours = 3 days).
pub const DEFAULT_RECENCY_HALF_LIFE_HOURS: f64 = 72.0;

/// Default recency half-life in seconds (72 hours).
pub const DEFAULT_RECENCY_HALF_LIFE_SECS: f64 = DEFAULT_RECENCY_HALF_LIFE_HOURS * 3600.0;

/// Runtime configuration for the context engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// Maximum number of entries to retain.
    pub max_entries: usize,
    /// Total token budget for context injection.
    pub token_budget: usize,
    /// Path to the backing database file.
    pub db_path: PathBuf,
    /// Strategy used when the store reaches capacity.
    pub eviction_policy: EvictionPolicy,
    /// Recency decay half-life in seconds.
    ///
    /// Controls how fast older entries lose relevance. A value of 259200 (72 hours)
    /// means an entry's score halves every 3 days.
    pub recency_half_life_secs: f64,
}

/// Strategy for evicting entries when at capacity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Least-recently-used entries are evicted first.
    Lru,
}
