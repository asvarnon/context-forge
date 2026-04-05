use std::path::Path;
use std::sync::Arc;

use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;

use cf_core::entry::ContextEntry;
use cf_core::error::CoreError;
use cf_core::traits::ContextStorage;

use crate::adapter;
use crate::schema::{kind_to_str, migrate, row_to_entry};

/// Maximum allowed length for field values extracted from runtime metadata.
const MAX_FIELD_LEN: usize = 4096;
/// Maximum allowed length for session IDs from runtime metadata.
const MAX_SESSION_ID_LEN: usize = 512;

/// Return `Some(value)` only when it fits within `limit` bytes.
fn bounded(value: &str, limit: usize) -> Option<String> {
    if value.len() <= limit {
        Some(value.to_owned())
    } else {
        None
    }
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
    /// Open (or create) a SQLite database at `db_path` and run migrations.
    ///
    /// For `":memory:"`, a single-connection pool is used so that all operations
    /// share the same in-memory database instance.
    pub fn open(db_path: &Path, max_entries: usize) -> cf_core::Result<Self> {
        let manager = SqliteConnectionManager::file(db_path);
        let mut builder = Pool::builder().connection_customizer(Box::new(PragmaCustomizer));

        // Each `:memory:` connection is a distinct in-memory database.
        // Restrict to a single connection so all callers see the same DB.
        if db_path == Path::new(":memory:") {
            builder = builder.max_size(1);
        } else {
            builder = builder.max_size(4);
        }

        let pool = builder
            .build(manager)
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let conn = pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
        migrate(&conn)?;

        Ok(Self {
            pool: Arc::new(pool),
            max_entries,
        })
    }

    /// Return a reference-counted handle to the connection pool so that
    /// [`SqliteSearcher`](crate::searcher::SqliteSearcher) can share it.
    pub fn pool(&self) -> Arc<Pool<SqliteConnectionManager>> {
        Arc::clone(&self.pool)
    }

    /// Return the current schema version from the database.
    pub fn schema_version(&self) -> cf_core::Result<i64> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| CoreError::Storage(e.to_string()))
    }

    /// Save an entry and store raw runtime metadata when available.
    pub(crate) fn save_with_metadata(
        &self,
        entry: &mut ContextEntry,
        raw_json: &serde_json::Value,
        runtime_hint: Option<&str>,
    ) -> cf_core::Result<()> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let tx = conn
            .transaction()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let detected_runtime = adapter::detect_runtime(raw_json, runtime_hint);
        if let Some(runtime) = detected_runtime.as_deref() {
            let mappings = adapter::load_mappings(&tx, runtime)?;
            let extracted = adapter::extract_fields(raw_json, &mappings);

            if entry.session_id.is_none() {
                if let Some(value) = extracted.get("session_id") {
                    if value.len() <= MAX_SESSION_ID_LEN {
                        entry.session_id = Some(value.clone());
                    }
                }
            }
            if let Some(value) = extracted.get("model") {
                entry.model = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("cwd") {
                entry.cwd = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("compaction_trigger") {
                entry.compaction_trigger = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("agent_type") {
                entry.agent_type = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("agent_id") {
                entry.agent_id = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("turn_id") {
                entry.turn_id = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("git_branch") {
                entry.git_branch = bounded(value, MAX_FIELD_LEN);
            }
            if let Some(value) = extracted.get("git_sha") {
                entry.git_sha = bounded(value, MAX_FIELD_LEN);
            }
        }

        entry.runtime = detected_runtime.clone();

        // LRU eviction: only evict when inserting a new entry (not replacing
        // an existing ID) and currently at capacity.
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1)",
                [&entry.id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        if !exists {
            let current_count: i64 = tx
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
                .map_err(|e| CoreError::Storage(e.to_string()))?;

            if current_count as usize >= self.max_entries {
                tx.execute(
                    "DELETE FROM entries WHERE id = (\
                     SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                    [],
                )
                .map_err(|e| CoreError::Storage(e.to_string()))?;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO entries (
                id,
                content,
                timestamp,
                kind,
                token_count,
                session_id,
                compaction_count,
                compaction_trigger,
                runtime,
                model,
                cwd,
                git_branch,
                git_sha,
                turn_id,
                agent_type,
                agent_id
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )",
            rusqlite::params![
                entry.id,
                entry.content,
                entry.timestamp,
                kind_to_str(&entry.kind),
                entry.token_count.map(|v| v as i64),
                entry.session_id,
                entry.compaction_count,
                entry.compaction_trigger,
                entry.runtime,
                entry.model,
                entry.cwd,
                entry.git_branch,
                entry.git_sha,
                entry.turn_id,
                entry.agent_type,
                entry.agent_id,
            ],
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;

        if let Some(runtime) = detected_runtime {
            let raw_json_text =
                serde_json::to_string(raw_json).map_err(|e| CoreError::Storage(e.to_string()))?;

            tx.execute(
                "INSERT INTO entry_metadata_raw (entry_id, runtime, raw_json) VALUES (?1, ?2, ?3)",
                rusqlite::params![entry.id, runtime, raw_json_text],
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        }

        tx.commit().map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(())
    }
}

impl ContextStorage for SqliteStorage {
    fn save(&self, entry: &ContextEntry) -> cf_core::Result<()> {
        let mut conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let tx = conn
            .transaction()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        // LRU eviction: only evict when inserting a new entry (not replacing
        // an existing ID) and currently at capacity.
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1)",
                [&entry.id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        if !exists {
            let current_count: i64 = tx
                .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
                .map_err(|e| CoreError::Storage(e.to_string()))?;

            if current_count as usize >= self.max_entries {
                tx.execute(
                    "DELETE FROM entries WHERE id = (\
                     SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                    [],
                )
                .map_err(|e| CoreError::Storage(e.to_string()))?;
            }
        }

        tx.execute(
            "INSERT OR REPLACE INTO entries (
                id,
                content,
                timestamp,
                kind,
                token_count,
                session_id,
                compaction_count,
                compaction_trigger,
                runtime,
                model,
                cwd,
                git_branch,
                git_sha,
                turn_id,
                agent_type,
                agent_id
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
            )",
            rusqlite::params![
                entry.id,
                entry.content,
                entry.timestamp,
                kind_to_str(&entry.kind),
                entry.token_count.map(|v| v as i64),
                entry.session_id,
                entry.compaction_count,
                entry.compaction_trigger,
                entry.runtime,
                entry.model,
                entry.cwd,
                entry.git_branch,
                entry.git_sha,
                entry.turn_id,
                entry.agent_type,
                entry.agent_id,
            ],
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;

        tx.commit().map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(())
    }

    fn save_with_metadata(
        &self,
        entry: &mut ContextEntry,
        raw_json: &serde_json::Value,
        runtime_hint: Option<&str>,
    ) -> cf_core::Result<()> {
        SqliteStorage::save_with_metadata(self, entry, raw_json, runtime_hint)
    }

    fn get_top_k(&self, k: usize) -> cf_core::Result<Vec<ContextEntry>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    id,
                    content,
                    timestamp,
                    kind,
                    token_count,
                    session_id,
                    compaction_count,
                    compaction_trigger,
                    runtime,
                    model,
                    cwd,
                    git_branch,
                    git_sha,
                    turn_id,
                    agent_type,
                    agent_id
                 FROM entries
                 ORDER BY timestamp DESC
                 LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let entries = stmt
            .query_map([k as i64], row_to_entry)
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(entries)
    }

    fn get_all(&self) -> cf_core::Result<Vec<ContextEntry>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    id,
                    content,
                    timestamp,
                    kind,
                    token_count,
                    session_id,
                    compaction_count,
                    compaction_trigger,
                    runtime,
                    model,
                    cwd,
                    git_branch,
                    git_sha,
                    turn_id,
                    agent_type,
                    agent_id
                 FROM entries
                 ORDER BY timestamp DESC",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let entries = stmt
            .query_map([], row_to_entry)
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(entries)
    }

    fn delete(&self, id: &str) -> cf_core::Result<bool> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let changes = conn
            .execute("DELETE FROM entries WHERE id = ?1", [id])
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(changes > 0)
    }

    fn clear(&self) -> cf_core::Result<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let changes = conn
            .execute("DELETE FROM entries", [])
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(changes)
    }

    fn count(&self) -> cf_core::Result<usize> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(count as usize)
    }

    fn max_compaction_count(&self, session_id: &str) -> cf_core::Result<Option<i64>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        conn.query_row(
            "SELECT MAX(compaction_count) FROM entries WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|e| CoreError::Storage(e.to_string()))
    }
}
