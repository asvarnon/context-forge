use serde::{Deserialize, Serialize};

/// A single context entry stored in the memory engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Unique identifier for this entry.
    pub id: String,
    /// The text content of the entry.
    pub content: String,
    /// Unix timestamp (seconds) when the entry was created.
    pub timestamp: i64,
    /// Origin of the entry (e.g. "precompact", "manual", "auto").
    pub source: String,
    /// Optional pre-computed token count.
    pub token_count: Option<u32>,
}

/// Classification of how an entry was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryKind {
    /// Manually inserted by the user or extension.
    Manual,
    /// Captured during a pre-compact hook.
    PreCompact,
    /// Automatically captured by the engine.
    Auto,
}

/// A search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEntry {
    /// The matched entry.
    pub entry: ContextEntry,
    /// Relevance score (higher is more relevant).
    pub score: f64,
}
