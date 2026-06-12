//! Core business logic: assembly, scoring, and snapshot management.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::config::{CoreConfig, DEFAULT_RECENCY_HALF_LIFE_SECS};
use crate::entry::{ContextEntry, EntryKind};
use crate::error::CoreError;
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
    /// Optional runtime session identifier.
    pub session_id: Option<String>,
    /// Raw JSON payload from stdin, held as a parsed value for adapter processing.
    pub raw_json: Option<serde_json::Value>,
    /// Runtime hint from `--runtime` CLI flag. Overrides auto-detection.
    pub runtime_hint: Option<String>,
}

/// Estimate token count using whitespace heuristic (1 token ≈ 4 chars).
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
    config: CoreConfig,
    /// Guards the compound compaction_count → save sequence within a single
    /// process. Eviction happens atomically inside the storage layer.
    /// Multi-process callers (separate CLI invocations) are not protected;
    /// hook scheduling is relied on for exclusion.
    write_lock: Mutex<()>,
}

impl ContextEngine {
    /// Create a new engine with the given storage backend, searcher, and config.
    ///
    /// If `config.recency_half_life_secs` is not positive and finite, it is
    /// clamped to [`DEFAULT_RECENCY_HALF_LIFE_SECS`] to prevent NaN/inf in
    /// recency decay scoring.
    pub fn new(
        storage: Box<dyn ContextStorage>,
        searcher: Box<dyn Searcher>,
        mut config: CoreConfig,
    ) -> Self {
        if !config.recency_half_life_secs.is_finite() || config.recency_half_life_secs <= 0.0 {
            config.recency_half_life_secs = DEFAULT_RECENCY_HALF_LIFE_SECS;
        }

        Self {
            storage,
            searcher,
            config,
            write_lock: Mutex::new(()),
        }
    }

    /// Assemble context entries that fit within `token_budget`.
    ///
    /// 1. Searches for candidates matching `query`.
    /// 2. Applies recency weighting to each candidate's score.
    /// 3. Sorts by weighted score descending.
    /// 4. Packs entries greedily until the budget is exhausted.
    pub fn assemble(&self, query: &str, token_budget: usize) -> Result<Vec<ContextEntry>> {
        let candidates = self.searcher.search(query, DEFAULT_SEARCH_LIMIT)?;
        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        let now = current_timestamp();

        // Apply recency weighting using the configured half-life.
        let half_life = self.config.recency_half_life_secs;
        let mut weighted: Vec<(f64, ContextEntry)> = candidates
            .into_iter()
            .map(|se| {
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
    /// handled atomically by the storage layer.
    ///
    /// Returns the generated entry ID.
    pub fn save_snapshot(
        &self,
        content: &str,
        kind: EntryKind,
        options: &SaveOptions,
    ) -> Result<String> {
        if content.is_empty() {
            return Err(CoreError::InvalidEntry("content must not be empty".into()));
        }

        let session_id = options.session_id.clone();

        let timestamp = current_timestamp();
        let id = Uuid::now_v7().to_string();
        let token_count = estimate_tokens(content);

        // Guards the compound compaction_count → save sequence within a single
        // process. Eviction happens atomically inside the storage layer.
        // Multi-process callers (separate CLI invocations) are not protected;
        // hook scheduling is relied on for exclusion.
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let compaction_count = if matches!(kind, EntryKind::Auto) {
            if let Some(sid) = session_id.as_deref() {
                let current_max = self.storage.max_compaction_count(sid)?;
                let next = current_max.unwrap_or(-1).checked_add(1).ok_or_else(|| {
                    CoreError::Storage(format!(
                        "compaction_count overflow for session '{}'",
                        sid.chars().take(64).collect::<String>()
                    ))
                })?;
                Some(next)
            } else {
                None
            }
        } else {
            None
        };

        let mut entry = ContextEntry {
            id: id.clone(),
            content: content.to_owned(),
            timestamp,
            kind,
            token_count: Some(token_count),
            session_id,
            compaction_count,
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        };

        match &options.raw_json {
            Some(raw_json) => {
                self.storage.save_with_metadata(
                    &mut entry,
                    raw_json,
                    options.runtime_hint.as_deref(),
                )?;
            }
            None => self.storage.save(&entry)?,
        }

        Ok(id)
    }
}

/// Current Unix timestamp in seconds.
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EvictionPolicy, DEFAULT_RECENCY_HALF_LIFE_SECS};
    use crate::entry::ScoredEntry;
    use std::path::PathBuf;

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

        fn count(&self) -> Result<usize> {
            Ok(self.entries.lock().unwrap().len())
        }

        fn max_compaction_count(&self, session_id: &str) -> Result<Option<i64>> {
            let max = self
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|entry| entry.session_id.as_deref() == Some(session_id))
                .filter_map(|entry| entry.compaction_count)
                .max();
            Ok(max)
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
        fn search(&self, _query: &str, limit: usize) -> Result<Vec<ScoredEntry>> {
            let guard = self.results.lock().unwrap();
            Ok(guard.iter().take(limit).cloned().collect())
        }
    }

