//! `context-forge` — a local-first persistent memory library for LLM applications.
//!
//! This crate provides `SQLite` + FTS5 BM25 retrieval, recency-decay scoring, and
//! token-budget-aware context assembly with no network calls. It is intended to
//! be embedded in larger applications (CLI tools, bots, agent runtimes) that need
//! durable, searchable memory.
//!
//! # Quick start
//!
//! ```
//! use context_forge::{kind, ContextForge, Config, SaveOptions};
//! use std::path::PathBuf;
//!
//! let mut config = Config::default();
//! config.db_path = PathBuf::from(":memory:");
//!
//! let cf = ContextForge::open(config)?;
//! cf.save("the deploy failure was caused by a missing env var", kind::SNAPSHOT, &SaveOptions::default())?;
//!
//! let hits = cf.query("deploy failure", None, 2048)?;
//! assert_eq!(hits.len(), 1);
//! # Ok::<(), context_forge::Error>(())
//! ```
//!
//! # Async callers
//!
//! This crate is synchronous by design — it performs blocking `SQLite` I/O and
//! never spawns its own threads or runtime. Callers using an async runtime
//! (e.g. Tokio) should wrap calls in [`spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
//! and share a single [`ContextForge`] instance behind an `Arc`:
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! let cf = Arc::new(ContextForge::open(config)?);
//!
//! // in an async context:
//! let hits = tokio::task::spawn_blocking({
//!     let cf = cf.clone();
//!     move || cf.query("deploy failure", Some("discord:thread:42"), 2048)
//! }).await??;
//! ```
//!
//! # Security
//!
//! **Retrieved entries are untrusted text.** Content persisted from past
//! conversations may contain adversarial instructions (stored prompt
//! injection) — whatever was saved into the store, including text that
//! originated from another user or from a tool's output, comes back out
//! verbatim on [`ContextForge::query`] (aside from save-time secret
//! scrubbing, see below).
//!
//! Callers **MUST** present retrieved memory to models as quoted data
//! (e.g. inside a fenced or otherwise clearly delimited context block
//! labeled as history), **never** as system-level instructions, and
//! **MUST NOT** execute or evaluate anything found in it.
//!
//! ## Save-time secret scrubbing
//!
//! [`ContextForge::save`] applies [`scrub_secrets`] to `content` before it
//! is persisted, using the [`ScrubConfig`] supplied in [`Config::scrub`].
//! This redacts common credential formats (cloud provider keys, API
//! tokens, private key blocks, JWTs, bearer tokens) with
//! `[REDACTED:<label>]` placeholders so they never reach the database or
//! the search index. Scrubbing is **on by default** and can be disabled
//! via `Config { scrub: ScrubConfig { enabled: false }, .. }` — this is an
//! explicit, non-silent opt-out.
//!
//! Note that [`SaveOptions::metadata`] is **not** scrubbed (see its docs).

#![warn(clippy::pedantic)]
#![warn(missing_docs)]

/// Engine and scrub configuration types (`Config`, `EvictionPolicy`, `ScrubConfig`).
pub mod config;
/// Local-LLM distillation trait and the optional `distill-http` implementation.
pub mod distill;
/// `ContextEngine`: search, recency decay, and token-budget assembly.
pub mod engine;
/// `ContextEntry`, `ScoredEntry`, and the `kind` constants module.
pub mod entry;
/// The crate's `Error` type.
pub mod error;
/// Save-time secret scrubbing (`scrub_secrets`, `ScrubConfig`).
pub mod scrub;
/// Session grouping helpers (`group_entries_by_session`, `SessionGroup`).
pub mod session;
/// SQLite-backed storage and search implementations. All SQL lives here.
pub mod storage;
/// `ContextStorage` and `Searcher` traits, and the crate's `Result` alias.
pub mod traits;

/// Importance-detection pipeline (tokenizer, lexicon, scoring). Pure
/// computation, no I/O. Enabled by the `analysis` feature (default).
#[cfg(feature = "analysis")]
pub mod analysis;

#[cfg(feature = "parallel")]
pub use analysis::with_thread_cap;

use std::path::Path;

// Re-export primary types at crate root for convenience.
pub use config::{Config, EvictionPolicy};
pub use distill::{DistilledMemory, Distiller, Fact, FactKind};
pub use engine::{ContextEngine, SaveOptions, MATCH_ALL_QUERY};
pub use entry::{kind, ContextEntry, ScoredEntry};
pub use error::Error;
pub use scrub::{scrub_secrets, ScrubConfig};
pub use session::{group_entries_by_session, SessionGroup};
pub use storage::{open_storage, SqliteSearcher, SqliteStorage};
pub use traits::{ContextStorage, Result, Searcher};

