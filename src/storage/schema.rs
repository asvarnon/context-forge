use rusqlite::Connection;

use crate::entry::ContextEntry;
use crate::Result;

const CREATE_SCHEMA_VERSION: &str =
    "CREATE TABLE IF NOT EXISTS schema_version (id INTEGER PRIMARY KEY CHECK(id = 1), version INTEGER NOT NULL)";

/// Map a `rusqlite::Row` to a `ContextEntry`.
///
/// The row must contain all columns by name: id, content, timestamp, kind,
/// `scope`, `session_id`, `token_count`, metadata.
pub(crate) fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<ContextEntry> {
    let token_count: Option<i64> = row.get("token_count")?;
    let metadata_str: Option<String> = row.get("metadata")?;
    let metadata = metadata_str
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;

    let token_count = token_count.map(usize::try_from).transpose().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(e))
    })?;

    Ok(ContextEntry {
        id: row.get("id")?,
        content: row.get("content")?,
        timestamp: row.get("timestamp")?,
        kind: row.get("kind")?,
        scope: row.get("scope")?,
        session_id: row.get("session_id")?,
        token_count,
        metadata,
    })
}

pub(crate) const SCHEMA_V1: &str = r"
CREATE TABLE IF NOT EXISTS entries (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK(kind IN ('Manual','PreCompact','Auto')),
    token_count INTEGER CHECK(token_count >= 0),
    created_at  INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);

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
";

pub(crate) const SCHEMA_V2: &str = r"
BEGIN IMMEDIATE;

ALTER TABLE entries ADD COLUMN session_id TEXT;
ALTER TABLE entries ADD COLUMN compaction_count INTEGER;
ALTER TABLE entries ADD COLUMN compaction_trigger TEXT;
ALTER TABLE entries ADD COLUMN runtime TEXT;
ALTER TABLE entries ADD COLUMN model TEXT;
ALTER TABLE entries ADD COLUMN cwd TEXT;
ALTER TABLE entries ADD COLUMN git_branch TEXT;
ALTER TABLE entries ADD COLUMN git_sha TEXT;
ALTER TABLE entries ADD COLUMN turn_id TEXT;
ALTER TABLE entries ADD COLUMN agent_type TEXT;
ALTER TABLE entries ADD COLUMN agent_id TEXT;
ALTER TABLE entries ADD COLUMN embedding BLOB;

CREATE INDEX IF NOT EXISTS idx_entries_session_id ON entries(session_id);
CREATE INDEX IF NOT EXISTS idx_entries_runtime ON entries(runtime);
CREATE INDEX IF NOT EXISTS idx_entries_git_branch ON entries(git_branch);
CREATE INDEX IF NOT EXISTS idx_embedding_present ON entries(id) WHERE embedding IS NOT NULL;

