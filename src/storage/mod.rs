use std::path::Path;
use std::sync::Arc;

use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::TransactionBehavior;

use crate::entry::ContextEntry;
use crate::storage::schema::{migrate, row_to_entry};
use crate::traits::ContextStorage;

pub mod schema;
pub mod searcher;

pub use searcher::SqliteSearcher;

/// Create a paired storage + searcher sharing the same connection pool.
///
/// # Errors
///
/// Returns an error if the database cannot be opened, the connection pool
/// cannot be built, or migrations fail.
pub fn open_storage(
    db_path: &Path,
    max_entries: usize,
) -> crate::Result<(SqliteStorage, SqliteSearcher)> {
    let storage = SqliteStorage::open(db_path, max_entries)?;
    let searcher = SqliteSearcher::new(storage.pool());
    Ok((storage, searcher))
}

#[derive(Debug)]
struct PragmaCustomizer;

impl CustomizeConnection<rusqlite::Connection, rusqlite::Error> for PragmaCustomizer {
    fn on_acquire(
        &self,
        conn: &mut rusqlite::Connection,
    ) -> std::result::Result<(), rusqlite::Error> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
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

        let pool = builder.build(manager)?;

        let conn = pool.get()?;
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
        let conn = self.pool.get()?;
        let version = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )?;
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
        let conn = self.pool.get()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

