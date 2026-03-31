use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
}

/// Strategy for evicting entries when at capacity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Least-recently-used entries are evicted first.
    Lru,
    /// Entries with the lowest relevance score are evicted first.
    LeastRelevant,
}