    fn default_config(max_entries: usize) -> CoreConfig {
        CoreConfig {
            max_entries,
            token_budget: 8192,
            db_path: PathBuf::from(":memory:"),
            eviction_policy: EvictionPolicy::Lru,
            recency_half_life_secs: DEFAULT_RECENCY_HALF_LIFE_SECS,
        }
    }

    fn make_entry(id: &str, content: &str, timestamp: i64) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: EntryKind::Manual,
            token_count: Some(estimate_tokens(content)),
            session_id: None,
            compaction_count: None,
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
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
        let assembled = engine.assemble("test", 2).unwrap();
        assert_eq!(assembled.len(), 1);
        assert_eq!(assembled[0].id, "a");

        // Budget large enough for all.
        let assembled = engine.assemble("test", 1000).unwrap();
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
        let assembled = engine.assemble("test", 5).unwrap();
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
        let assembled = engine.assemble("anything", 1000).unwrap();
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

        let assembled = engine.assemble("test", 1000).unwrap();
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
            .save_snapshot("hello world", EntryKind::Manual, &SaveOptions::default())
            .unwrap();
        assert!(!id.is_empty());

        // Verify the entry exists in storage.
        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].content, "hello world");
        assert_eq!(all[0].token_count, Some(estimate_tokens("hello world")));
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
                EntryKind::PreCompact,
                &SaveOptions::default(),
            )
            .unwrap();
        let id2 = engine
            .save_snapshot(
                "identical content",
                EntryKind::PreCompact,
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

        let result = engine.save_snapshot("", EntryKind::Manual, &SaveOptions::default());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("content must not be empty"));
    }

    #[test]
    fn test_auto_compaction_count_increments_for_session() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        engine
            .save_snapshot(
                "first auto",
                EntryKind::Auto,
                &SaveOptions {
                    session_id: Some("sess-1".to_owned()),
                    ..SaveOptions::default()
                },
            )
            .unwrap();
        engine
            .save_snapshot(
                "second auto",
                EntryKind::Auto,
                &SaveOptions {
                    session_id: Some("sess-1".to_owned()),
                    ..SaveOptions::default()
                },
            )
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        let mut counts: Vec<i64> = all
            .iter()
            .filter_map(|entry| entry.compaction_count)
            .collect();
        counts.sort_unstable();

        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .all(|entry| entry.session_id.as_deref() == Some("sess-1")));
        assert_eq!(counts, vec![0, 1]);
    }

    #[test]
    fn test_pre_compact_with_session_has_no_compaction_count() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        engine
            .save_snapshot(
                "pre compact snapshot",
                EntryKind::PreCompact,
                &SaveOptions {
                    session_id: Some("sess-2".to_owned()),
                    ..SaveOptions::default()
                },
            )
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id.as_deref(), Some("sess-2"));
        assert_eq!(all[0].compaction_count, None);
    }

    #[test]
    fn test_auto_without_session_has_no_compaction_count() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        engine
            .save_snapshot(
                "auto without session",
                EntryKind::Auto,
                &SaveOptions::default(),
            )
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].session_id, None);
        assert_eq!(all[0].compaction_count, None);
    }

    #[test]
    fn test_pre_compact_then_auto_starts_compaction_count_at_zero() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        engine
            .save_snapshot(
                "pre compact snapshot",
                EntryKind::PreCompact,
                &SaveOptions {
                    session_id: Some("sess-mixed".to_owned()),
                    ..SaveOptions::default()
                },
            )
            .unwrap();

        engine
            .save_snapshot(
                "first auto after pre compact",
                EntryKind::Auto,
                &SaveOptions {
                    session_id: Some("sess-mixed".to_owned()),
                    ..SaveOptions::default()
                },
            )
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        let auto = all
            .iter()
            .find(|entry| {
                entry.kind == EntryKind::Auto && entry.session_id.as_deref() == Some("sess-mixed")
            })
            .expect("auto entry for sess-mixed should exist");

        assert_eq!(auto.compaction_count, Some(0));
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
