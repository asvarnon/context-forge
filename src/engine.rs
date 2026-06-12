//! Core business logic: assembly, scoring, and snapshot management.

use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::config::{Config, DEFAULT_RECENCY_HALF_LIFE_SECS};
use crate::entry::ContextEntry;
use crate::error::Error;
use crate::traits::{ContextStorage, Result, Searcher};

/// Default candidate limit when fetching search results for assembly.
const DEFAULT_SEARCH_LIMIT: usize = 50;

/// Well-defined query constant that means "match all entries" in the Searcher trait.
///
/// FTS5 interprets `*` as a prefix wildcard that matches every token.
/// Searcher implementations MUST treat this value as a match-all query.
pub const MATCH_ALL_QUERY: &str = "*";

/// Options for saving a snapshot entry.
#[derive(Debug, Default, Clone)]
pub struct SaveOptions {
    /// Optional caller session identifier.
    pub session_id: Option<String>,
    /// Namespace partition for the new entry. `None` = global scope.
    pub scope: Option<String>,
    /// Arbitrary caller metadata, stored as JSON.
    ///
    /// Unlike `content`, metadata is persisted **verbatim** by
    /// [`crate::ContextForge::save`] — it is not passed through
    /// [`crate::scrub_secrets`]. Callers MUST NOT place untrusted or
    /// secret-bearing text in this field.
    pub metadata: Option<serde_json::Value>,
}

/// Estimate token count using whitespace heuristic (1 token ≈ 4 chars).
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Exponential recency decay.
///
/// Returns a multiplier in `(0.0, 1.0]` where 1.0 means "just created" and
/// the value halves every `half_life` seconds.
fn recency_decay(age_seconds: f64, half_life: f64) -> f64 {
    0.5_f64.powf(age_seconds / half_life)
}

/// The core context engine that coordinates storage, search, and assembly.
pub struct ContextEngine {
    storage: Box<dyn ContextStorage>,
    searcher: Box<dyn Searcher>,
    config: Config,
}

impl ContextEngine {
    /// Create a new engine with the given storage backend, searcher, and config.
    ///
    /// If `config.recency_half_life_secs` is not positive and finite, it is
    /// clamped to [`DEFAULT_RECENCY_HALF_LIFE_SECS`] to prevent NaN/inf in
    /// recency decay scoring.
    #[must_use]
    pub fn new(
        storage: Box<dyn ContextStorage>,
        searcher: Box<dyn Searcher>,
        mut config: Config,
    ) -> Self {
        if !config.recency_half_life_secs.is_finite() || config.recency_half_life_secs <= 0.0 {
            config.recency_half_life_secs = DEFAULT_RECENCY_HALF_LIFE_SECS;
        }

        Self {
            storage,
            searcher,
            config,
        }
    }

