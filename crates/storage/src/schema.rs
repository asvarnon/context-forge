use cf_core::error::CoreError;
use cf_core::Result;
use rusqlite::Connection;

const CURRENT_VERSION: i64 = 1;

const CREATE_SCHEMA_VERSION: &str =
    "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)";

const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS entries (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    token_count INTEGER,
    created_at  INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    content,
    content=entries,
    content_rowid=rowid
);

CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS entries_au AFTER UPDATE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;
"#;

/// Run database migrations up to the latest schema version.
///
/// This function is idempotent — calling it multiple times on the same
/// database has no additional effect once the schema is current.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_SCHEMA_VERSION)
        .map_err(|e| CoreError::Storage(e.to_string()))?;

    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;

    if version < CURRENT_VERSION {
        conn.execute_batch(SCHEMA_V1)
            .map_err(|e| CoreError::Storage(e.to_string()))?;

        if version == 0 {
            conn.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                [CURRENT_VERSION],
            )
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        } else {
            conn.execute("UPDATE schema_version SET version = ?1", [CURRENT_VERSION])
                .map_err(|e| CoreError::Storage(e.to_string()))?;
        }
    }

    Ok(())
}
