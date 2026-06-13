use crate::entry::{ContextEntry, ScoredEntry};
use crate::error::Error;

/// Result type alias for crate operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Trait for persisting and retrieving context entries.
///
/// Implementations must be thread-safe (`Send + Sync`) to support
/// concurrent access from multiple worker threads.
pub trait ContextStorage: Send + Sync {
    /// Persist a single entry.
    ///
    /// # Security
    ///
    /// Implementations persist `entry.content` verbatim — secret scrubbing
    /// happens only in [`ContextForge::save`](crate::ContextForge::save).
    /// Callers writing through this trait directly are responsible for
    /// scrubbing first (see [`scrub_secrets`](crate::scrub_secrets)).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage write fails.
    fn save(&self, entry: &ContextEntry) -> Result<()>;

    /// Return the top-k entries (most recent or highest priority).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage read fails.
    fn get_top_k(&self, k: usize) -> Result<Vec<ContextEntry>>;

    /// Return every stored entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage read fails.
    fn get_all(&self) -> Result<Vec<ContextEntry>>;

    /// Delete an entry by id. Returns `true` if an entry was removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    fn delete(&self, id: &str) -> Result<bool>;

    /// Remove all entries. Returns the number of entries removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    fn clear(&self) -> Result<usize>;

    /// Remove all entries within a given scope. Returns the number of entries removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    fn clear_scope(&self, scope: &str) -> Result<usize>;

    /// Return the total number of stored entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage read fails.
    fn count(&self) -> Result<usize>;
}

/// Trait for searching context entries by relevance.
///
/// Implementations must be thread-safe (`Send + Sync`) to support
/// concurrent access from multiple worker threads.
pub trait Searcher: Send + Sync {
    /// Search for entries matching `query`, optionally restricted to `scope`,
    /// returning at most `limit` results ordered by descending relevance score.
    ///
    /// `scope = None` searches every entry regardless of scope (global recall).
    /// `scope = Some(s)` restricts results to entries whose `scope` equals `s`.
    ///
    /// `query` is treated as natural-language text: implementations should
    /// split it into terms and OR-match them (e.g. via FTS5 bm25 ranking)
    /// rather than requiring every term to match. Query-language operator
    /// syntax (`AND`, `OR`, `NEAR`, prefix `*`, quoted phrases, column
    /// filters, etc.) must **not** be interpreted from `query` — operator
    /// characters are treated as term separators, so arbitrary text never
    /// produces a syntax error. A query with no usable terms (empty or
    /// punctuation-only) returns an empty result set, not an error. The
    /// special value [`crate::engine::MATCH_ALL_QUERY`] (`"*"`) matches every
    /// entry.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying search fails.
    fn search(&self, query: &str, scope: Option<&str>, limit: usize) -> Result<Vec<ScoredEntry>>;
}
