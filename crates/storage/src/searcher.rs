use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use cf_core::entry::{ContextEntry, EntryKind, ScoredEntry};
use cf_core::error::CoreError;
use cf_core::traits::Searcher;
use cf_core::Result;

/// FTS5-backed full-text search over stored context entries.
pub struct SqliteSearcher {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SqliteSearcher {
    /// Create a new searcher sharing the given connection pool.
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self { pool }
    }
}

/// Parse a SQLite text value back into an `EntryKind`.
fn str_to_kind(s: &str) -> Result<EntryKind> {
    match s {
        "Manual" => Ok(EntryKind::Manual),
        "PreCompact" => Ok(EntryKind::PreCompact),
        "Auto" => Ok(EntryKind::Auto),
        other => Err(CoreError::Storage(format!("unknown EntryKind: {other}"))),
    }
}

impl Searcher for SqliteSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredEntry>> {
        let conn = self.pool.get().map_err(|e| CoreError::Storage(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT e.id, e.content, e.timestamp, e.kind, e.token_count, bm25(entries_fts) AS score \
                 FROM entries_fts f \
                 JOIN entries e ON e.rowid = f.rowid \
                 WHERE entries_fts MATCH ?1 \
                 ORDER BY score \
                 LIMIT ?2",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![query, limit as i64], |row| {
                let kind_str: String = row.get(3)?;
                let token_count: Option<i64> = row.get(4)?;
                let raw_score: f64 = row.get(5)?;

                let kind = str_to_kind(&kind_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

                Ok(ScoredEntry {
                    entry: ContextEntry {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        timestamp: row.get(2)?,
                        kind,
                        token_count: token_count.map(|v| v as usize),
                    },
                    // bm25() returns negative values; negate so higher = more relevant.
                    score: -raw_score,
                })
            })
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(results)
    }
}
