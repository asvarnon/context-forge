use crate::entry::{ContextEntry, ScoredEntry};
use crate::error::CoreError;

/// Result type alias for core operations.
pub type Result<T> = std::result::Result<T, CoreError>;

/// Trait for persisting and retrieving context entries.
///
/// Implementations must be thread-safe (`Send + Sync`) to support
/// concurrent access from napi worker threads.
pub trait ContextStorage: Send + Sync {
    /// Persist a single entry.
    fn save(&self, entry: &ContextEntry) -> Result<()>;
    /// Return the top-k entries (most recent or highest priority).
    fn get_top_k(&self, k: usize) -> Result<Vec<ContextEntry>>;
    /// Return every stored entry.
    fn get_all(&self) -> Result<Vec<ContextEntry>>;
    /// Delete an entry by id. Returns `true` if an entry was removed.
    fn delete(&self, id: &str) -> Result<bool>;
    /// Remove all entries. Returns the number of entries removed.
    fn clear(&self) -> Result<usize>;
    /// Return the total number of stored entries.
    fn count(&self) -> Result<usize>;
    /// Return the maximum compaction count observed for `session_id`.
    ///
    /// Returns `None` when the session has no entries with compaction counts.
    fn max_compaction_count(&self, session_id: &str) -> Result<Option<i64>>;
}

/// Trait for searching context entries by relevance.
///
/// Implementations must be thread-safe (`Send + Sync`) to support
/// concurrent access from napi worker threads.
pub trait Searcher: Send + Sync {
    /// Search for entries matching `query`, returning at most `limit` results
    /// ordered by descending relevance score.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredEntry>>;
}