/// The documented entry point for `context-forge`.
///
/// `ContextForge` wires together a [`SqliteStorage`] backend, a
/// [`SqliteSearcher`], and a [`ContextEngine`] behind a small,
/// stable API surface. Advanced callers that need direct access to the
/// underlying storage or searcher can construct those types directly and
/// pass them to [`ContextEngine::new`] instead.
pub struct ContextForge {
    engine: ContextEngine,
    scrub_config: ScrubConfig,
}

impl ContextForge {
    /// Open (or create) the database at `config.db_path`, run any pending
    /// migrations, and build the engine.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened, the connection
    /// pool cannot be built, or migrations fail.
    pub fn open(config: Config) -> Result<Self> {
        let db_path = config.db_path.clone();
        let max_entries = config.max_entries;
        let scrub_config = config.scrub.clone();
        let (storage, searcher) = open_storage(Path::new(&db_path), max_entries)?;
        let engine = ContextEngine::new(Box::new(storage), Box::new(searcher), config);
        Ok(Self {
            engine,
            scrub_config,
        })
    }

    /// Save a new entry. Returns the generated entry ID.
    ///
    /// `kind` is a caller-defined classification (see [`mod@kind`] for
    /// well-known values). Capacity enforcement (LRU eviction) is handled
    /// atomically by the storage layer.
    ///
    /// Before persistence, `content` is passed through [`scrub_secrets`]
    /// using this instance's [`ScrubConfig`] (see [`Config::scrub`]),
    /// redacting common credential formats with `[REDACTED:<label>]`
    /// placeholders. `opts.metadata` is stored verbatim and is **not**
    /// scrubbed — see [`SaveOptions::metadata`].
    ///
    /// # Errors
    ///
    /// Returns an error if `content` is empty or if the underlying storage
    /// write fails.
    pub fn save(&self, content: &str, kind: &str, opts: &SaveOptions) -> Result<String> {
        let scrubbed = scrub_secrets(content, &self.scrub_config);
        self.engine.save_snapshot(&scrubbed, kind, opts)
    }

    /// Distill `transcript` into a summary and durable facts, then save
    /// them as separate entries sharing `opts.scope` and
    /// `opts.session_id`.
    ///
    /// `transcript` is passed through [`scrub_secrets`] **before** it is
    /// sent to `distiller` (so secrets never reach a distillation
    /// endpoint), and the summary/facts produced are scrubbed again before
    /// persistence via the normal [`ContextForge::save`] path (defense in
    /// depth).
    ///
    /// The summary is saved with `kind::SUMMARY`; each fact is saved with
    /// `kind::FACT` and metadata `{"fact_kind": "<kind>", "source":
    /// "distill"}`.
    ///
    /// The distilled output is bounded before any of it is saved: at most
    /// [`MAX_FACTS`](crate::distill::MAX_FACTS) facts are kept, each fact's
    /// text is truncated to at most
    /// [`MAX_FACT_CHARS`](crate::distill::MAX_FACT_CHARS) characters, and the
    /// summary is truncated to at most
    /// [`MAX_SUMMARY_CHARS`](crate::distill::MAX_SUMMARY_CHARS) characters.
    /// Excess facts and text beyond these limits are silently dropped or
    /// truncated, since they are untrusted model-generated content.
    ///
    /// Returns the IDs of the saved entries: the summary's ID first,
    /// followed by each fact's ID in order.
    ///
    /// # Errors
    ///
    /// Returns an error if distillation fails, or if any save fails.
    pub fn distill_and_save(
        &self,
        transcript: &str,
        distiller: &dyn Distiller,
        opts: &SaveOptions,
    ) -> Result<Vec<String>> {
        let scrubbed_transcript = scrub_secrets(transcript, &self.scrub_config);
        let memory = distiller.distill(&scrubbed_transcript)?;
        let memory = crate::distill::cap_distilled_memory(memory);

        let mut ids = Vec::with_capacity(1 + memory.facts.len());

        let summary_id = self.save(&memory.summary, kind::SUMMARY, opts)?;
        ids.push(summary_id);

        for fact in &memory.facts {
            let fact_kind_str = match fact.kind {
                FactKind::Decision => "decision",
                FactKind::Correction => "correction",
                FactKind::Preference => "preference",
                FactKind::State => "state",
            };
            let metadata = serde_json::json!({
                "fact_kind": fact_kind_str,
                "source": "distill",
            });
            let fact_opts = SaveOptions {
                session_id: opts.session_id.clone(),
                scope: opts.scope.clone(),
                metadata: Some(metadata),
            };
            let fact_id = self.save(&fact.text, kind::FACT, &fact_opts)?;
            ids.push(fact_id);
        }

        Ok(ids)
    }

