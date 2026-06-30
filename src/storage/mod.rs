use std::path::Path;
use std::sync::Arc;

/// Shared in-memory tantivy FTS index.
pub(crate) mod fts_index;
/// Turso-backed async FTS searcher.
pub mod turso_searcher;
/// Turso-backed async storage implementation.
pub mod turso_storage;

pub use turso_searcher::TursoSearcher;
pub use turso_storage::TursoStorage;

/// Create a paired storage + searcher backed by turso.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or migrations fail.
pub async fn open_storage(
    db_path: &Path,
    max_entries: usize,
) -> crate::Result<(TursoStorage, TursoSearcher)> {
    let storage = TursoStorage::open(db_path, max_entries).await?;
    let db = Arc::clone(&storage.db);
    let fts = Arc::clone(&storage.fts);
    let searcher = TursoSearcher::new(db, fts);
    Ok((storage, searcher))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::engine::MATCH_ALL_QUERY;
    use crate::entry::{kind, ContextEntry};
    use crate::storage::open_storage;
    use crate::traits::{ContextStorage, Searcher};

    fn make_entry(id: &str, content: &str, timestamp: i64, kind: &str) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: kind.to_owned(),
            scope: None,
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }

    fn make_scoped_entry(id: &str, content: &str, timestamp: i64, scope: &str) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind: kind::MANUAL.to_owned(),
            scope: Some(scope.to_owned()),
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }

    #[tokio::test]
    async fn test_save_and_count() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        let entry = make_entry("e1", "hello world", 1000, kind::MANUAL);
        storage.save(&entry).await.unwrap();
        assert_eq!(storage.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_save_and_get_top_k() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "first", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, kind::SNAPSHOT))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, kind::SUMMARY))
            .await
            .unwrap();

        let top2 = storage.get_top_k(2).await.unwrap();
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].id, "e3"); // most recent
        assert_eq!(top2[1].id, "e2");
    }

    #[tokio::test]
    async fn test_save_and_get_all() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "first", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, kind::MANUAL))
            .await
            .unwrap();

        let all = storage.get_all().await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].id, "e3");
        assert_eq!(all[1].id, "e2");
        assert_eq!(all[2].id, "e1");
    }

    #[tokio::test]
    async fn test_delete() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "hello", 1000, kind::MANUAL))
            .await
            .unwrap();

        assert!(storage.delete("e1").await.unwrap());
        assert!(!storage.delete("nonexistent").await.unwrap());
        assert_eq!(storage.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_clear() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "a", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "b", 200, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "c", 300, kind::MANUAL))
            .await
            .unwrap();

        let cleared = storage.clear().await.unwrap();
        assert_eq!(cleared, 3);
        assert_eq!(storage.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_clear_scope() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_scoped_entry("e1", "a", 100, "scope-a"))
            .await
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "b", 200, "scope-a"))
            .await
            .unwrap();
        storage
            .save(&make_scoped_entry("e3", "c", 300, "scope-b"))
            .await
            .unwrap();

        let cleared = storage.clear_scope("scope-a").await.unwrap();
        assert_eq!(cleared, 2);
        assert_eq!(storage.count().await.unwrap(), 1);

        let all = storage.get_all().await.unwrap();
        assert_eq!(all[0].id, "e3");
    }

    #[tokio::test]
    async fn test_clear_scope_no_match() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_scoped_entry("e1", "a", 100, "scope-a"))
            .await
            .unwrap();

        let cleared = storage.clear_scope("scope-z").await.unwrap();
        assert_eq!(cleared, 0);
        assert_eq!(storage.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let (storage, _) = open_storage(Path::new(":memory:"), 2).await.unwrap();
        storage
            .save(&make_entry("e1", "oldest", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "middle", 200, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "newest", 300, kind::MANUAL))
            .await
            .unwrap();

        assert_eq!(storage.count().await.unwrap(), 2);

        let all = storage.get_all().await.unwrap();
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !ids.contains(&"e1"),
            "oldest entry should have been evicted"
        );
        assert!(ids.contains(&"e2"));
        assert!(ids.contains(&"e3"));
    }

    #[tokio::test]
    async fn test_fts_search() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry(
                "e1",
                "rust programming language",
                100,
                kind::MANUAL,
            ))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "python scripting", 200, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "rust borrow checker", 300, kind::MANUAL))
            .await
            .unwrap();

        let results = searcher.search("rust", None, 5).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(
            results[0].score >= results[1].score,
            "results should be ordered by descending score"
        );
    }

    #[tokio::test]
    async fn test_fts_search_no_results() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "hello world", 100, kind::MANUAL))
            .await
            .unwrap();

        let results = searcher.search("nonexistent", None, 5).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_fts_search_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_scoped_entry("e1", "rust programming", 100, "a"))
            .await
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "rust borrow checker", 200, "b"))
            .await
            .unwrap();

        let results_a = searcher.search("rust", Some("a"), 5).await.unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_b = searcher.search("rust", Some("b"), 5).await.unwrap();
        assert_eq!(results_b.len(), 1);
        assert_eq!(results_b[0].entry.id, "e2");

        let results_all = searcher.search("rust", None, 5).await.unwrap();
        assert_eq!(results_all.len(), 2);
    }

    #[tokio::test]
    async fn test_new_entry_with_scope_and_metadata() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();

        let metadata = serde_json::json!({"runtime": "codex", "model": "gpt-5.3-codex"});
        let entry = ContextEntry {
            id: "v3-entry".into(),
            content: "entry with scope and metadata".into(),
            timestamp: 1_700_000_100,
            kind: kind::SUMMARY.to_owned(),
            scope: Some("project:homelab-rs".into()),
            session_id: Some("session-123".into()),
            token_count: Some(6),
            metadata: Some(metadata.clone()),
        };

        storage.save(&entry).await.unwrap();

        let all = storage.get_all().await.unwrap();
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.id, entry.id);
        assert_eq!(got.content, entry.content);
        assert_eq!(got.timestamp, entry.timestamp);
        assert_eq!(got.kind, entry.kind);
        assert_eq!(got.scope, entry.scope);
        assert_eq!(got.session_id, entry.session_id);
        assert_eq!(got.token_count, entry.token_count);
        assert_eq!(got.metadata, Some(metadata));
    }

    #[tokio::test]
    async fn test_insert_or_replace() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "original content", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e1", "updated content", 200, kind::SUMMARY))
            .await
            .unwrap();

        assert_eq!(storage.count().await.unwrap(), 1);

        let all = storage.get_all().await.unwrap();
        assert_eq!(all[0].content, "updated content");
    }

    #[tokio::test]
    async fn test_search_match_all_query() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "first entry", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "second entry", 200, kind::SNAPSHOT))
            .await
            .unwrap();
        storage
            .save(&make_entry("e3", "third entry", 300, kind::SUMMARY))
            .await
            .unwrap();

        let results = searcher.search(MATCH_ALL_QUERY, None, 10).await.unwrap();
        assert_eq!(results.len(), 3);

        // Ordered by timestamp descending (newest first).
        assert_eq!(results[0].entry.id, "e3");
        assert_eq!(results[1].entry.id, "e2");
        assert_eq!(results[2].entry.id, "e1");

        // Match-all results all share the same fixed score.
        for r in &results {
            assert!((r.score - 1.0).abs() < f64::EPSILON);
        }
    }

    #[tokio::test]
    async fn test_search_match_all_query_scoped() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_scoped_entry("e1", "first entry", 100, "a"))
            .await
            .unwrap();
        storage
            .save(&make_scoped_entry("e2", "second entry", 200, "b"))
            .await
            .unwrap();

        let results_a = searcher
            .search(MATCH_ALL_QUERY, Some("a"), 10)
            .await
            .unwrap();
        assert_eq!(results_a.len(), 1);
        assert_eq!(results_a[0].entry.id, "e1");

        let results_all = searcher.search(MATCH_ALL_QUERY, None, 10).await.unwrap();
        assert_eq!(results_all.len(), 2);
    }

    #[tokio::test]
    async fn search_with_fts5_operator_characters_does_not_error() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "marco polo", 100, kind::MANUAL))
            .await
            .unwrap();

        // Production bug: a query containing `"` and `.` previously caused
        // `fts5: syntax error`. It must now succeed and find the entry.
        let results = searcher
            .search(r#"if I say "marco"."#, None, 10)
            .await
            .unwrap();

        assert!(
            results.iter().any(|r| r.entry.id == "e1"),
            "expected 'marco polo' entry to be found despite FTS5 operator characters in the query"
        );
    }

    #[tokio::test]
    async fn test_fts_search_scores_are_nonzero() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry(
                "e1",
                "rust programming language",
                100,
                kind::MANUAL,
            ))
            .await
            .unwrap();
        storage
            .save(&make_entry(
                "e2",
                "python scripting tools",
                200,
                kind::MANUAL,
            ))
            .await
            .unwrap();

        let results = searcher.search("rust", None, 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(
            results[0].score > 0.0,
            "expected non-zero BM25 score from tantivy, got {}",
            results[0].score
        );
    }

    #[tokio::test]
    async fn search_or_joins_terms_for_message_length_queries() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).await.unwrap();
        storage
            .save(&make_entry("e1", "alpha one", 100, kind::MANUAL))
            .await
            .unwrap();
        storage
            .save(&make_entry("e2", "beta two", 200, kind::MANUAL))
            .await
            .unwrap();

        // Under implicit AND, no entry contains all of "alpha", "beta", and
        // "gamma", so this would return zero results. OR-join must return both.
        let results = searcher.search("alpha beta gamma", None, 10).await.unwrap();

        let ids: Vec<&str> = results.iter().map(|r| r.entry.id.as_str()).collect();
        assert!(ids.contains(&"e1"), "expected 'alpha one' entry in results");
        assert!(ids.contains(&"e2"), "expected 'beta two' entry in results");
    }
}
