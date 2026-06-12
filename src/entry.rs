use serde::{Deserialize, Serialize};

/// A single context entry stored in the memory engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Unique identifier (UUIDv7, time-ordered).
    pub id: String,
    /// The text content of the entry.
    pub content: String,
    /// Unix timestamp (seconds) when the entry was created.
    pub timestamp: i64,
    /// Caller-defined classification. See the [`kind`] module for well-known values.
    pub kind: String,
    /// Namespace partition, e.g. "discord:thread:123", "project:homelab-rs".
    ///
    /// `None` means global scope.
    pub scope: Option<String>,
    /// Optional caller session identifier.
    pub session_id: Option<String>,
    /// Optional pre-computed token count.
    pub token_count: Option<usize>,
    /// Arbitrary caller metadata, stored as JSON.
    pub metadata: Option<serde_json::Value>,
}

/// Well-known `kind` values. Callers may define their own.
pub mod kind {
    /// User-inserted entry.
    pub const MANUAL: &str = "manual";
    /// Raw conversation capture.
    pub const SNAPSHOT: &str = "snapshot";
    /// Distilled thread/session summary.
    pub const SUMMARY: &str = "summary";
    /// Single distilled fact (Phase 5).
    pub const FACT: &str = "fact";
}

impl Default for ContextEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            content: String::new(),
            timestamp: 0,
            kind: kind::MANUAL.to_owned(),
            scope: None,
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }
}

/// A search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEntry {
    /// The matched entry.
    pub entry: ContextEntry,
    /// Relevance score (higher is more relevant).
    pub score: f64,
}
