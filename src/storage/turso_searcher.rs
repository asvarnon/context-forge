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

        // fts_score(content, ?1) occupies column 8. It returns a real BM25 value when
        // the optimizer routes the query through the Tantivy index scan; otherwise it
        // returns 0.0 (corpus-level IDF statistics are only available inside that scan
        // context — fts_score has no per-row fallback the way fts_match does).
        // Scope filtering is done in Rust so the WHERE clause stays simple enough for
        // the optimizer to potentially recognize the fts_score + fts_match pattern.
        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata, \
                 fts_score(content, ?1) AS score \
                 FROM entries \
                 WHERE fts_match(content, ?1) \
                 ORDER BY score DESC \
                 LIMIT ?2",
                (
                    match_expr,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let entry = turso_row_to_entry(&row)?;
            // Apply scope filter in Rust: the SQL WHERE omits it so the query pattern
            // stays closer to what the optimizer recognizes for fts_score.
            if scope_owned.is_some() && entry.scope.as_deref() != scope_owned.as_deref() {
                continue;
            }
            let score = match row.get_value(8)? {
                turso::Value::Real(f) => f,
                turso::Value::Integer(i) => i as f64,
                turso::Value::Null => 0.0,
                other => {
                    return Err(crate::Error::Migration(format!(
                        "fts_score: unexpected value type {other:?}"
                    )))
                }
            };
            result.push(ScoredEntry { score, entry });
        }
        result.truncate(limit);

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
