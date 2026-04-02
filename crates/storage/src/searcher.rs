use std::sync::Arc;

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;

use cf_core::engine::MATCH_ALL_QUERY;
use cf_core::entry::ScoredEntry;
use cf_core::error::CoreError;
use cf_core::traits::Searcher;
use cf_core::Result;

use crate::schema::row_to_entry;

/// FTS5-backed full-text search over stored context entries.
pub struct SqliteSearcher {
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SqliteSearcher {
    /// Create a new searcher sharing the given connection pool.
    pub fn new(pool: Arc<Pool<SqliteConnectionManager>>) -> Self {
        Self { pool }
    }

    /// Return all entries with score derived from timestamp, ordered by timestamp descending.
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
                "SELECT id, content, timestamp, kind, token_count, \
                        session_id, compaction_count, compaction_trigger, runtime, model, cwd, \
                        git_branch, git_sha, turn_id, agent_type, agent_id \
                 FROM entries \
                 ORDER BY timestamp DESC \
                 LIMIT ?1",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                let entry = row_to_entry(row)?;

                Ok(ScoredEntry {
                    score: entry.timestamp as f64,
                    entry,
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
                "SELECT e.id, e.content, e.timestamp, e.kind, e.token_count, \
                        e.session_id, e.compaction_count, e.compaction_trigger, e.runtime, \
                        e.model, e.cwd, e.git_branch, e.git_sha, e.turn_id, e.agent_type, \
                        e.agent_id, bm25(entries_fts) AS score \
                 FROM entries_fts f \
                 JOIN entries e ON e.rowid = f.rowid \
                 WHERE entries_fts MATCH ?1 \
                 ORDER BY score \
                 LIMIT ?2",
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        let results = stmt
            .query_map(rusqlite::params![query, limit as i64], |row| {
                let entry = row_to_entry(row)?;
                let raw_score: f64 = row.get("score")?;

                Ok(ScoredEntry {
                    entry,
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
