use std::path::Path;
use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Row;

use cf_core::entry::{ContextEntry, EntryKind};
use cf_core::error::CoreError;
use cf_core::traits::ContextStorage;

use crate::schema::migrate;

/// SQLite-backed implementation of [`ContextStorage`].
pub struct SqliteStorage {
    pool: Arc<Pool<SqliteConnectionManager>>,
    max_entries: usize,
}

impl SqliteStorage {
    /// Open (or create) a SQLite database at `db_path` and run migrations.
    ///
    /// Pass `Path::new(":memory:")` for an in-memory database (useful in tests).
    pub fn open(db_path: &Path, max_entries: usize) -> cf_core::Result<Self> {
        let manager = SqliteConnectionManager::file(db_path);
        let pool = Pool::builder()
            .max_size(4)
            .build(manager)
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let conn = pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;

        // Set PRAGMAs — WAL is ignored on :memory: databases but harmless.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")
            .map_err(|e| CoreError::Storage(e.to_string()))?;

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

/// Convert an `EntryKind` to its SQLite text representation.
fn kind_to_str(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Manual => "Manual",
        EntryKind::PreCompact => "PreCompact",
        EntryKind::Auto => "Auto",
    }
}

/// Parse a SQLite text value back into an `EntryKind`.
fn str_to_kind(s: &str) -> cf_core::Result<EntryKind> {
    match s {
        "Manual" => Ok(EntryKind::Manual),
        "PreCompact" => Ok(EntryKind::PreCompact),
        "Auto" => Ok(EntryKind::Auto),
        other => Err(CoreError::Storage(format!("unknown EntryKind: {other}"))),
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
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;

        // LRU eviction: if at capacity, delete the oldest entry.
        let current_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        let current_count = current_count as usize;

        if current_count >= self.max_entries {
            conn.execute(
                "DELETE FROM entries WHERE id = (SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                [],
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        }

        conn.execute(
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

        Ok(())
    }

    fn get_top_k(&self, k: usize) -> cf_core::Result<Vec<ContextEntry>> {
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
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
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
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
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
        let changes = conn
            .execute("DELETE FROM entries WHERE id = ?1", [id])
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(changes > 0)
    }

    fn clear(&self) -> cf_core::Result<usize> {
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
        let changes = conn
            .execute("DELETE FROM entries", [])
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(changes)
    }

    fn count(&self) -> cf_core::Result<usize> {
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM entries", [], |r| r.get(0))
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        Ok(count as usize)
    }
}
