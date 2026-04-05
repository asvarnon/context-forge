pub mod adapter;
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
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    use cf_core::engine::MATCH_ALL_QUERY;
    use cf_core::entry::{ContextEntry, EntryKind};
    use cf_core::traits::{ContextStorage, Searcher};
    use rusqlite::Connection;
    use serde_json::json;

    use crate::{open_storage, SqliteStorage};

    fn temp_db_path(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("cf-storage-{name}-{nanos}.db"))
    }

    fn make_entry(id: &str, content: &str, timestamp: i64, kind: EntryKind) -> ContextEntry {
        ContextEntry {
            id: id.into(),
            content: content.into(),
            timestamp,
            kind,
            token_count: None,
            session_id: None,
            compaction_count: None,
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
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
    fn test_v2_migration_idempotent() {
        let storage1 = SqliteStorage::open(Path::new(":memory:"), 100).unwrap();
        let conn = storage1.pool().get().unwrap();
        // Running migrate a second time on the same connection should succeed.
        crate::schema::migrate(&conn).unwrap();
    }

    #[test]
    fn test_v1_to_v2_migration() {
        let db_path = temp_db_path("v1-to-v2");

        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(crate::schema::SCHEMA_V1).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_version (id INTEGER PRIMARY KEY CHECK(id = 1), version INTEGER NOT NULL)",
            )
            .unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 1)",
                [],
            )
            .unwrap();

            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["m1", "manual entry", 100_i64, "Manual", 2_i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["p1", "precompact entry", 200_i64, "PreCompact", 3_i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entries (id, content, timestamp, kind, token_count) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params!["a1", "auto entry", 300_i64, "Auto", 4_i64],
            )
            .unwrap();
        }

        let storage = SqliteStorage::open(&db_path, 100).unwrap();
        let conn = storage.pool().get().unwrap();

        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, 2);

        let null_runtime_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entries WHERE runtime IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_runtime_count, 3);

        let null_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entries
                 WHERE session_id IS NULL
                   AND compaction_count IS NULL
                   AND compaction_trigger IS NULL
                   AND model IS NULL
                   AND cwd IS NULL
                   AND git_branch IS NULL
                   AND git_sha IS NULL
                   AND turn_id IS NULL
                   AND agent_type IS NULL
                   AND agent_id IS NULL
                   AND embedding IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(null_count, 3);

        let runtime_configs: i64 = conn
            .query_row("SELECT COUNT(*) FROM runtime_configs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runtime_configs, 5);

        let field_mappings: i64 = conn
            .query_row("SELECT COUNT(*) FROM runtime_field_mappings", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(field_mappings, 14);

        let tags: i64 = conn
            .query_row("SELECT COUNT(*) FROM tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tags, 0, "tags table should exist but be empty");

        let entry_tags: i64 = conn
            .query_row("SELECT COUNT(*) FROM entry_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(entry_tags, 0, "entry_tags table should exist but be empty");

        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn test_new_entry_with_v2_fields() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        let entry = ContextEntry {
            id: "v2-entry".into(),
            content: "entry with runtime metadata".into(),
            timestamp: 1_700_000_100,
            kind: EntryKind::Auto,
            token_count: Some(6),
            session_id: Some("session-123".into()),
            compaction_count: Some(2),
            compaction_trigger: Some("matcher:threshold".into()),
            runtime: Some("codex".into()),
            model: Some("gpt-5.3-codex".into()),
            cwd: Some("/workspace/context-forge".into()),
            git_branch: Some("feature/schema-v2".into()),
            git_sha: Some("abc123def".into()),
            turn_id: Some("turn-77".into()),
            agent_type: Some("coder".into()),
            agent_id: Some("agent-main".into()),
        };

        storage.save(&entry).unwrap();

        let all = storage.get_all().unwrap();
        assert_eq!(all.len(), 1);
        let got = &all[0];
        assert_eq!(got.id, entry.id);
        assert_eq!(got.content, entry.content);
        assert_eq!(got.timestamp, entry.timestamp);
        assert_eq!(got.kind, entry.kind);
        assert_eq!(got.token_count, entry.token_count);
        assert_eq!(got.session_id, entry.session_id);
        assert_eq!(got.compaction_count, entry.compaction_count);
        assert_eq!(got.compaction_trigger, entry.compaction_trigger);
        assert_eq!(got.runtime, entry.runtime);
        assert_eq!(got.model, entry.model);
        assert_eq!(got.cwd, entry.cwd);
        assert_eq!(got.git_branch, entry.git_branch);
        assert_eq!(got.git_sha, entry.git_sha);
        assert_eq!(got.turn_id, entry.turn_id);
        assert_eq!(got.agent_type, entry.agent_type);
        assert_eq!(got.agent_id, entry.agent_id);
    }

    #[test]
    fn test_save_with_metadata_claude_code_sets_fields_and_persists_raw_json() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        let mut entry = make_entry(
            "meta-claude",
            "compact summary",
            1_700_000_111,
            EntryKind::Auto,
        );
        entry.session_id = Some("existing-session".into());

        let raw_json = json!({
            "source": "compact",
            "session_id": "mapped-session",
            "model": "claude-3.7",
            "cwd": "/tmp/project",
            "matcher_value": "token_limit",
            "agent_type": "assistant",
            "agent_id": "agent-1"
        });

        storage
            .save_with_metadata(&mut entry, &raw_json, None)
            .unwrap();

        let stored = storage.get_all().unwrap();
        assert_eq!(stored.len(), 1);
        let got = &stored[0];

        // Existing session_id is preserved and never overwritten by mappings.
        assert_eq!(got.session_id.as_deref(), Some("existing-session"));
        assert_eq!(got.runtime.as_deref(), Some("claude-code"));
        assert_eq!(got.model.as_deref(), Some("claude-3.7"));
        assert_eq!(got.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(got.compaction_trigger.as_deref(), Some("token_limit"));
        assert_eq!(got.agent_type.as_deref(), Some("assistant"));
        assert_eq!(got.agent_id.as_deref(), Some("agent-1"));

        let conn = storage.pool().get().unwrap();
        let (runtime, raw): (String, String) = conn
            .query_row(
                "SELECT runtime, raw_json FROM entry_metadata_raw WHERE entry_id = ?1",
                [&entry.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(runtime, "claude-code");
        assert_eq!(raw, serde_json::to_string(&raw_json).unwrap());
    }

    #[test]
    fn test_save_with_metadata_codex_maps_thread_id_and_git_fields() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        let mut entry = make_entry(
            "meta-codex",
            "codex summary",
            1_700_000_222,
            EntryKind::Auto,
        );
        let raw_json = json!({
            "threadId": "thread-42",
            "turnId": "turn-9",
            "git": {
                "branch": "feature/runtime-adapter",
                "sha": "abc123"
            }
        });

        storage
            .save_with_metadata(&mut entry, &raw_json, None)
            .unwrap();

        let stored = storage.get_all().unwrap();
        assert_eq!(stored.len(), 1);
        let got = &stored[0];

        assert_eq!(got.runtime.as_deref(), Some("codex"));
        assert_eq!(got.session_id.as_deref(), Some("thread-42"));
        assert_eq!(got.turn_id.as_deref(), Some("turn-9"));
        assert_eq!(got.git_branch.as_deref(), Some("feature/runtime-adapter"));
        assert_eq!(got.git_sha.as_deref(), Some("abc123"));

        let conn = storage.pool().get().unwrap();
        let metadata_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entry_metadata_raw WHERE entry_id = ?1",
                [&entry.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metadata_count, 1);
    }

    #[test]
    fn test_save_with_metadata_cline_runtime() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        let payload = json!({
            "sessionId": "sid-cline-1",
            "model": "claude-3.5-sonnet"
        });

        let mut entry = make_entry("cline1", "cline conversation", 1000, EntryKind::Auto);
        storage
            .save_with_metadata(&mut entry, &payload, None)
            .unwrap();

        assert_eq!(entry.runtime.as_deref(), Some("cline"));
    }

    #[test]
    fn test_save_with_metadata_openclaw_runtime() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        let payload = json!({
            "sessionKey": "sk-oc-1",
            "cwd": "/home/user/project"
        });

        let mut entry = make_entry("oc1", "openclaw conversation", 1000, EntryKind::Auto);
        storage
            .save_with_metadata(&mut entry, &payload, None)
            .unwrap();

        assert_eq!(entry.runtime.as_deref(), Some("openclaw"));
    }

    #[test]
    fn test_save_with_metadata_gemini_runtime() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();
        let payload = json!({
            "session_id": "gem-sess-1",
            "model": "gemini-2.0-flash"
        });

        let mut entry = make_entry("gem1", "gemini conversation", 1000, EntryKind::Auto);
        entry.session_id = None; // ensure adapter fills it
        storage
            .save_with_metadata(&mut entry, &payload, None)
            .unwrap();

        assert_eq!(entry.runtime.as_deref(), Some("gemini"));
        assert_eq!(entry.session_id.as_deref(), Some("gem-sess-1"));
    }

    #[test]
    fn test_save_with_metadata_unknown_runtime_saves_entry_without_metadata_row() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        let mut entry = make_entry(
            "meta-unknown",
            "unknown runtime",
            1_700_000_333,
            EntryKind::Auto,
        );
        let raw_json = json!({
            "foo": "bar"
        });

        storage
            .save_with_metadata(&mut entry, &raw_json, None)
            .unwrap();

        let stored = storage.get_all().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].runtime, None);

        let conn = storage.pool().get().unwrap();
        let metadata_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entry_metadata_raw WHERE entry_id = ?1",
                [&entry.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(metadata_count, 0);
    }

    #[test]
    fn test_max_compaction_count_none_for_unknown_session() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        storage
            .save(&make_entry("e1", "hello", 100, EntryKind::Manual))
            .unwrap();

        let max = storage.max_compaction_count("missing-session").unwrap();
        assert_eq!(max, None);
    }

    #[test]
    fn test_max_compaction_count_returns_session_max() {
        let (storage, _) = open_storage(Path::new(":memory:"), 100).unwrap();

        let entry1 = ContextEntry {
            id: "s1-0".into(),
            content: "first".into(),
            timestamp: 100,
            kind: EntryKind::Auto,
            token_count: Some(1),
            session_id: Some("sess-1".into()),
            compaction_count: Some(0),
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        };
        let entry2 = ContextEntry {
            id: "s1-2".into(),
            content: "second".into(),
            timestamp: 200,
            kind: EntryKind::Auto,
            token_count: Some(1),
            session_id: Some("sess-1".into()),
            compaction_count: Some(2),
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        };
        let other = ContextEntry {
            id: "s2-5".into(),
            content: "other".into(),
            timestamp: 300,
            kind: EntryKind::Auto,
            token_count: Some(1),
            session_id: Some("sess-2".into()),
            compaction_count: Some(5),
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        };

        storage.save(&entry1).unwrap();
        storage.save(&entry2).unwrap();
        storage.save(&other).unwrap();

        let max = storage.max_compaction_count("sess-1").unwrap();
        assert_eq!(max, Some(2));
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
            .save(&make_entry(
                "e2",
                "second entry",
                200,
                EntryKind::PreCompact,
            ))
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