    /// Assemble entries matching `query` that fit within `token_budget`.
    ///
    /// `scope = None` searches every entry regardless of scope (global
    /// recall). `scope = Some(s)` restricts the search to entries whose
    /// `scope` equals `s`.
    ///
    /// # Errors
    ///
    /// Returns an error if the search or recency-weighting step fails.
    pub fn query(
        &self,
        query: &str,
        scope: Option<&str>,
        token_budget: usize,
    ) -> Result<Vec<ContextEntry>> {
        self.engine.assemble(query, scope, token_budget)
    }

    /// Delete a single entry by id. Returns `true` if an entry was removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    pub fn delete(&self, id: &str) -> Result<bool> {
        self.engine.storage().delete(id)
    }

    /// Remove all entries within a given scope. Returns the number of
    /// entries removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    pub fn clear_scope(&self, scope: &str) -> Result<usize> {
        self.engine.storage().clear_scope(scope)
    }

    /// Remove all entries. Returns the number of entries removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage delete fails.
    pub fn clear_all(&self) -> Result<usize> {
        self.engine.storage().clear()
    }

    /// Return the total number of stored entries.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage count fails.
    pub fn count(&self) -> Result<usize> {
        self.engine.storage().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn context_entry_json_roundtrip() {
        let entry = ContextEntry {
            id: "e1".into(),
            content: "hello world".into(),
            timestamp: 1_700_000_000,
            kind: kind::MANUAL.to_owned(),
            scope: None,
            session_id: None,
            token_count: Some(3),
            metadata: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ContextEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "e1");
        assert_eq!(back.token_count, Some(3));
    }

    #[test]
    fn error_display_messages() {
        let invalid = Error::InvalidEntry("empty content".into());
        assert_eq!(invalid.to_string(), "invalid entry: empty content");

        let migration = Error::Migration("schema mismatch".into());
        assert_eq!(migration.to_string(), "migration error: schema mismatch");

        let distill = Error::Distill("model unavailable".into());
        assert_eq!(distill.to_string(), "distillation error: model unavailable");
    }

    #[test]
    fn core_config_json_roundtrip() {
        let cfg = Config {
            max_entries: 1000,
            token_budget: 8192,
            db_path: PathBuf::from("/tmp/cf.db"),
            eviction_policy: EvictionPolicy::Lru,
            recency_half_life_secs: 259_200.0,
            ..Config::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_entries, 1000);
        assert_eq!(back.eviction_policy, EvictionPolicy::Lru);
    }

    #[test]
    fn trait_objects_are_object_safe() {
        // This test verifies that the traits compile as trait objects.
        fn _assert_storage(_s: Box<dyn ContextStorage>) {}
        fn _assert_searcher(_s: Box<dyn Searcher>) {}
    }

    #[test]
    fn kind_constants_are_distinct() {
        assert_ne!(kind::MANUAL, kind::SNAPSHOT);
        assert_ne!(kind::MANUAL, kind::SUMMARY);
        assert_ne!(kind::MANUAL, kind::FACT);
        assert_ne!(kind::SNAPSHOT, kind::SUMMARY);
        assert_ne!(kind::SNAPSHOT, kind::FACT);
        assert_ne!(kind::SUMMARY, kind::FACT);
    }

    #[test]
    fn scored_entry_json_roundtrip() {
        let scored = ScoredEntry {
            entry: ContextEntry {
                id: "s1".into(),
                content: "search hit".into(),
                timestamp: 1_700_000_001,
                kind: kind::SUMMARY.to_owned(),
                scope: None,
                session_id: None,
                token_count: None,
                metadata: None,
            },
            score: 0.95,
        };
        let json = serde_json::to_string(&scored).unwrap();
        let back: ScoredEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entry.id, "s1");
        assert!((back.score - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn context_forge_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ContextForge>();
    }

    #[test]
    fn context_forge_open_save_query_roundtrip() {
        let config = Config {
            db_path: PathBuf::from(":memory:"),
            ..Config::default()
        };
        let cf = ContextForge::open(config).unwrap();

        let id = cf
            .save("hello world", kind::MANUAL, &SaveOptions::default())
            .unwrap();
        assert!(!id.is_empty());
        assert_eq!(cf.count().unwrap(), 1);

        let hits = cf.query("hello", None, 1000).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);

        assert!(cf.delete(&id).unwrap());
        assert_eq!(cf.count().unwrap(), 0);
    }

    #[test]
    fn context_forge_clear_all() {
        let config = Config {
            db_path: PathBuf::from(":memory:"),
            ..Config::default()
        };
        let cf = ContextForge::open(config).unwrap();

        cf.save("a", kind::MANUAL, &SaveOptions::default()).unwrap();
        cf.save("b", kind::MANUAL, &SaveOptions::default()).unwrap();

        let cleared = cf.clear_all().unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(cf.count().unwrap(), 0);
    }

    /// A stub [`Distiller`] for tests that records the transcript it was
    /// called with and returns a fixed [`DistilledMemory`].
    struct StubDistiller {
        transcript: std::sync::Mutex<Option<String>>,
    }

    impl StubDistiller {
        fn new() -> Self {
            Self {
                transcript: std::sync::Mutex::new(None),
            }
        }
    }

    impl Distiller for StubDistiller {
        fn distill(&self, transcript: &str) -> Result<DistilledMemory> {
            *self.transcript.lock().unwrap() = Some(transcript.to_owned());
            Ok(DistilledMemory {
                summary: "User decided to roll back the deploy.".to_owned(),
                facts: vec![
                    Fact {
                        kind: FactKind::Decision,
                        text: "We decided to roll back the deploy.".to_owned(),
                    },
                    Fact {
                        kind: FactKind::Preference,
                        text: "The user prefers terse commit messages.".to_owned(),
                    },
                ],
            })
        }
    }

    #[test]
    fn distill_and_save_scrubs_saves_and_returns_ids() {
        let config = Config {
            db_path: PathBuf::from(":memory:"),
            ..Config::default()
        };
        let cf = ContextForge::open(config).unwrap();

        let distiller = StubDistiller::new();
        let transcript = "Here is a secret key=AKIAABCDEFGHIJKLMNOP end of transcript";

        let opts = SaveOptions {
            session_id: Some("sess-1".to_owned()),
            scope: Some("project:test".to_owned()),
            metadata: None,
        };

        let ids = cf.distill_and_save(transcript, &distiller, &opts).unwrap();

        // Summary ID first, then one ID per fact.
        assert_eq!(ids.len(), 3);
        for id in &ids {
            assert!(!id.is_empty());
        }

        // The transcript reaching the distiller was scrubbed.
        let seen = distiller.transcript.lock().unwrap().clone().unwrap();
        assert!(seen.contains("[REDACTED:aws-key]"));
        assert!(!seen.contains("AKIAABCDEFGHIJKLMNOP"));

        // Summary saved with kind::SUMMARY.
        let summary = cf
            .query("rollback OR rollback OR roll", None, 10_000)
            .unwrap();
        let summary_entry = summary
            .iter()
            .find(|e| e.id == ids[0])
            .expect("summary entry present");
        assert_eq!(summary_entry.kind, kind::SUMMARY);
        assert_eq!(summary_entry.scope.as_deref(), Some("project:test"));
        assert_eq!(summary_entry.session_id.as_deref(), Some("sess-1"));

        // Facts saved with kind::FACT and the right metadata.
        let all = cf.query(MATCH_ALL_QUERY, None, 100_000).unwrap();
        for fact_id in &ids[1..] {
            let fact_entry = all
                .iter()
                .find(|e| &e.id == fact_id)
                .expect("fact entry present");
            assert_eq!(fact_entry.kind, kind::FACT);
            let metadata = fact_entry.metadata.as_ref().expect("metadata present");
            assert_eq!(metadata["source"], "distill");
            assert!(metadata["fact_kind"].is_string());
        }
    }

    /// A stub [`Distiller`] that returns a fixed number of facts, used to
    /// verify that [`ContextForge::distill_and_save`] caps excess facts
    /// before persisting them.
    struct ManyFactsDistiller {
        fact_count: usize,
    }

    impl Distiller for ManyFactsDistiller {
        fn distill(&self, _transcript: &str) -> Result<DistilledMemory> {
            let facts = (0..self.fact_count)
                .map(|i| Fact {
                    kind: FactKind::State,
                    text: format!("fact number {i}"),
                })
                .collect();
            Ok(DistilledMemory {
                summary: "summary".to_owned(),
                facts,
            })
        }
    }

    #[test]
    fn distill_and_save_caps_excess_facts() {
        let config = Config {
            db_path: PathBuf::from(":memory:"),
            ..Config::default()
        };
        let cf = ContextForge::open(config).unwrap();

        let distiller = ManyFactsDistiller {
            fact_count: crate::distill::MAX_FACTS + 20,
        };

        let ids = cf
            .distill_and_save("transcript", &distiller, &SaveOptions::default())
            .unwrap();

        // Summary ID first, then one ID per capped fact.
        assert_eq!(ids.len(), 1 + crate::distill::MAX_FACTS);
        assert_eq!(cf.count().unwrap(), 1 + crate::distill::MAX_FACTS);
    }
}
