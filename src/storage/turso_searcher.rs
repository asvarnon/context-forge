use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, QueryParser, TermQuery};
use tantivy::schema::{IndexRecordOption, OwnedValue};
use tantivy::{TantivyDocument, Term};

use crate::engine::MATCH_ALL_QUERY;
use crate::entry::ScoredEntry;
use crate::storage::fts_index::FtsIndex;
use crate::storage::turso_storage::turso_row_to_entry;
use crate::traits::Searcher;

/// Turso-backed searcher using a standalone tantivy index for real BM25 scoring.
///
/// On FTS queries: tantivy returns scored IDs → turso fetches full rows by ID.
/// On match-all: SQL ORDER BY timestamp DESC (no scoring needed).
pub struct TursoSearcher {
    db: Arc<turso::Database>,
    fts: Arc<FtsIndex>,
}

impl TursoSearcher {
    /// Create a new searcher sharing the given turso database and tantivy index.
    #[must_use]
    pub(crate) fn new(db: Arc<turso::Database>, fts: Arc<FtsIndex>) -> Self {
        Self { db, fts }
    }
}

#[async_trait]
impl Searcher for TursoSearcher {
    async fn search_semantic(
        &self,
        embedding: &[f32],
        scope: Option<&str>,
        limit: usize,
    ) -> crate::Result<Vec<crate::entry::ScoredEntry>> {
        self.search_semantic_impl(embedding, scope, limit).await
    }

    async fn search(
        &self,
        query: &str,
        scope: Option<&str>,
        limit: usize,
    ) -> crate::Result<Vec<ScoredEntry>> {
        if query == MATCH_ALL_QUERY {
            return self.search_all(scope, limit).await;
        }

        // --- tantivy BM25 search ---
        let searcher = self.fts.reader.searcher();
        // parse_query_lenient: OR-joins terms by default and never errors on bad syntax.
        let (content_query, _errors) =
            QueryParser::for_index(searcher.index(), vec![self.fts.content_field])
                .parse_query_lenient(query);

        // When a scope is requested, combine the content query with an exact-match
        // scope term so TopDocs is already scoped — no overfetch or Rust-side filter needed.
        let tantivy_query: Box<dyn tantivy::query::Query> = if let Some(s) = scope {
            let scope_term = Term::from_field_text(self.fts.scope_field, s);
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, content_query),
                (
                    Occur::Must,
                    Box::new(TermQuery::new(scope_term, IndexRecordOption::Basic)),
                ),
            ]))
        } else {
            content_query
        };

        let top_docs = searcher
            .search(&tantivy_query, &TopDocs::with_limit(limit))
            .map_err(|e| crate::Error::Migration(e.to_string()))?;

        if top_docs.is_empty() {
            return Ok(Vec::new());
        }

        // Collect (score, id) pairs from the tantivy index.
        let mut scored_ids: Vec<(f32, String)> = Vec::with_capacity(top_docs.len());
        for (score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher
                .doc(doc_address)
                .map_err(|e| crate::Error::Migration(e.to_string()))?;
            if let Some(OwnedValue::Str(id_str)) = doc.get_first(self.fts.id_field) {
                scored_ids.push((score, id_str.to_owned()));
            }
        }

        if scored_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch full rows from turso by ID.
        // IDs are UUID v7 strings generated internally — safe to inline directly.
        let id_list: String = scored_ids
            .iter()
            .map(|(_, id)| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
             FROM entries WHERE id IN ({id_list})"
        );

        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn.query(&sql, ()).await?;
        let mut id_to_entry = std::collections::HashMap::new();
        while let Some(row) = rows.next().await? {
            let entry = turso_row_to_entry(&row)?;
            id_to_entry.insert(entry.id.clone(), entry);
        }

        // Zip scores back in tantivy's ranked order.
        let mut result: Vec<ScoredEntry> = scored_ids
            .into_iter()
            .filter_map(|(score, id)| {
                let entry = id_to_entry.remove(&id)?;
                Some(ScoredEntry {
                    score: f64::from(score),
                    entry,
                })
            })
            .collect();

        // tantivy already sorted by score DESC; preserve that order.
        result.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(result)
    }
}

impl TursoSearcher {
    async fn search_semantic_impl(
        &self,
        embedding: &[f32],
        scope: Option<&str>,
        limit: usize,
    ) -> crate::Result<Vec<crate::entry::ScoredEntry>> {
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
        let scope_owned = scope.map(str::to_owned);
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata, \
                 vector_distance_cos(embedding, vector32(?1)) as dist \
                 FROM entries \
                 WHERE embedding IS NOT NULL AND (?2 IS NULL OR scope = ?2) \
                 ORDER BY dist ASC \
                 LIMIT ?3",
                (
                    vec_str,
                    scope_owned,
                    i64::try_from(limit).unwrap_or(i64::MAX),
                ),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            let entry = turso_row_to_entry(&row)?;
            let dist = match row.get_value(8)? {
                turso::Value::Real(f) => f,
                turso::Value::Integer(i) => i as f64,
                _ => 1.0,
            };
            let score = (1.0 - dist).max(0.0);
            result.push(crate::entry::ScoredEntry { score, entry });
        }

        tracing::debug!(
            count = %result.len(),
            "turso semantic search complete"
        );
        Ok(result)
    }

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
                (scope_owned, i64::try_from(limit).unwrap_or(i64::MAX)),
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
