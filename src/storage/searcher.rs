use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use crate::engine::MATCH_ALL_QUERY;
use crate::entry::ScoredEntry;
use crate::storage::schema::row_to_entry;
use crate::traits::Searcher;
use crate::Result;

/// FTS5-backed full-text search over stored context entries.
pub struct SqliteSearcher {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SqliteSearcher {
    /// Create a new searcher sharing the given connection pool.
    #[must_use]
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self { pool }
    }

    /// Return all entries (optionally restricted to `scope`) with a fixed
    /// score of `1.0`, ordered by timestamp descending.
    ///
    /// This implements the `MATCH_ALL_QUERY` contract without relying on FTS5
    /// MATCH, which does not support a bare `*` glob. A fixed score keeps the
    /// scale consistent with the BM25 path's "higher = better" convention;
    /// the engine's recency decay (monotonic in age) preserves ordering.
    fn search_all(&self, scope: Option<&str>, limit: usize) -> Result<Vec<ScoredEntry>> {
        let conn = self.pool.get()?;

        let mut stmt = conn.prepare(
            "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
             FROM entries \
             WHERE (?1 IS NULL OR scope = ?1) \
             ORDER BY timestamp DESC \
             LIMIT ?2",
        )?;

        let results = stmt
            .query_map(
                rusqlite::params![scope, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    let entry = row_to_entry(row)?;
                    Ok(ScoredEntry { score: 1.0, entry })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(results)
    }
}

impl Searcher for SqliteSearcher {
    fn search(&self, query: &str, scope: Option<&str>, limit: usize) -> Result<Vec<ScoredEntry>> {
        if query == MATCH_ALL_QUERY {
            return self.search_all(scope, limit);
        }

        let conn = self.pool.get()?;

        let mut stmt = conn.prepare(
            "SELECT e.id, e.content, e.timestamp, e.kind, e.scope, e.session_id, \
                    e.token_count, e.metadata, bm25(entries_fts) AS score \
             FROM entries_fts f \
             JOIN entries e ON e.rowid = f.rowid \
             WHERE entries_fts MATCH ?1 AND (?2 IS NULL OR e.scope = ?2) \
             ORDER BY score \
             LIMIT ?3",
        )?;

        let results = stmt
            .query_map(
                rusqlite::params![query, scope, i64::try_from(limit).unwrap_or(i64::MAX)],
                |row| {
                    let entry = row_to_entry(row)?;
                    let raw_score: f64 = row.get("score")?;

                    Ok(ScoredEntry {
                        entry,
                        // bm25() returns negative values; negate so higher = more relevant.
                        score: -raw_score,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(results)
    }
}