    /// Assemble context entries that fit within `token_budget`.
    ///
    /// 1. Searches for candidates matching `query`, restricted to `scope` if given.
    /// 2. Applies recency weighting to each candidate's score.
    /// 3. Sorts by weighted score descending.
    /// 4. Packs entries greedily until the budget is exhausted.
    ///
    /// `scope = None` searches every entry regardless of scope (global recall).
    /// `scope = Some(s)` restricts the search to entries whose `scope` equals `s`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying search fails.
    pub fn assemble(
        &self,
        query: &str,
        scope: Option<&str>,
        token_budget: usize,
    ) -> Result<Vec<ContextEntry>> {
        let candidates = self.searcher.search(query, scope, DEFAULT_SEARCH_LIMIT)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let now = current_timestamp();

        // Apply recency weighting using the configured half-life.
        let half_life = self.config.recency_half_life_secs;
        let mut weighted: Vec<(f64, ContextEntry)> = candidates
            .into_iter()
            .map(|se| {
                // Unix timestamps are well within f64's 52-bit mantissa for
                // any realistic date range, so precision loss is not a concern.
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Unix timestamps fit losslessly in f64 for millions of years"
                )]
                let age = (now - se.entry.timestamp).max(0) as f64;
                let decay = recency_decay(age, half_life);
                let weighted_score = se.score * decay;
                (weighted_score, se.entry)
            })
            .collect();

        // Sort descending by weighted score (total_cmp handles NaN consistently).
        weighted.sort_by(|a, b| b.0.total_cmp(&a.0));

        // Greedy bin-packing by token budget — skip oversized entries, don't abort.
        let mut result = Vec::new();
        let mut tokens_used: usize = 0;
        for (_score, entry) in weighted {
            let entry_tokens = entry
                .token_count
                .unwrap_or_else(|| estimate_tokens(&entry.content));
            if tokens_used.saturating_add(entry_tokens) > token_budget {
                continue;
            }
            tokens_used += entry_tokens;
            result.push(entry);
        }

        Ok(result)
    }

    /// Save a new snapshot entry. Capacity enforcement (LRU eviction) is
    /// handled atomically by the storage layer inside an immediate
    /// transaction, so no in-process locking is required here.
    ///
    /// Returns the generated entry ID.
    ///
    /// # Security
    ///
    /// This is a low-level write path that does **not** scrub secrets from
    /// `content`. [`ContextForge::save`](crate::ContextForge::save) is the
    /// only entry point that applies [`scrub_secrets`](crate::scrub_secrets)
    /// before persistence; callers writing through the engine directly are
    /// responsible for scrubbing first.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidEntry`] if `content` is empty, or propagates
    /// any error from the underlying storage write.
    pub fn save_snapshot(
        &self,
        content: &str,
        kind: &str,
        options: &SaveOptions,
    ) -> Result<String> {
        if content.is_empty() {
            return Err(Error::InvalidEntry("content must not be empty".into()));
        }

        let timestamp = current_timestamp();
        let id = Uuid::now_v7().to_string();
        let token_count = estimate_tokens(content);

        let entry = ContextEntry {
            id: id.clone(),
            content: content.to_owned(),
            timestamp,
            kind: kind.to_owned(),
            scope: options.scope.clone(),
            session_id: options.session_id.clone(),
            token_count: Some(token_count),
            metadata: options.metadata.clone(),
        };

        self.storage.save(&entry)?;

        Ok(id)
    }

    /// Return a reference to the underlying storage backend.
    ///
    /// This is exposed for callers that need direct access to storage
    /// operations (delete, clear, count) not covered by [`Self::assemble`]
    /// or [`Self::save_snapshot`].
    #[must_use]
    pub fn storage(&self) -> &dyn ContextStorage {
        self.storage.as_ref()
    }
}

