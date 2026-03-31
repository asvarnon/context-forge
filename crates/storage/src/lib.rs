pub mod schema;
pub mod searcher;
pub mod storage;

pub use searcher::SqliteSearcher;
pub use storage::SqliteStorage;

/// Create a paired storage + searcher sharing the same connection pool.
pub fn open_storage(
    db_path: &std::path::Path,
    max_entries: usize,
) -> cf_core::Result<(SqliteStorage, SqliteSearcher)> {
    let storage = SqliteStorage::open(db_path, max_entries)?;
    let searcher = SqliteSearcher::new(storage.pool());
    Ok((storage, searcher))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cf_core::engine::MATCH_ALL_QUERY;
    use cf_core::entry::{ContextEntry, EntryKind};
    use cf_core::traits::{ContextStorage, Searcher};

    use crate::{open_storage, SqliteStorage};

    fn make_entry(id: &str, content: &str, timestamp: i64, kind: EntryKind) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind,
            token_count: None,
        }
    }

    #[test]
    fn test_save_and_count() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        let entry = make_entry("e1", "hello world", 1000, EntryKind::Manual);
        storage.save(&entry).unwrap();
        assert_eq!(storage.count().unwrap(), 1);
    }

    #[test]
    fn test_save_and_get_top_k() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first", 100, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, EntryKind::PreCompact))
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, EntryKind::Auto))
            .unwrap();

        let top2 = storage.get_top_k(2).unwrap();
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].id, "e3"); // most recent
        assert_eq!(top2[1].id, "e2");
    }

    #[test]
    fn test_save_and_get_all() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first", 100, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e2", "second", 200, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e3", "third", 300, EntryKind::Manual))
            .unwrap();

        let all = storage.get_all().unwrap();
        assert_eq!(all.len(), 3);
        // ordered by timestamp desc
        assert_eq!(all[0].id, "e3");
        assert_eq!(all[1].id, "e2");
        assert_eq!(all[2].id, "e1");
    }

    #[test]
    fn test_delete() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "hello", 1000, EntryKind::Manual))
            .unwrap();

        assert!(storage.delete("e1").unwrap());
        assert!(!storage.delete("nonexistent").unwrap());
        assert_eq!(storage.count().unwrap(), 0);
    }

    #[test]
    fn test_clear() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "a", 100, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e2", "b", 200, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e3", "c", 300, EntryKind::Manual))
            .unwrap();

        let cleared = storage.clear().unwrap();
        assert_eq!(cleared, 3);
        assert_eq!(storage.count().unwrap(), 0);
    }

    #[test]
    fn test_lru_eviction() {
        let (storage, _) = open_storage(Path::new(":memory:"), 2).unwrap();
        storage
            .save(&make_entry("e1", "oldest", 100, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e2", "middle", 200, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e3", "newest", 300, EntryKind::Manual))
            .unwrap();

        assert_eq!(storage.count().unwrap(), 2);

        let all = storage.get_all().unwrap();
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        assert!(
            !ids.contains(&"e1"),
            "oldest entry should have been evicted"
        );
        assert!(ids.contains(&"e2"));
        assert!(ids.contains(&"e3"));
    }

    #[test]
    fn test_fts_search() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry(
                "e1",
                "rust programming language",
                100,
                EntryKind::Manual,
            ))
            .unwrap();
        storage
            .save(&make_entry(
                "e2",
                "python scripting",
                200,
                EntryKind::Manual,
            ))
            .unwrap();
        storage
            .save(&make_entry(
                "e3",
                "rust borrow checker",
                300,
                EntryKind::Manual,
            ))
            .unwrap();

        let results = searcher.search("rust", 5).unwrap();
        assert_eq!(results.len(), 2);
        // Assert ordering by relevance (highest score first), not absolute values.
        assert!(
            results[0].score >= results[1].score,
            "results should be ordered by descending score"
        );
    }

    #[test]
    fn test_fts_search_no_results() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "hello world", 100, EntryKind::Manual))
            .unwrap();

        let results = searcher.search("nonexistent", 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_schema_migration_idempotent() {
        let storage1 = SqliteStorage::open(Path::new(":memory:"), 100).unwrap();
        let conn = storage1.pool().get().unwrap();
        // Running migrate a second time on the same connection should succeed.
        crate::schema::migrate(&conn).unwrap();
    }

    #[test]
    fn test_insert_or_replace() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry(
                "e1",
                "original content",
                100,
                EntryKind::Manual,
            ))
            .unwrap();
        storage
            .save(&make_entry("e1", "updated content", 200, EntryKind::Auto))
            .unwrap();

        assert_eq!(storage.count().unwrap(), 1);

        let all = storage.get_all().unwrap();
        assert_eq!(all[0].content, "updated content");
    }

    #[test]
    fn test_search_match_all_query() {
        let (storage, searcher) = open_storage(Path::new(":memory:"), 100).unwrap();
        storage
            .save(&make_entry("e1", "first entry", 100, EntryKind::Manual))
            .unwrap();
        storage
            .save(&make_entry("e2", "second entry", 200, EntryKind::PreCompact))
            .unwrap();
        storage
            .save(&make_entry("e3", "third entry", 300, EntryKind::Auto))
            .unwrap();

        let results = searcher.search(MATCH_ALL_QUERY, 10).unwrap();
        assert_eq!(results.len(), 3);

        // Ordered by score descending (newest first since score = timestamp)
        assert_eq!(results[0].entry.id, "e3");
        assert_eq!(results[1].entry.id, "e2");
        assert_eq!(results[2].entry.id, "e1");

        // Scores correspond to timestamps
        assert!((results[0].score - 300.0).abs() < f64::EPSILON);
        assert!((results[1].score - 200.0).abs() < f64::EPSILON);
        assert!((results[2].score - 100.0).abs() < f64::EPSILON);

        // Descending score order
        assert!(results[0].score >= results[1].score);
        assert!(results[1].score >= results[2].score);
    }
}
