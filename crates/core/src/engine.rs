//! Core business logic: assembly, scoring, eviction, and snapshot management.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{CoreConfig, EvictionPolicy, DEFAULT_RECENCY_HALF_LIFE_SECS};
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

/// Monotonic counter for ID uniqueness within the same second.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    /// Guards the compound count → evict → save sequence against concurrent callers.
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

    /// Save a new snapshot entry, evicting if at capacity.
    ///
    /// Returns the generated entry ID.
    pub fn save_snapshot(&self, content: &str, kind: EntryKind) -> Result<String> {
        if content.is_empty() {
            return Err(CoreError::InvalidEntry("content must not be empty".into()));
        }

        let timestamp = current_timestamp();
        let id = generate_id(content, timestamp);
        let token_count = estimate_tokens(content);

        let entry = ContextEntry {
            id: id.clone(),
            content: content.to_owned(),
            timestamp,
            kind,
            token_count: Some(token_count),
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
        };

        // Lock the compound count → evict → save operation to prevent TOCTOU races.
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Evict if at capacity.
        if self.storage.count()? >= self.config.max_entries {
            self.evict_one()?;
        }

        self.storage.save(&entry)?;
        Ok(id)
    }

    /// Evict a single entry according to the configured eviction policy.
    fn evict_one(&self) -> Result<()> {
        match self.config.eviction_policy {
            EvictionPolicy::Lru => self.evict_oldest(),
            EvictionPolicy::LeastRelevant => self.evict_least_relevant(),
        }
    }

    /// Evict the entry with the smallest (oldest) timestamp.
    fn evict_oldest(&self) -> Result<()> {
        let all = self.storage.get_all()?;
        if let Some(oldest) = all.iter().min_by_key(|e| e.timestamp) {
            if !self.storage.delete(&oldest.id)? {
                return Err(CoreError::Storage(format!(
                    "eviction failed: entry '{}' was not deleted",
                    oldest.id
                )));
            }
        }
        Ok(())
    }

    /// Evict the entry with the lowest search relevance.
    ///
    /// Uses the searcher to retrieve scored results and removes the lowest.
    /// Falls back to LRU if search returns nothing (e.g., FTS5 empty query).
    fn evict_least_relevant(&self) -> Result<()> {
        let results = self.searcher.search(MATCH_ALL_QUERY, i64::MAX as usize)?;
        if let Some(lowest) = results.iter().min_by(|a, b| a.score.total_cmp(&b.score)) {
            if !self.storage.delete(&lowest.entry.id)? {
                return Err(CoreError::Storage(format!(
                    "eviction failed: entry '{}' was not deleted",
                    lowest.entry.id
                )));
            }
        } else {
            // Fallback to LRU when search returns nothing.
            self.evict_oldest()?;
        }
        Ok(())
    }
}

/// Generate a unique ID from content and timestamp.
///
/// Uses FNV-1a (64-bit) for a process-independent hash. A per-process
/// atomic counter is mixed in to prevent collisions when two entries
/// have identical content and land within the same second.
fn generate_id(content: &str, timestamp: i64) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut hash = FNV_OFFSET;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for byte in timestamp.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    for byte in seq.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{timestamp}-{hash:x}")
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
    use crate::config::DEFAULT_RECENCY_HALF_LIFE_SECS;
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

        fn with_entries(entries: Vec<ContextEntry>) -> Self {
            Self {
                entries: Mutex::new(entries),
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
            .save_snapshot("hello world", EntryKind::Manual)
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
    fn test_eviction_at_boundary() {
        let mut entries = Vec::new();
        for i in 0..100 {
            entries.push(make_entry(
                &format!("e{i}"),
                &format!("entry {i}"),
                1_700_000_000 + i as i64,
            ));
        }
        let storage = MockStorage::with_entries(entries);

        let engine = ContextEngine::new(
            Box::new(storage),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        assert_eq!(engine.storage.count().unwrap(), 100);

        // Saving one more should evict one, keeping count at 100.
        engine.save_snapshot("entry 100", EntryKind::Auto).unwrap();
        assert_eq!(engine.storage.count().unwrap(), 100);
    }

    #[test]
    fn test_lru_eviction_removes_oldest() {
        let entries = vec![
            make_entry("oldest", "first entry", 1_700_000_000),
            make_entry("middle", "second entry", 1_700_000_500),
            make_entry("newest", "third entry", 1_700_001_000),
        ];
        let storage = MockStorage::with_entries(entries);

        let engine = ContextEngine::new(
            Box::new(storage),
            Box::new(MockSearcher::empty()),
            default_config(3), // at capacity
        );

        engine
            .save_snapshot("fourth entry", EntryKind::Manual)
            .unwrap();

        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 3);
        // "oldest" (timestamp 1_700_000_000) should have been evicted.
        assert!(!all.iter().any(|e| e.id == "oldest"));
        assert!(all.iter().any(|e| e.id == "middle"));
        assert!(all.iter().any(|e| e.id == "newest"));
    }

    #[test]
    fn test_least_relevant_eviction() {
        let entries = vec![
            make_entry("low", "low relevance", 1_700_000_000),
            make_entry("high", "high relevance", 1_700_000_001),
        ];

        // Searcher returns both entries, "low" has lower score.
        let search_results = vec![
            make_scored("high", "high relevance", 1_700_000_001, 5.0),
            make_scored("low", "low relevance", 1_700_000_000, 1.0),
        ];

        let mut config = default_config(2);
        config.eviction_policy = EvictionPolicy::LeastRelevant;

        let engine = ContextEngine::new(
            Box::new(MockStorage::with_entries(entries)),
            Box::new(MockSearcher::new(search_results)),
            config,
        );

        engine.save_snapshot("new entry", EntryKind::Auto).unwrap();

        let all = engine.storage.get_all().unwrap();
        assert_eq!(all.len(), 2);
        // "low" should have been evicted (lowest score).
        assert!(!all.iter().any(|e| e.id == "low"));
        assert!(all.iter().any(|e| e.id == "high"));
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
    fn test_generate_id_unique_even_with_same_inputs() {
        // Monotonic counter ensures uniqueness even with identical content + timestamp.
        let id1 = generate_id("content", 1_700_000_000);
        let id2 = generate_id("content", 1_700_000_000);
        assert_ne!(id1, id2);

        // Different content → different id.
        let id3 = generate_id("other", 1_700_000_000);
        assert_ne!(id1, id3);

        // Different timestamp → different id.
        let id4 = generate_id("content", 1_700_000_001);
        assert_ne!(id1, id4);

        // ID should contain the timestamp prefix.
        assert!(id1.starts_with("1700000000-"));
    }

    #[test]
    fn test_save_empty_content_rejected() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let result = engine.save_snapshot("", EntryKind::Manual);
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
