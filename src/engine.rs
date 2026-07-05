//! Core business logic: assembly, scoring, and snapshot management.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::config::{Config, DEFAULT_RECENCY_HALF_LIFE_SECS};
use crate::entry::ContextEntry;
use crate::error::Error;
use crate::lexicon::LexiconScorer;
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
    scorer: Option<Arc<dyn LexiconScorer>>,
    #[cfg(feature = "semantic")]
    embedder: Option<Arc<dyn crate::semantic::Embedder>>,
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
            scorer: None,
            #[cfg(feature = "semantic")]
            embedder: None,
        }
    }

    /// Attach a [`LexiconScorer`] to this engine.
    ///
    /// The scorer runs inside [`Self::assemble`] after BM25 + recency decay
    /// and before the token-budget cut. Calling this again replaces the previous
    /// scorer. Prefer constructing via [`crate::ContextForgeBuilder`], which
    /// automatically composes [`crate::DefaultEnglishScorer`] with any
    /// caller-provided persona scorer.
    #[must_use]
    pub fn with_scorer(mut self, scorer: Arc<dyn LexiconScorer>) -> Self {
        self.scorer = Some(scorer);
        self
    }

    /// Attach an [`crate::semantic::Embedder`] for semantic search.
    ///
    /// When set, [`Self::save_snapshot`] generates and stores an embedding for
    /// each new entry, and [`Self::assemble`] blends BM25 and semantic
    /// candidates via Reciprocal Rank Fusion (RRF, k=60).
    #[cfg(feature = "semantic")]
    #[must_use]
    pub fn with_embedder(mut self, embedder: Arc<dyn crate::semantic::Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
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
    pub async fn assemble(
        &self,
        query: &str,
        scope: Option<&str>,
        token_budget: usize,
    ) -> Result<Vec<ContextEntry>> {
        let limit = DEFAULT_SEARCH_LIMIT;

        tracing::debug!(query = %query, ?scope, token_budget, "assemble: bm25 search");
        let bm25_candidates = self.searcher.search(query, scope, limit).await?;
        tracing::debug!(count = %bm25_candidates.len(), "assemble: bm25 complete");

        // Semantic search runs when the feature is enabled and an embedder is
        // configured; otherwise returns empty (no-op default on the Searcher trait).
        let semantic_candidates = self.run_semantic_search(query, scope, limit).await;
        tracing::debug!(count = %semantic_candidates.len(), "assemble: semantic complete");

        if bm25_candidates.is_empty() && semantic_candidates.is_empty() {
            tracing::debug!("assemble: both searches empty");
            return Ok(Vec::new());
        }

        // Reciprocal Rank Fusion (RRF, k=60) over the full union of both
        // candidate sets. Entries absent from one list get worst_rank = limit+1.
        //
        // RRF score = 1/(k + bm25_rank) + 1/(k + semantic_rank)
        // Then multiply by recency decay and (1 + lexicon_boost).
        let worst_rank = limit + 1;
        let k = 60.0_f64;
        let now = current_timestamp();
        let half_life = self.config.recency_half_life_secs;

        // Build entry map: semantic first so BM25 entries win on overlap.
        let mut entry_map: std::collections::HashMap<String, ContextEntry> =
            std::collections::HashMap::new();
        for se in &semantic_candidates {
            entry_map
                .entry(se.entry.id.clone())
                .or_insert_with(|| se.entry.clone());
        }
        for se in &bm25_candidates {
            entry_map.insert(se.entry.id.clone(), se.entry.clone());
        }

        let bm25_rank: std::collections::HashMap<&str, usize> = bm25_candidates
            .iter()
            .enumerate()
            .map(|(i, se)| (se.entry.id.as_str(), i + 1))
            .collect();
        let semantic_rank: std::collections::HashMap<&str, usize> = semantic_candidates
            .iter()
            .enumerate()
            .map(|(i, se)| (se.entry.id.as_str(), i + 1))
            .collect();

        let mut weighted: Vec<(f64, ContextEntry)> = entry_map
            .into_iter()
            .map(|(id, entry)| {
                let br = *bm25_rank.get(id.as_str()).unwrap_or(&worst_rank);
                let sr = *semantic_rank.get(id.as_str()).unwrap_or(&worst_rank);
                let rrf = 1.0 / (k + br as f64) + 1.0 / (k + sr as f64);

                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Unix timestamps fit losslessly in f64 for millions of years"
                )]
                let age = (now - entry.timestamp).max(0) as f64;
                let decay = recency_decay(age, half_life);
                let boost = self
                    .scorer
                    .as_ref()
                    .map_or(0.0_f32, |s| s.score(&entry, query));
                let final_score = rrf * decay * (1.0 + f64::from(boost).clamp(-1.0, 2.0));
                (final_score, entry)
            })
            .collect();

        weighted.sort_by(|a, b| b.0.total_cmp(&a.0));
        tracing::debug!(fused = %weighted.len(), "assemble: rrf fusion complete");

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

        tracing::debug!(entries = %result.len(), tokens_used, "assemble: complete");
        Ok(result)
    }

    /// Run semantic search if an embedder is available; otherwise return empty.
    async fn run_semantic_search(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> Vec<crate::entry::ScoredEntry> {
        #[cfg(feature = "semantic")]
        if let Some(ref embedder) = self.embedder {
            let emb = embedder.clone();
            let query_owned = query.to_owned();
            match tokio::task::spawn_blocking(move || emb.embed(&query_owned)).await {
                Ok(Ok(embedding)) => {
                    tracing::debug!(dims = %embedding.len(), "query embedding ready");
                    match self.searcher.search_semantic(&embedding, scope, limit).await {
                        Ok(results) => return results,
                        Err(e) => tracing::warn!(error = %e, "semantic search failed"),
                    }
                }
                Ok(Err(e)) => tracing::warn!(error = %e, "query embedding failed"),
                Err(e) => tracing::warn!(error = %e, "embed task panicked"),
            }
        }
        // Suppress unused-parameter lint when semantic feature is disabled.
        #[cfg(not(feature = "semantic"))]
        let _ = (query, scope, limit);
        Vec::new()
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
    pub async fn save_snapshot(
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

        self.storage.save(&entry).await?;
        tracing::trace!(id = %id, kind = %kind, tokens = %token_count, "entry saved");

        // Non-fatal: if embedding fails, the entry is stored and BM25-searchable.
        self.embed_and_store(&id, content).await;

        Ok(id)
    }

    /// Generate and persist an embedding for an already-saved entry.
    ///
    /// Errors are logged and swallowed — the entry is always stored even when
    /// embedding fails.
    async fn embed_and_store(&self, id: &str, content: &str) {
        #[cfg(feature = "semantic")]
        if let Some(ref embedder) = self.embedder {
            tracing::debug!(id = %id, "generating embedding");
            let emb = embedder.clone();
            let text = content.to_owned();
            let id_owned = id.to_owned();
            match tokio::task::spawn_blocking(move || emb.embed(&text)).await {
                Ok(Ok(embedding)) => {
                    if let Err(e) = self.storage.save_embedding(&id_owned, &embedding).await {
                        tracing::warn!(
                            id = %id_owned,
                            error = %e,
                            "save_embedding failed; entry stored without semantic index"
                        );
                    } else {
                        tracing::trace!(id = %id_owned, dims = %embedding.len(), "embedding stored");
                    }
                }
                Ok(Err(e)) => tracing::warn!(
                    id = %id,
                    error = %e,
                    "embed failed; entry stored without semantic index"
                ),
                Err(e) => tracing::warn!(
                    id = %id,
                    error = %e,
                    "embed task panicked; entry stored without semantic index"
                ),
            }
        }
        #[cfg(not(feature = "semantic"))]
        let _ = (id, content);
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

    /// Backfill embeddings for all entries that do not yet have one.
    ///
    /// Entries are fetched and embedded in batches of `batch_size` (capped to
    /// at least 1). The `progress` callback is called after each batch with
    /// `(done_so_far, total)`.
    ///
    /// Returns the number of entries that were successfully embedded.
    ///
    /// # Errors
    ///
    /// Returns an error if fetching unembedded entries fails or if the
    /// embedding task panics.
    #[cfg(feature = "semantic")]
    pub async fn backfill_embeddings(
        &self,
        batch_size: usize,
        progress: impl Fn(usize, usize),
    ) -> Result<usize> {
        let embedder = match &self.embedder {
            Some(e) => e.clone(),
            None => {
                tracing::debug!("backfill_embeddings: no embedder configured");
                return Ok(0);
            }
        };

        let all = self.storage.get_unembedded(usize::MAX).await?;
        let total = all.len();
        if total == 0 {
            tracing::debug!("backfill_embeddings: all entries already embedded");
            return Ok(0);
        }

        tracing::debug!(total = %total, batch_size = %batch_size, "backfill_embeddings: starting");
        let batch = batch_size.max(1);
        let mut embedded = 0usize;

        for chunk in all.chunks(batch) {
            let texts: Vec<String> = chunk.iter().map(|e| e.content.clone()).collect();
            let emb = embedder.clone();
            let embeddings = tokio::task::spawn_blocking(move || {
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                emb.embed_batch(&refs)
            })
            .await
            .map_err(|e| Error::Migration(format!("backfill embed task panicked: {e}")))?
            .map_err(|e| Error::Migration(format!("backfill embed failed: {e}")))?;

            for (entry, embedding) in chunk.iter().zip(embeddings.iter()) {
                if let Err(e) = self.storage.save_embedding(&entry.id, embedding).await {
                    tracing::warn!(id = %entry.id, error = %e, "backfill: save_embedding failed");
                } else {
                    embedded += 1;
                }
            }

            progress(embedded, total);
            tracing::debug!(done = %embedded, total = %total, "backfill_embeddings: batch done");
        }

        Ok(embedded)
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
    use async_trait::async_trait;
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

    #[async_trait]
    impl ContextStorage for MockStorage {
        async fn save(&self, entry: &ContextEntry) -> Result<()> {
            self.entries.lock().unwrap().push(entry.clone());
            Ok(())
        }

        async fn get_top_k(&self, k: usize) -> Result<Vec<ContextEntry>> {
            let guard = self.entries.lock().unwrap();
            let mut sorted = guard.clone();
            sorted.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
            sorted.truncate(k);
            Ok(sorted)
        }

        async fn get_all(&self) -> Result<Vec<ContextEntry>> {
            Ok(self.entries.lock().unwrap().clone())
        }

        async fn delete(&self, id: &str) -> Result<bool> {
            let mut guard = self.entries.lock().unwrap();
            let before = guard.len();
            guard.retain(|e| e.id != id);
            Ok(guard.len() < before)
        }

        async fn clear(&self) -> Result<usize> {
            let mut guard = self.entries.lock().unwrap();
            let n = guard.len();
            guard.clear();
            Ok(n)
        }

        async fn clear_scope(&self, scope: &str) -> Result<usize> {
            let mut guard = self.entries.lock().unwrap();
            let before = guard.len();
            guard.retain(|e| e.scope.as_deref() != Some(scope));
            Ok(before - guard.len())
        }

        async fn count(&self) -> Result<usize> {
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

    #[async_trait]
    impl Searcher for MockSearcher {
        async fn search(
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

    #[tokio::test]
    async fn test_assemble_fits_budget() {
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
        let assembled = engine.assemble("test", None, 2).await.unwrap();
        assert_eq!(assembled.len(), 1);
        assert_eq!(assembled[0].id, "a");

        // Budget large enough for all.
        let assembled = engine.assemble("test", None, 1000).await.unwrap();
        assert_eq!(assembled.len(), 3);
    }

    #[tokio::test]
    async fn test_assemble_skips_oversized_entries() {
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
        let assembled = engine.assemble("test", None, 5).await.unwrap();
        assert_eq!(assembled.len(), 1);
        assert_eq!(assembled[0].id, "small");
    }

    #[tokio::test]
    async fn test_assemble_empty_results() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );
        let assembled = engine.assemble("anything", None, 1000).await.unwrap();
        assert!(assembled.is_empty());
    }

    #[tokio::test]
    async fn test_recency_weighting() {
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

        let assembled = engine.assemble("test", None, 1000).await.unwrap();
        assert_eq!(assembled.len(), 2);
        // Newer entry should rank first due to higher recency score.
        assert_eq!(assembled[0].id, "new");
        assert_eq!(assembled[1].id, "old");
    }

    #[tokio::test]
    async fn test_save_snapshot_creates_entry() {
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
            .await
            .unwrap();
        assert!(!id.is_empty());

        // Verify the entry exists in storage.
        let all = engine.storage.get_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].content, "hello world");
        assert_eq!(all[0].token_count, Some(estimate_tokens("hello world")));
        assert_eq!(all[0].kind, crate::entry::kind::MANUAL);
    }

    #[tokio::test]
    async fn test_save_snapshot_populates_scope_and_metadata() {
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
            .await
            .unwrap();

        let all = engine.storage.get_all().await.unwrap();
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

    #[tokio::test]
    async fn test_save_snapshot_ids_are_unique() {
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
            .await
            .unwrap();
        let id2 = engine
            .save_snapshot(
                "identical content",
                crate::entry::kind::SNAPSHOT,
                &SaveOptions::default(),
            )
            .await
            .unwrap();
        assert_ne!(
            id1, id2,
            "Two saves of identical content must produce distinct IDs"
        );
    }

    #[tokio::test]
    async fn test_save_empty_content_rejected() {
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::empty()),
            default_config(100),
        );

        let result = engine
            .save_snapshot("", crate::entry::kind::MANUAL, &SaveOptions::default())
            .await;
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

            assert!(
                (engine.config.recency_half_life_secs - DEFAULT_RECENCY_HALF_LIFE_SECS).abs()
                    < f64::EPSILON,
                "half_life {value} should have been clamped to default",
            );
        }
    }

    #[tokio::test]
    async fn lexicon_scorer_boosts_important_entry_over_neutral() {
        use crate::lexicon::DefaultEnglishScorer;

        let now = current_timestamp();
        // Equal BM25 scores, equal timestamps — only differentiator is lexicon signal.
        let results = vec![
            make_scored("neutral", "something unrelated here", now, 1.0),
            make_scored("important", "confirmed, that is correct", now, 1.0),
        ];

        let scorer: Arc<dyn LexiconScorer> = Arc::new(DefaultEnglishScorer::default());
        let engine = ContextEngine::new(
            Box::new(MockStorage::new()),
            Box::new(MockSearcher::new(results)),
            default_config(100),
        )
        .with_scorer(scorer);

        let assembled = engine.assemble("test", None, 1000).await.unwrap();
        assert_eq!(assembled.len(), 2);
        assert_eq!(
            assembled[0].id, "important",
            "lexicon-boosted entry should rank first"
        );
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
