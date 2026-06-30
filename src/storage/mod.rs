use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::TransactionBehavior;

use crate::entry::ContextEntry;
use crate::storage::schema::{migrate, row_to_entry};
use crate::traits::ContextStorage;

// Shim-phase error converters: rusqlite/r2d2 errors become Migration strings
// until Step 3 replaces the rusqlite bodies with real Turso implementations.
fn re(e: rusqlite::Error) -> crate::Error { crate::Error::Migration(e.to_string()) }
fn pe(e: r2d2::Error) -> crate::Error { crate::Error::Migration(e.to_string()) }

/// Forward-only schema migrations and row-to-entry conversion.
pub mod schema;
/// FTS5-backed `Searcher` implementation backed by rusqlite/spawn_blocking.
pub mod searcher;
/// Turso-native async storage implementation.
pub mod turso_storage;
/// Turso-native async FTS5 searcher.
pub mod turso_searcher;

pub use searcher::SqliteSearcher;
pub use turso_searcher::TursoSearcher;
pub use turso_storage::TursoStorage;

/// Create a paired storage + searcher backed by turso.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or migrations fail.
pub async fn open_storage(
    db_path: &Path,
    max_entries: usize,
) -> crate::Result<(TursoStorage, TursoSearcher)> {
    let storage = TursoStorage::open(db_path, max_entries).await?;
    let db = Arc::clone(&storage.db);
    let searcher = TursoSearcher::new(db);
    Ok((storage, searcher))
}

#[derive(Debug)]
struct PragmaCustomizer;

impl CustomizeConnection<rusqlite::Connection, rusqlite::Error> for PragmaCustomizer {
    fn on_acquire(
        &self,
        conn: &mut rusqlite::Connection,
    ) -> std::result::Result<(), rusqlite::Error> {
        // busy_timeout must be set before journal_mode=WAL: switching to WAL
        // briefly takes an exclusive lock, and on a fresh database with
        // multiple connections racing the switch, an un-armed busy_timeout
        // means that lock contention fails immediately with SQLITE_BUSY
        // instead of waiting.
        conn.execute_batch(
            "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;",
        )?;
        Ok(())
    }
}

/// SQLite-backed implementation of [`ContextStorage`].
pub struct SqliteStorage {
    pool: Arc<Pool<SqliteConnectionManager>>,
    max_entries: usize,
}

impl SqliteStorage {
    /// Open (or create) a `SQLite` database at `db_path` and run migrations.
    ///
    /// For `":memory:"`, a single-connection pool is used so that all operations
    /// share the same in-memory database instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened, the connection
    /// pool cannot be built, or migrations fail.
    pub fn open(db_path: &Path, max_entries: usize) -> crate::Result<Self> {
        let manager = SqliteConnectionManager::file(db_path);
        let mut builder = Pool::builder().connection_customizer(Box::new(PragmaCustomizer));

        // Each `:memory:` connection is a distinct in-memory database.
        // Restrict to a single connection so all callers see the same DB.
        if db_path == Path::new(":memory:") {
            builder = builder.max_size(1);
        } else {
            builder = builder.max_size(4);
        }

        let pool = builder.build(manager).map_err(pe)?;

        let conn = pool.get().map_err(pe)?;
        migrate(&conn)?;

        Ok(Self {
            pool: Arc::new(pool),
            max_entries,
        })
    }

    /// Return a reference-counted handle to the connection pool so that
    /// [`SqliteSearcher`] can share it.
    #[must_use]
    pub fn pool(&self) -> Arc<Pool<SqliteConnectionManager>> {
        Arc::clone(&self.pool)
    }

    /// Return the current schema version from the database.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection pool or query fails.
    pub fn schema_version(&self) -> crate::Result<i64> {
        let conn = self.pool.get().map_err(pe)?;
        let version = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .map_err(re)?;
        Ok(version)
    }