/// Current Unix timestamp in seconds.
///
/// Returns `0` if the system clock reports a time before the Unix epoch,
/// which should not happen on any supported platform.
fn current_timestamp() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EvictionPolicy, DEFAULT_RECENCY_HALF_LIFE_SECS};
    use crate::entry::ScoredEntry;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // Mock implementations
    // -----------------------------------------------------------------------

    struct MockStorage {
        entries: Mutex<Vec<ContextEntry>>,
    }

    impl MockStorage {
        fn new() -> Self {
            Self {
                entries: Mutex::new(Vec::new()),
            }
        }
    }

    impl ContextStorage for MockStorage {
        fn save(&self, entry: &ContextEntry) -> Result<()> {
            self.entries.lock().unwrap().push(entry.clone());
            Ok(())
        }

        fn get_top_k(&self, k: usize) -> Result<Vec<ContextEntry>> {
            let guard = self.entries.lock().unwrap();
            let mut sorted = guard.clone();
            sorted.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            sorted.truncate(k);
            Ok(sorted)
        }

        fn get_all(&self) -> Result<Vec<ContextEntry>> {
            Ok(self.entries.lock().unwrap().clone())
        }

        fn delete(&self, id: &str) -> Result<bool> {
            let mut guard = self.entries.lock().unwrap();
            let before = guard.len();
            guard.retain(|e| e.id != id);
            Ok(guard.len() < before)
        }

        fn clear(&self) -> Result<usize> {
            let mut guard = self.entries.lock().unwrap();
            let n = guard.len();
            guard.clear();
            Ok(n)
        }

        fn clear_scope(&self, scope: &str) -> Result<usize> {
            let mut guard = self.entries.lock().unwrap();
            let before = guard.len();
            guard.retain(|e| e.scope.as_deref() != Some(scope));
            Ok(before - guard.len())
        }

        fn count(&self) -> Result<usize> {
            Ok(self.entries.lock().unwrap().len())
        }
    }

    struct MockSearcher {
        results: Mutex<Vec<ScoredEntry>>,
    }

    impl MockSearcher {
        fn new(results: Vec<ScoredEntry>) -> Self {
            Self {
                results: Mutex::new(results),
            }
        }

        fn empty() -> Self {
            Self::new(Vec::new())
        }
    }

    impl Searcher for MockSearcher {
        fn search(
            &self,
            _query: &str,
            _scope: Option<&str>,
            limit: usize,
        ) -> Result<Vec<ScoredEntry>> {
            let guard = self.results.lock().unwrap();
            Ok(guard.iter().take(limit).cloned().collect())
        }
    }

    fn default_config(max_entries: usize) -> Config {
        Config {
            max_entries,
            token_budget: 8192,
            db_path: PathBuf::from(":memory:"),
            eviction_policy: EvictionPolicy::Lru,
            recency_half_life_secs: DEFAULT_RECENCY_HALF_LIFE_SECS,
            ..Config::default()
        }
    }

    fn make_entry(id: &str, content: &str, timestamp: i64) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: crate::entry::kind::MANUAL.to_owned(),
            scope: None,
            session_id: None,
            token_count: Some(estimate_tokens(content)),
            metadata: None,
        }
    }

    fn make_scored(id: &str, content: &str, timestamp: i64, score: f64) -> ScoredEntry {
        ScoredEntry {
            entry: make_entry(id, content, timestamp),
            score,
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1); // (1+3)/4 = 1
        assert_eq!(estimate_tokens("ab"), 1); // (2+3)/4 = 1
        assert_eq!(estimate_tokens("abc"), 1); // (3+3)/4 = 1 (integer)
        assert_eq!(estimate_tokens("abcd"), 1); // (4+3)/4 = 1
        assert_eq!(estimate_tokens("abcde"), 2); // (5+3)/4 = 2
        assert_eq!(estimate_tokens("abcdefgh"), 2); // (8+3)/4 = 2 (integer)
        assert_eq!(estimate_tokens("hello world, this is a test"), 7);
    }

    #[test]
    fn test_engine_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContextEngine>();
    }

    #[test]
    fn test_assemble_fits_budget() {
        let now = current_timestamp();
        let results = vec![
            make_scored("a", "short", now, 1.0),
            make_scored("b", "medium length text here", now, 0.9),
            make_scored("c", "another entry with more content inside", now, 0.8),
        ];

        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::new(results)),
            default_config(100),
        );

        // Budget of 2 tokens: "short" = 2 tokens fits exactly.
        let assembled = engine.assemble("test", None, 2).unwrap();
        assert_eq!(assembled.len(), 1);
        assert_eq!(assembled[0].id, "a");

        // Budget large enough for all.
        let assembled = engine.assemble("test", None, 1000).unwrap();
        assert_eq!(assembled.len(), 3);
    }

    #[test]
    fn test_assemble_skips_oversized_entries() {
        // B1 regression: oversized top-ranked entry must not abort the loop.
        let now = current_timestamp();
        let big_content = "x".repeat(4000); // 1000 tokens
        let results = vec![
            make_scored("big", &big_content, now, 1.0),
            make_scored("small", "fits", now, 0.9),
        ];

        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::new(results)),
            default_config(100),
        );

        // Budget = 5: "big" (1000 tokens) is skipped, "fits" (1 token) is returned.
        let assembled = engine.assemble("test", None, 5).unwrap();
        assert_eq!(assembled.len(), 1);
        assert_eq!(assembled[0].id, "small");
    }

    #[test]
    fn test_assemble_empty_results() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );
        let assembled = engine.assemble("anything", None, 1000).unwrap();
        assert!(assembled.is_empty());
    }

    #[test]
    fn test_recency_weighting() {
        let now = current_timestamp();
        // Two entries with equal BM25 scores but different timestamps.
        let results = vec![
            make_scored("old", "old entry", now - 86_400, 1.0), // 24h old
            make_scored("new", "new entry", now, 1.0),          // just now
        ];

        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::new(results)),
            default_config(100),
        );

        let assembled = engine.assemble("test", None, 1000).unwrap();
        assert_eq!(assembled.len(), 2);
        // Newer entry should rank first due to higher recency score.
        assert_eq!(assembled[0].id, "new");
        assert_eq!(assembled[1].id, "old");
    }

    #[test]
    fn test_save_snapshot_creates_entry() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let id = engine
            .save_snapshot(
                "hello world",
                crate::entry::kind::MANUAL,
                &SaveOptions::default(),
            )
            .unwrap();
        assert!(!id.is_empty());

        // Verify the entry exists in storage.
        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].content, "hello world");
        assert_eq!(all[0].token_count, Some(estimate_tokens("hello world")));
        assert_eq!(all[0].kind, crate::entry::kind::MANUAL);
    }

    #[test]
    fn test_save_snapshot_populates_scope_and_metadata() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let metadata = serde_json::json!({"source": "test"});
        let options = SaveOptions {
            session_id: Some("sess-1".to_owned()),
            scope: Some("project:homelab-rs".to_owned()),
            metadata: Some(metadata.clone()),
        };

        let id = engine
            .save_snapshot("hello scoped", crate::entry::kind::SNAPSHOT, &options)
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].kind, crate::entry::kind::SNAPSHOT);
        assert_eq!(all[0].scope.as_deref(), Some("project:homelab-rs"));
        assert_eq!(all[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(all[0].metadata, Some(metadata));
    }

    #[test]
    fn test_recency_decay_values() {
        let half_life = crate::config::DEFAULT_RECENCY_HALF_LIFE_HOURS * 3600.0;

        // At age 0, decay should be 1.0.
        let decay_0 = recency_decay(0.0, half_life);
        assert!((decay_0 - 1.0).abs() < f64::EPSILON);

        // At age = half_life, decay should be 0.5.
        let decay_half = recency_decay(half_life, half_life);
        assert!((decay_half - 0.5).abs() < 1e-10);

        // At age = 2 * half_life, decay should be 0.25.
        let decay_double = recency_decay(2.0 * half_life, half_life);
        assert!((decay_double - 0.25).abs() < 1e-10);
    }

    #[test]
    fn test_save_snapshot_ids_are_unique() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let id1 = engine
            .save_snapshot(
                "identical content",
                crate::entry::kind::SNAPSHOT,
                &SaveOptions::default(),
            )
            .unwrap();
        let id2 = engine
            .save_snapshot(
                "identical content",
                crate::entry::kind::SNAPSHOT,
                &SaveOptions::default(),
            )
            .unwrap();
        assert_ne!(
            id1, id2,
            "Two saves of identical content must produce distinct IDs"
        );
    }

    #[test]
    fn test_save_empty_content_rejected() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let result = engine.save_snapshot("", crate::entry::kind::MANUAL, &SaveOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("content must not be empty"));
    }

    #[test]
    fn test_invalid_half_life_clamped_to_default() {
        let invalid_values: Vec<f64> = vec![
            0.0,
            -1.0,
            -100.0,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];

        for value in invalid_values {
            let mut config = default_config(100);
            config.recency_half_life_secs = value;

            let engine = ContextEngine::new(
                Box::new(MockStorage::new()),
                Box::new(MockSearcher::empty()),
                config,
            );

            assert_eq!(
                engine.config.recency_half_life_secs, DEFAULT_RECENCY_HALF_LIFE_SECS,
                "half_life {value} should have been clamped to default",
            );
        }
    }

    #[test]
    fn test_valid_half_life_preserved() {
        let mut config = default_config(100);
        config.recency_half_life_secs = 3600.0; // 1 hour — valid

        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            config,
        );

        assert!((engine.config.recency_half_life_secs - 3600.0).abs() < f64::EPSILON);
    }
}
