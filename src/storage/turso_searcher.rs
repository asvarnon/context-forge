use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::engine::MATCH_ALL_QUERY;
use crate::entry::ScoredEntry;
use crate::storage::turso_storage::turso_row_to_entry;
use crate::traits::Searcher;

/// Turso-backed full-text searcher (tantivy FTS index via `USING fts`).
pub struct TursoSearcher {
    db: Arc<turso::Database>,
}

impl TursoSearcher {
    /// Create a new searcher sharing the given turso database handle.
    #[must_use]
    pub fn new(db: Arc<turso::Database>) -> Self {
        Self { db }
    }
}

// Converts a raw user query into a Tantivy-compatible query string.
// Splits on non-alphanumeric chars, wraps each term in double quotes (phrase query),
// and joins with OR. Returns None if the query has no alphanumeric terms.
// The resulting format — `"term1" OR "term2"` — is valid for both FTS5 and
// Tantivy's QueryParser, which turso uses under the hood.
fn to_fts5_match(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();

    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

#[async_trait]
impl Searcher for TursoSearcher {
    async fn search(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> crate::Result<Vec<ScoredEntry>> {
        if query == MATCH_ALL_QUERY {
            return self.search_all(scope, limit).await;
        }

        let Some(match_expr) = to_fts5_match(query) else {
            return Ok(Vec::new());
        };

        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let scope_owned = scope.map(str::to_owned);

        // Use fts_match() instead of the MATCH operator: the operator form breaks
        // under compound WHERE clauses; fts_match() works as a standalone scalar
        // function (documented as usable without an FTS index).
        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
                 FROM entries \
                 WHERE fts_match(content, ?1) AND (?2 IS NULL OR scope = ?2) \
                 ORDER BY timestamp DESC \
                 LIMIT ?3",
                (
                    match_expr,
                    scope_owned,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let entry = turso_row_to_entry(&row)?;
            // BM25 scoring via fts_score() is deferred to Step 4 (tantivy integration).
            result.push(ScoredEntry { score: 1.0, entry });
        }

        Ok(result)
    }
}

impl TursoSearcher {
    async fn search_all(
        &self,
        scope: Option<&str>,
        limit: usize,
    ) -> crate::Result<Vec<ScoredEntry>> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let scope_owned = scope.map(str::to_owned);

        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
                 FROM entries \
                 WHERE (?1 IS NULL OR scope = ?1) \
                 ORDER BY timestamp DESC \
                 LIMIT ?2",
                (
                    scope_owned,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let entry = turso_row_to_entry(&row)?;
            result.push(ScoredEntry { score: 1.0, entry });
        }

        Ok(result)
    }
}
