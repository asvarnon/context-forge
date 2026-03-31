use std::path::Path;
use std::sync::Arc;

use r2d2::{CustomizeConnection, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Row;

use cf_core::entry::ContextEntry;
use cf_core::error::CoreError;
use cf_core::traits::ContextStorage;

use crate::schema::{kind_to_str, migrate, str_to_kind};

#[derive(Debug)]
struct PragmaCustomizer;

impl CustomizeConnection<rusqlite::Connection, rusqlite::Error> for PragmaCustomizer {
    fn on_acquire(
        &self,
        conn: &mut rusqlite::Connection,
    ) -> std::result::Result<(), rusqlite::Error> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
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
}

/// Map a `rusqlite::Row` (from `SELECT * FROM entries …`) to a `ContextEntry`.
fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<ContextEntry> {
    let kind_str: String = row.get(3)?;
    let token_count: Option<i64> = row.get(4)?;
    Ok(ContextEntry {
        id: row.get(0)?,
        content: row.get(1)?,
        timestamp: row.get(2)?,
        kind: str_to_kind(&kind_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
        token_count: token_count.map(|v| v as usize),
    })
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
            "INSERT OR REPLACE INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                entry.id,
                entry.content,
                entry.timestamp,
                kind_to_str(&entry.kind),
                entry.token_count.map(|v| v as i64),
            ],
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;

        tx.commit().map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(())
    }

    fn get_top_k(&self, k: usize) -> cf_core::Result<Vec<ContextEntry>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT id, content, timestamp, kind, token_count FROM entries ORDER BY timestamp DESC LIMIT ?1")
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
            .prepare("SELECT id, content, timestamp, kind, token_count FROM entries ORDER BY timestamp DESC")
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
}