impl ContextStorage for SqliteStorage {
    fn save(&self, entry: &ContextEntry) -> crate::Result<()> {
        let mut conn = self.pool.get()?;

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        // LRU eviction: only evict when inserting a new entry (not replacing
        // an existing ID) and currently at capacity.
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1)",
            [&entry.id],
            |r| r.get(0),
        )?;

        if !exists {
            let current_count: i64 =
                tx.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;

            let current_count = usize::try_from(current_count).unwrap_or(usize::MAX);
            if current_count >= self.max_entries {
                tx.execute(
                    "DELETE FROM entries WHERE id = (\
                     SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                    [],
                )?;
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
                id,
                content,
                timestamp,
                kind,
                scope,
                session_id,
                token_count,
                metadata
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
            )",
            rusqlite::params![
                entry.id,
                entry.content,
                entry.timestamp,
                entry.kind,
                entry.scope,
                entry.session_id,
                entry
                    .token_count
                    .map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
                metadata_json,
            ],
        )?;

        tx.commit()?;

        Ok(())
    }

    fn get_top_k(&self, k: usize) -> crate::Result<Vec<ContextEntry>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                content,
                timestamp,
                kind,
                scope,
                session_id,
                token_count,
                metadata
             FROM entries
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;

        let entries = stmt
            .query_map([i64::try_from(k).unwrap_or(i64::MAX)], row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    fn get_all(&self) -> crate::Result<Vec<ContextEntry>> {
        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(
            "SELECT
                id,
                content,
                timestamp,
                kind,
                scope,
                session_id,
                token_count,
                metadata
             FROM entries
             ORDER BY timestamp DESC",
        )?;

        let entries = stmt
            .query_map([], row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    fn delete(&self, id: &str) -> crate::Result<bool> {
        let conn = self.pool.get()?;
        let changes = conn.execute("DELETE FROM entries WHERE id = ?1", [id])?;
        Ok(changes > 0)
    }

    fn clear(&self) -> crate::Result<usize> {
        let conn = self.pool.get()?;
        let changes = conn.execute("DELETE FROM entries", [])?;
        Ok(changes)
    }

    fn clear_scope(&self, scope: &str) -> crate::Result<usize> {
        let conn = self.pool.get()?;
        let changes = conn.execute("DELETE FROM entries WHERE scope = ?1", [scope])?;
        Ok(changes)
    }

    fn count(&self) -> crate::Result<usize> {
        let conn = self.pool.get()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;

    use crate::engine::MATCH_ALL_QUERY;
    use crate::entry::{kind, ContextEntry};
    use crate::storage::{open_storage, SqliteStorage};
    use crate::traits::{ContextStorage, Searcher};

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cf-storage-{name}-{nanos}.db"))
    }

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

    #[test]
    fn checkpoint_runs_without_error() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(dir.path().join("test.db").as_path(), 100).unwrap();
        assert!(storage.checkpoint().is_ok());
    }

    #[test]
    fn test_save_and_count() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        let entry = make_entry("e1", "hello world", 1000, kind::MANUAL);
        storage.save(&entry).unwrap();
        assert_eq!(storage.count().unwrap(), 1);
    }

    #[test]
    fn test_save_and_get_top_k() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, kind::SNAPSHOT))
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, kind::SUMMARY))
            .unwrap();

        let top2 = storage.get_top_k(2).unwrap();
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].id, "e3"); // most recent
        assert_eq!(top2[1].id, "e2");
    }

    #[test]
    fn test_save_and_get_all() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, kind::MANUAL))
            .unwrap();

        let all = storage.get_all().unwrap();
        assert_eq!(all.len(), 3);
        // ordered by timestamp desc
        assert_eq!(all[0].id, "e3");
        assert_eq!(all[1].id, "e2");
        assert_eq!(all[2].id, "e1");
    }

    #[test]
    fn test_delete() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "hello", 1000, kind::MANUAL))
            .unwrap();

        assert!(storage.delete("e1").unwrap());
        assert!(!storage.delete("nonexistent").unwrap());
        assert_eq!(storage.count().unwrap(), 0);
    }

    #[test]
    fn test_clear() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "a", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e2", "b", 200, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e3", "c", 300, kind::MANUAL))
            .unwrap();

        let cleared = storage.clear().unwrap();
        assert_eq!(cleared, 3);
        assert_eq!(storage.count().unwrap(), 0);
    }

    #[test]
    fn test_clear_scope() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_scoped_entry("e1", "a", 100, "scope-a"))
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "b", 200, "scope-a"))
            .unwrap();
        storage
            .save(&make_scoped_entry("e3", "c", 300, "scope-b"))
            .unwrap();

        let cleared = storage.clear_scope("scope-a").unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(storage.count().unwrap(), 1);

        let all = storage.get_all().unwrap();
        assert_eq!(all[0].id, "e3");
    }

    #[test]
    fn test_clear_scope_no_match() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_scoped_entry("e1", "a", 100, "scope-a"))
            .unwrap();

        let cleared = storage.clear_scope("scope-z").unwrap();
        assert_eq!(cleared, 0);
        assert_eq!(storage.count().unwrap(), 1);
    }

    #[test]
    fn test_lru_eviction() {
        let (storage, _) = open_storage(Path::new(":memory:"), 2).unwrap();
        storage
            .save(&make_entry("e1", "oldest", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e2", "middle", 200, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e3", "newest", 300, kind::MANUAL))
            .unwrap();

        assert_eq!(storage.count().unwrap(), 2);

        let all = storage.get_all().unwrap();
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !ids.contains(&"e1"),
            "oldest entry should have been evicted"
        );
        assert!(ids.contains(&"e2"));
        assert!(ids.contains(&"e3"));
    }

    #[test]
    fn test_fts_search() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry(
                "e1",
                "rust programming language",
                100,
                kind::MANUAL,
            ))
            .unwrap();
        storage
            .save(&make_entry("e2", "python scripting", 200, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e3", "rust borrow checker", 300, kind::MANUAL))
            .unwrap();

        let results = searcher.search("rust", None, 5).unwrap();
        assert_eq!(results.len(), 2);
        // Assert ordering by relevance (highest score first), not absolute values.
        assert!(
            results[0].score >= results[1].score,
            "results should be ordered by descending score"
        );
    }

    #[test]
    fn test_fts_search_no_results() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "hello world", 100, kind::MANUAL))
            .unwrap();

        let results = searcher.search("nonexistent", None, 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_fts_search_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_scoped_entry("e1", "rust programming", 100, "a"))
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "rust borrow checker", 200, "b"))
            .unwrap();

        let results_a = searcher.search("rust", Some("a"), 5).unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_b = searcher.search("rust", Some("b"), 5).unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].entry.id, "e2");

        let results_all = searcher.search("rust", None, 5).unwrap();
        assert_eq!(results_all.len(), 2);
    }

    #[test]
    fn test_v2_migration_idempotent() {
        let storage1 = SqliteStorage::open(Path::new(":memory:"), 100).unwrap();
        let conn = storage1.pool().get().unwrap();
        // Running migrate a second time on the same connection should succeed.
        crate::storage::schema::migrate(&conn).unwrap();
    }

    #[test]
    fn test_v1_to_v3_migration() {
        let db_path = temp_db_path("v1-to-v3");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(crate::storage::schema::SCHEMA_V1)
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (id INTEGER PRIMARY KEY CHECK(id = 1), version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 1)",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["m1", "manual entry", 100_i64, "Manual", 2_i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["p1", "precompact entry", 200_i64, "PreCompact", 3_i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["a1", "auto entry", 300_i64, "Auto", 4_i64],
            )
            .unwrap();
        }

        let storage = SqliteStorage::open(&db_path, 100).unwrap();
        let conn = storage.pool().get().unwrap();

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 3);

        // Kinds are remapped to the new lowercase TEXT vocabulary.
        let mut kinds: Vec<String> = conn
            .prepare("SELECT kind FROM entries ORDER BY timestamp")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        kinds.sort();
        assert_eq!(kinds, vec!["manual", "snapshot", "summary"]);

        let tags: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tags, 0, "tags table should exist but be empty");

        let entry_tags: i64 = conn
            .query_row("SELECT COUNT(*) FROM entry_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entry_tags, 0, "entry_tags table should exist but be empty");

        // v2-only runtime tables are gone after the v3 rebuild.
        let runtime_configs_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='runtime_configs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(runtime_configs_exists, 0);

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_v2_to_v3_migration() {
        let db_path = temp_db_path("v2-to-v3");

        {
            let conn = Connection::open(&db_path).unwrap();

            // Build a v2 fixture by running SCHEMA_V1 then SCHEMA_V2 directly,
            // then inserting rows with runtime columns populated.
            conn.execute_batch(crate::storage::schema::SCHEMA_V1)
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (id INTEGER PRIMARY KEY CHECK(id = 1), version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 1)",
                [],
            )
            .unwrap();
            conn.execute_batch(crate::storage::schema::SCHEMA_V2)
                .unwrap();

            conn.execute(
                "INSERT INTO entries (
                    id, content, timestamp, kind, token_count, session_id,
                    compaction_count, compaction_trigger, runtime, model, cwd,
                    git_branch, git_sha, turn_id, agent_type, agent_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                rusqlite::params![
                    "a1",
                    "auto entry with runtime metadata",
                    300_i64,
                    "Auto",
                    4_i64,
                    "session-abc",
                    2_i64,
                    "matcher:threshold",
                    "codex",
                    "gpt-5.3-codex",
                    "/workspace/context-forge",
                    "feature/schema-v2",
                    "abc123def",
                    "turn-77",
                    "coder",
                    "agent-main",
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["m1", "manual entry", 100_i64, "Manual", 2_i64],
            )
            .unwrap();
        }

        let storage = SqliteStorage::open(&db_path, 100).unwrap();

        // entries preserved
        let all = storage.get_all().unwrap();
        assert_eq!(all.len(), 2);

        // kinds remapped: 'Auto' -> 'summary', 'Manual' -> 'manual'
        let auto_entry = all.iter().find(|e| e.id == "a1").unwrap();
        let manual_entry = all.iter().find(|e| e.id == "m1").unwrap();
        assert_eq!(auto_entry.kind, "summary");
        assert_eq!(manual_entry.kind, "manual");

        // runtime fields present inside metadata JSON
        let metadata = auto_entry
            .metadata
            .as_ref()
            .expect("metadata should be present for migrated v2 entry");
        assert_eq!(metadata["runtime"], "codex");
        assert_eq!(metadata["model"], "gpt-5.3-codex");
        assert_eq!(metadata["cwd"], "/workspace/context-forge");
        assert_eq!(metadata["git_branch"], "feature/schema-v2");
        assert_eq!(metadata["git_sha"], "abc123def");
        assert_eq!(metadata["compaction_trigger"], "matcher:threshold");
        assert_eq!(metadata["turn_id"], "turn-77");
        assert_eq!(metadata["agent_type"], "coder");
        assert_eq!(metadata["agent_id"], "agent-main");

        // session_id and token_count survive the rebuild
        assert_eq!(auto_entry.session_id.as_deref(), Some("session-abc"));
        assert_eq!(auto_entry.token_count, Some(4));

        // FTS query still matches content
        let (_, searcher) = open_storage(&db_path, 100).unwrap();
        let results = searcher.search("runtime metadata", None, 10).unwrap();
        assert!(
            results.iter().any(|r| r.entry.id == "a1"),
            "FTS search should still find the migrated entry's content"
        );

        // runtime_configs table is gone
        let conn = storage.pool().get().unwrap();
        let runtime_configs_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='runtime_configs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(runtime_configs_exists, 0);

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 3);

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_new_entry_with_scope_and_metadata() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

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

        storage.save(&entry).unwrap();

        let all = storage.get_all().unwrap();
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

    #[test]
    fn test_insert_or_replace() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "original content", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e1", "updated content", 200, kind::SUMMARY))
            .unwrap();

        assert_eq!(storage.count().unwrap(), 1);

        let all = storage.get_all().unwrap();
        assert_eq!(all[0].content, "updated content");
    }

    #[test]
    fn test_search_match_all_query() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first entry", 100, kind::MANUAL))
            .unwrap();
        storage
            .save(&make_entry("e2", "second entry", 200, kind::SNAPSHOT))
            .unwrap();
        storage
            .save(&make_entry("e3", "third entry", 300, kind::SUMMARY))
            .unwrap();

        let results = searcher.search(MATCH_ALL_QUERY, None, 10).unwrap();
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

    #[test]
    fn test_search_match_all_query_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_scoped_entry("e1", "first entry", 100, "a"))
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "second entry", 200, "b"))
            .unwrap();

        let results_a = searcher.search(MATCH_ALL_QUERY, Some("a"), 10).unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_all = searcher.search(MATCH_ALL_QUERY, None, 10).unwrap();
        assert_eq!(results_all.len(), 2);
    }
}