    /// Run a WAL checkpoint (TRUNCATE mode) to flush the WAL file.
    ///
    /// Safe to call at any time; no-op if no WAL pages are pending.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection pool or checkpoint pragma fails.
    pub fn checkpoint(&self) -> crate::Result<()> {
        let conn = self.pool.get().map_err(pe)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").map_err(re)?;
        Ok(())
    }
}

#[async_trait]
impl ContextStorage for SqliteStorage {
    async fn save(&self, entry: &ContextEntry) -> crate::Result<()> {
        let pool = Arc::clone(&self.pool);
        let max_entries = self.max_entries;
        let entry = entry.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get().map_err(pe)?;
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(re)?;

            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1)",
                [&entry.id],
                |r| r.get(0),
            ).map_err(re)?;

            if !exists {
                let current_count: i64 =
                    tx.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0)).map_err(re)?;
                let current_count = usize::try_from(current_count).unwrap_or(usize::MAX);
                if current_count >= max_entries {
                    tx.execute(
                        "DELETE FROM entries WHERE id = (\
                         SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                        [],
                    ).map_err(re)?;
                }
            }

            let metadata_json = entry
                .metadata
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|e| crate::Error::InvalidEntry(format!("metadata is not valid JSON: {e}")))?;

            tx.execute(
                "INSERT OR REPLACE INTO entries (
                    id, content, timestamp, kind, scope,
                    session_id, token_count, metadata
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    entry.id, entry.content, entry.timestamp, entry.kind,
                    entry.scope, entry.session_id,
                    entry.token_count.map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
                    metadata_json,
                ],
            ).map_err(re)?;

            tx.commit().map_err(re)?;
            Ok(())
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn get_top_k(&self, k: usize) -> crate::Result<Vec<ContextEntry>> {
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let mut stmt = conn.prepare(
                "SELECT id, content, timestamp, kind, scope,
                        session_id, token_count, metadata
                 FROM entries ORDER BY timestamp DESC LIMIT ?1",
            ).map_err(re)?;
            let entries = stmt
                .query_map([i64::try_from(k).unwrap_or(i64::MAX)], row_to_entry)
                .map_err(re)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(re)?;
            Ok(entries)
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn get_all(&self) -> crate::Result<Vec<ContextEntry>> {
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let mut stmt = conn.prepare(
                "SELECT id, content, timestamp, kind, scope,
                        session_id, token_count, metadata
                 FROM entries ORDER BY timestamp DESC",
            ).map_err(re)?;
            let entries = stmt
                .query_map([], row_to_entry)
                .map_err(re)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(re)?;
            Ok(entries)
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn delete(&self, id: &str) -> crate::Result<bool> {
        let pool = Arc::clone(&self.pool);
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let changes = conn.execute("DELETE FROM entries WHERE id = ?1", [id.as_str()]).map_err(re)?;
            Ok(changes > 0)
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn clear(&self) -> crate::Result<usize> {
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let changes = conn.execute("DELETE FROM entries", []).map_err(re)?;
            Ok(changes)
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn clear_scope(&self, scope: &str) -> crate::Result<usize> {
        let pool = Arc::clone(&self.pool);
        let scope = scope.to_owned();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let changes = conn.execute(
                "DELETE FROM entries WHERE scope = ?1", [scope.as_str()]
            ).map_err(re)?;
            Ok(changes)
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }

    async fn count(&self) -> crate::Result<usize> {
        let pool = Arc::clone(&self.pool);
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(pe)?;
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
                .map_err(re)?;
            Ok(usize::try_from(count).unwrap_or(usize::MAX))
        })
        .await
        .map_err(|e| crate::Error::Migration(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::engine::MATCH_ALL_QUERY;
    use crate::entry::{kind, ContextEntry};
    use crate::storage::open_storage;
    use crate::traits::{ContextStorage, Searcher};

    fn make_entry(id: &str, content: &str, timestamp: i64, kind: &str) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: kind.to_owned(),
            scope: None,
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }

    fn make_scoped_entry(id: &str, content: &str, timestamp: i64, scope: &str) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: kind::MANUAL.to_owned(),
            scope: Some(scope.to_owned()),
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_save_and_count() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        let entry = make_entry("e1", "hello world", 1000, kind::MANUAL);
        storage.save(&entry).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_save_and_get_top_k() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "first", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "second", 200, kind::SNAPSHOT)).await.unwrap();
        storage.save(&make_entry("e3", "third", 300, kind::SUMMARY)).await.unwrap();

        let top2 = storage.get_top_k(2).await.unwrap();
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].id, "e3"); // most recent
        assert_eq!(top2[1].id, "e2");
    }

    #[tokio::test]
    async fn test_save_and_get_all() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "first", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "second", 200, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e3", "third", 300, kind::MANUAL)).await.unwrap();

        let all = storage.get_all().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "e3");
        assert_eq!(all[1].id, "e2");
        assert_eq!(all[2].id, "e1");
    }

    #[tokio::test]
    async fn test_delete() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "hello", 1000, kind::MANUAL)).await.unwrap();

        assert!(storage.delete("e1").await.unwrap());
        assert!(!storage.delete("nonexistent").await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_clear() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "a", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "b", 200, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e3", "c", 300, kind::MANUAL)).await.unwrap();

        let cleared = storage.clear().await.unwrap();
        assert_eq!(cleared, 3);
        assert_eq!(storage.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_clear_scope() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_scoped_entry("e1", "a", 100, "scope-a")).await.unwrap();
        storage.save(&make_scoped_entry("e2", "b", 200, "scope-a")).await.unwrap();
        storage.save(&make_scoped_entry("e3", "c", 300, "scope-b")).await.unwrap();

        let cleared = storage.clear_scope("scope-a").await.unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(storage.count().await.unwrap(), 1);

        let all = storage.get_all().await.unwrap();
        assert_eq!(all[0].id, "e3");
    }

    #[tokio::test]
    async fn test_clear_scope_no_match() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_scoped_entry("e1", "a", 100, "scope-a")).await.unwrap();

        let cleared = storage.clear_scope("scope-z").await.unwrap();
        assert_eq!(cleared, 0);
        assert_eq!(storage.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let (storage, _) = open_storage(Path::new(":memory:"), 2).await.unwrap();
        storage.save(&make_entry("e1", "oldest", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "middle", 200, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e3", "newest", 300, kind::MANUAL)).await.unwrap();

        assert_eq!(storage.count().await.unwrap(), 2);

        let all = storage.get_all().await.unwrap();
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"e1"), "oldest entry should have been evicted");
        assert!(ids.contains(&"e2"));
        assert!(ids.contains(&"e3"));
    }

    #[tokio::test]
    async fn test_fts_search() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "rust programming language", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "python scripting", 200, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e3", "rust borrow checker", 300, kind::MANUAL)).await.unwrap();

        let results = searcher.search("rust", None, 5).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results[0].score >= results[1].score,
            "results should be ordered by descending score"
        );
    }

    #[tokio::test]
    async fn test_fts_search_no_results() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "hello world", 100, kind::MANUAL)).await.unwrap();

        let results = searcher.search("nonexistent", None, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_fts_search_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_scoped_entry("e1", "rust programming", 100, "a")).await.unwrap();
        storage.save(&make_scoped_entry("e2", "rust borrow checker", 200, "b")).await.unwrap();

        let results_a = searcher.search("rust", Some("a"), 5).await.unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_b = searcher.search("rust", Some("b"), 5).await.unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].entry.id, "e2");

        let results_all = searcher.search("rust", None, 5).await.unwrap();
        assert_eq!(results_all.len(), 2);
    }

    #[tokio::test]
    async fn test_new_entry_with_scope_and_metadata() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();

        let metadata = serde_json::json!({"runtime": "codex", "model": "gpt-5.3-codex"});
        let entry = ContextEntry {
            id: "v3-entry".into(),
            content: "entry with scope and metadata".into(),
            timestamp: 1_700_000_100,
            kind: kind::SUMMARY.to_owned(),
            scope: Some("project:homelab-rs".into()),
            session_id: Some("session-123".into()),
            token_count: Some(6),
            metadata: Some(metadata.clone()),
        };

        storage.save(&entry).await.unwrap();

        let all = storage.get_all().await.unwrap();
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.id, entry.id);
        assert_eq!(got.content, entry.content);
        assert_eq!(got.timestamp, entry.timestamp);
        assert_eq!(got.kind, entry.kind);
        assert_eq!(got.scope, entry.scope);
        assert_eq!(got.session_id, entry.session_id);
        assert_eq!(got.token_count, entry.token_count);
        assert_eq!(got.metadata, Some(metadata));
    }

    #[tokio::test]
    async fn test_insert_or_replace() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "original content", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e1", "updated content", 200, kind::SUMMARY)).await.unwrap();

        assert_eq!(storage.count().await.unwrap(), 1);

        let all = storage.get_all().await.unwrap();
        assert_eq!(all[0].content, "updated content");
    }

    #[tokio::test]
    async fn test_search_match_all_query() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "first entry", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "second entry", 200, kind::SNAPSHOT)).await.unwrap();
        storage.save(&make_entry("e3", "third entry", 300, kind::SUMMARY)).await.unwrap();

        let results = searcher.search(MATCH_ALL_QUERY, None, 10).await.unwrap();
        assert_eq!(results.len(), 3);

        // Ordered by timestamp descending (newest first).
        assert_eq!(results[0].entry.id, "e3");
        assert_eq!(results[1].entry.id, "e2");
        assert_eq!(results[2].entry.id, "e1");

        // Match-all results all share the same fixed score.
        for r in &results {
            assert!((r.score - 1.0).abs() < f64::EPSILON);
        }
    }

    #[tokio::test]
    async fn test_search_match_all_query_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_scoped_entry("e1", "first entry", 100, "a")).await.unwrap();
        storage.save(&make_scoped_entry("e2", "second entry", 200, "b")).await.unwrap();

        let results_a = searcher.search(MATCH_ALL_QUERY, Some("a"), 10).await.unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_all = searcher.search(MATCH_ALL_QUERY, None, 10).await.unwrap();
        assert_eq!(results_all.len(), 2);
    }

    #[tokio::test]
    async fn search_with_fts5_operator_characters_does_not_error() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "marco polo", 100, kind::MANUAL)).await.unwrap();

        // Production bug: a query containing `"` and `.` previously caused
        // `fts5: syntax error`. It must now succeed and find the entry.
        let results = searcher.search(r#"if I say "marco"."#, None, 10).await.unwrap();

        assert!(
            results.iter().any(|r| r.entry.id == "e1"),
            "expected 'marco polo' entry to be found despite FTS5 operator characters in the query"
        );
    }

    #[tokio::test]
    async fn search_or_joins_terms_for_message_length_queries() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage.save(&make_entry("e1", "alpha one", 100, kind::MANUAL)).await.unwrap();
        storage.save(&make_entry("e2", "beta two", 200, kind::MANUAL)).await.unwrap();

        // Under implicit AND, no entry contains all of "alpha", "beta", and
        // "gamma", so this would return zero results. OR-join must return both.
        let results = searcher.search("alpha beta gamma", None, 10).await.unwrap();

        let ids: Vec<&str> = results.iter().map(|r| r.entry.id.as_str()).collect();
        assert!(ids.contains(&"e1"), "expected 'alpha one' entry in results");
        assert!(ids.contains(&"e2"), "expected 'beta two' entry in results");
    }
}
