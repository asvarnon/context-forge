use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use cf_core::engine::MATCH_ALL_QUERY;
use cf_core::entry::{ContextEntry, ScoredEntry};
use cf_core::error::CoreError;
use cf_core::traits::Searcher;
use cf_core::Result;

use crate::schema::str_to_kind;

/// FTS5-backed full-text search over stored context entries.
pub struct SqliteSearcher {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SqliteSearcher {
    /// Create a new searcher sharing the given connection pool.
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self { pool }
    }

    /// Return all entries with a uniform score of 1.0, ordered by timestamp descending.
    ///
    /// This implements the `MATCH_ALL_QUERY` contract without relying on FTS5 MATCH,
    /// which does not support a bare `*` glob.
    fn search_all(&self, limit: usize) -> Result<Vec<ScoredEntry>> {
        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, content, timestamp, kind, token_count \
                 FROM entries \
                 ORDER BY timestamp DESC \
                 LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let kind_str: String = row.get(3)?;
                let token_count: Option<i64> = row.get(4)?;

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
                    score: 1.0,
                })
            })
            .map_err(|e| CoreError::Storage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        Ok(results)
    }
}

impl Searcher for SqliteSearcher {
    fn search(&self, query: &str, limit: usize) -> Result<Vec<ScoredEntry>> {
        if query == MATCH_ALL_QUERY {
            return self.search_all(limit);
        }

        let conn = self
            .pool
            .get()
            .map_err(|e| CoreError::Storage(e.to_string()))?;

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
