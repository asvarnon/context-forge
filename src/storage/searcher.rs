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

/// Convert an arbitrary natural-language string into a safe FTS5 `MATCH`
/// expression.
///
/// Splits `query` on any character that is not Unicode-alphanumeric,
/// drops empty fragments, double-quotes each remaining term (escaping any
/// internal `"` as `""`), and joins the terms with `" OR "`. Quoting
/// ensures FTS5 operator characters in the input (`.`, `"`, `*`, `:`, `-`,
/// etc.) are never interpreted as query syntax. Returns `None` when `query`
/// contains no alphanumeric terms (e.g. empty or punctuation-only input).
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

impl Searcher for SqliteSearcher {
    fn search(&self, query: &str, scope: Option<&str>, limit: usize) -> Result<Vec<ScoredEntry>> {
        if query == MATCH_ALL_QUERY {
            return self.search_all(scope, limit);
        }

        let Some(match_expr) = to_fts5_match(query) else {
            return Ok(Vec::new());
        };

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
                rusqlite::params![match_expr, scope, i64::try_from(limit).unwrap_or(i64::MAX)],
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

#[cfg(test)]
mod tests {
    use super::to_fts5_match;

    #[test]
    fn single_term() {
        assert_eq!(to_fts5_match("marco"), Some("\"marco\"".to_owned()));
    }

    #[test]
    fn multi_word_with_punctuation() {
        let input = r#"if I say "marco" you say "polo"."#;
        let expected = "\"if\" OR \"I\" OR \"say\" OR \"marco\" OR \"you\" OR \"say\" OR \"polo\"";
        assert_eq!(to_fts5_match(input), Some(expected.to_owned()));
    }

    #[test]
    fn punctuation_only_returns_none() {
        assert_eq!(to_fts5_match("...:-*"), None);
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(to_fts5_match(""), None);
    }

    #[test]
    fn hyphen_and_colon_split_terms() {
        assert_eq!(
            to_fts5_match("well-known scope:test"),
            Some("\"well\" OR \"known\" OR \"scope\" OR \"test\"".to_owned())
        );
    }

    #[test]
    fn unicode_term_survives_as_single_term() {
        assert_eq!(to_fts5_match("café"), Some("\"café\"".to_owned()));
    }
}