CREATE TABLE IF NOT EXISTS runtime_configs (
    runtime      TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    hook_format  TEXT NOT NULL,
    active       INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE IF NOT EXISTS runtime_field_mappings (
    runtime         TEXT NOT NULL REFERENCES runtime_configs(runtime),
    canonical_field TEXT NOT NULL,
    source_path     TEXT NOT NULL,
    transform       TEXT,
    PRIMARY KEY (runtime, canonical_field)
) STRICT;

CREATE TABLE IF NOT EXISTS entry_metadata_raw (
    entry_id TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    runtime  TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    PRIMARY KEY (entry_id)
) STRICT;

CREATE TABLE IF NOT EXISTS tags (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
) STRICT;

CREATE TABLE IF NOT EXISTS entry_tags (
    entry_id TEXT NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag_id   TEXT NOT NULL REFERENCES tags(id),
    PRIMARY KEY (entry_id, tag_id)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_entry_tags_tag_id ON entry_tags(tag_id);

INSERT OR IGNORE INTO runtime_configs (runtime, display_name, hook_format, active) VALUES
    ('claude-code', 'Claude Code', 'json_stdin', 1),
    ('codex', 'Codex CLI', 'json_stdin', 1),
    ('gemini', 'Gemini CLI', 'json_stdin', 1),
    ('cline', 'Cline', 'json_stdin', 1),
    ('openclaw', 'OpenClaw', 'json_stdin', 1);

INSERT OR IGNORE INTO runtime_field_mappings (runtime, canonical_field, source_path, transform) VALUES
    ('claude-code', 'session_id', 'session_id', NULL),
    ('claude-code', 'model', 'model', NULL),
    ('claude-code', 'cwd', 'cwd', NULL),
    ('claude-code', 'compaction_trigger', 'matcher_value', NULL),
    ('claude-code', 'agent_type', 'agent_type', NULL),
    ('claude-code', 'agent_id', 'agent_id', NULL),
    ('codex', 'session_id', 'threadId', NULL),
    ('codex', 'turn_id', 'turnId', NULL),
    ('codex', 'git_branch', 'git.branch', NULL),
    ('codex', 'git_sha', 'git.sha', NULL),
    ('gemini', 'session_id', 'session_id', NULL),
    ('gemini', 'cwd', 'cwd', NULL),
    ('cline', 'session_id', 'sessionId', NULL),
    ('openclaw', 'session_id', 'sessionKey', NULL);

INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 2);
COMMIT;
";

pub(crate) const SCHEMA_V3: &str = r"
BEGIN IMMEDIATE;

CREATE TABLE entries_v3 (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    scope       TEXT,
    session_id  TEXT,
    token_count INTEGER CHECK(token_count >= 0),
    metadata    TEXT,
    created_at  INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
) STRICT;

INSERT INTO entries_v3 (id, content, timestamp, kind, scope, session_id, token_count, metadata, created_at)
SELECT id, content, timestamp,
       CASE kind WHEN 'Manual' THEN 'manual' WHEN 'PreCompact' THEN 'snapshot' WHEN 'Auto' THEN 'summary' ELSE lower(kind) END,
       NULL,
       session_id, token_count,
       json_patch('{}', json_object(
           'runtime', runtime, 'model', model, 'cwd', cwd,
           'git_branch', git_branch, 'git_sha', git_sha,
           'compaction_trigger', compaction_trigger,
           'turn_id', turn_id, 'agent_type', agent_type, 'agent_id', agent_id)),
       created_at
FROM entries;

DROP TRIGGER IF EXISTS entries_ai;
DROP TRIGGER IF EXISTS entries_ad;
DROP TRIGGER IF EXISTS entries_au;
DROP TABLE entries_fts;
DROP TABLE entries;
ALTER TABLE entries_v3 RENAME TO entries;

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);
CREATE INDEX IF NOT EXISTS idx_entries_scope ON entries(scope);
CREATE INDEX IF NOT EXISTS idx_entries_session_id ON entries(session_id);

CREATE VIRTUAL TABLE entries_fts USING fts5(content, content=entries, content_rowid=rowid);
INSERT INTO entries_fts(rowid, content) SELECT rowid, content FROM entries;

CREATE TRIGGER entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;

CREATE TRIGGER entries_au AFTER UPDATE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;

DROP TABLE IF EXISTS runtime_field_mappings;
DROP TABLE IF EXISTS runtime_configs;
DROP TABLE IF EXISTS entry_metadata_raw;

INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 3);
COMMIT;
";

fn me(e: rusqlite::Error) -> crate::Error { crate::Error::Migration(e.to_string()) }

/// Run database migrations up to the latest schema version.
///
/// This function is idempotent — calling it multiple times on the same
/// database has no additional effect once the schema is current.
///
/// # Errors
///
/// Returns an error if any migration statement fails to execute.
pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(CREATE_SCHEMA_VERSION).map_err(me)?;

    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .map_err(me)?;

    if version < 1 {
        conn.execute_batch(SCHEMA_V1).map_err(me)?;

        conn.execute(
            "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 1)",
            [],
        )
        .map_err(me)?;
    }

    if version < 2 {
        conn.execute_batch(SCHEMA_V2).map_err(me)?;
    }

    if version < 3 {
        conn.execute_batch(SCHEMA_V3).map_err(me)?;
    }

    Ok(())
}
