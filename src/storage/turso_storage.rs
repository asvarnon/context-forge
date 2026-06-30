use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::entry::ContextEntry;
use crate::storage::schema::SCHEMA_V2;
use crate::traits::ContextStorage;

// V1 schema without fts5 virtual table or triggers.
// The turso FTS index is added by the turso_migrate epilogue.
const TURSO_SCHEMA_V1: &str = r"
CREATE TABLE IF NOT EXISTS entries (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK(kind IN ('Manual','PreCompact','Auto')),
    token_count INTEGER CHECK(token_count >= 0),
    created_at  INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);
";

// V3 rebuild without fts5 virtual table or triggers.
// Identical logic to schema.rs SCHEMA_V3 except:
//   - DROP TRIGGER IF EXISTS uses IF EXISTS (safe for turso-native DBs with no triggers)
//   - DROP TABLE IF EXISTS entries_fts (safe when fts5 table was never created)
//   - No CREATE VIRTUAL TABLE / INSERT INTO entries_fts / CREATE TRIGGER
const TURSO_SCHEMA_V3: &str = r"
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
DROP TABLE IF EXISTS entries_fts;
DROP TABLE entries;
ALTER TABLE entries_v3 RENAME TO entries;

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);
CREATE INDEX IF NOT EXISTS idx_entries_scope ON entries(scope);
CREATE INDEX IF NOT EXISTS idx_entries_session_id ON entries(session_id);

DROP TABLE IF EXISTS runtime_field_mappings;
DROP TABLE IF EXISTS runtime_configs;
DROP TABLE IF EXISTS entry_metadata_raw;

INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 3);
COMMIT;
";

/// Turso-backed implementation of [`crate::traits::ContextStorage`].
pub struct TursoStorage {
    pub(crate) db: Arc<turso::Database>,
    max_entries: usize,
}

impl TursoStorage {
    /// Open (or create) a turso database at `db_path` and run migrations.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or migrations fail.
    pub async fn open(db_path: &Path, max_entries: usize) -> crate::Result<Self> {
        let path = db_path
            .to_str()
            .ok_or_else(|| crate::Error::Migration("non-UTF-8 database path".into()))?;
        let db = turso::Builder::new_local(path)
            .experimental_index_method(true)
            .build()
            .await?;
        let conn = db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        turso_migrate(&conn).await?;
        Ok(Self {
            db: Arc::new(db),
            max_entries,
        })
    }
}

pub(crate) async fn turso_migrate(conn: &turso::Connection) -> crate::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version \
         (id INTEGER PRIMARY KEY CHECK(id = 1), version INTEGER NOT NULL)",
        (),
    )
    .await?;

    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            (),
        )
        .await?;

    let version: i64 = match rows.next().await? {
        Some(row) => match row.get_value(0)? {
            turso::Value::Integer(v) => v,
            _ => 0,
        },
        None => 0,
    };

    if version < 1 {
        conn.execute_batch(TURSO_SCHEMA_V1).await?;
        conn.execute(
            "INSERT OR REPLACE INTO schema_version (id, version) VALUES (1, 1)",
            (),
        )
        .await?;
    }

    if version < 2 {
        conn.execute_batch(SCHEMA_V2).await?;
    }

    if version < 3 {
        conn.execute_batch(TURSO_SCHEMA_V3).await?;
    }

    // Remove fts5 virtual table and triggers that rusqlite migrations may have left behind.
    // These conflict with turso's native FTS MATCH operator.
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS entries_ai; \
         DROP TRIGGER IF EXISTS entries_ad; \
         DROP TRIGGER IF EXISTS entries_au; \
         DROP TABLE IF EXISTS entries_fts;",
    )
    .await?;

    // Ensure the turso FTS index exists, regardless of migration path.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_turso_fts ON entries USING fts (content)",
        (),
    )
    .await?;

    Ok(())
}

pub(crate) fn turso_row_to_entry(row: &turso::Row) -> crate::Result<ContextEntry> {
    let id = get_text(row, 0, "id")?;
    let content = get_text(row, 1, "content")?;
    let timestamp = get_int(row, 2, "timestamp")?;
    let kind = get_text(row, 3, "kind")?;
    let scope = get_text_opt(row, 4, "scope")?;
    let session_id = get_text_opt(row, 5, "session_id")?;
    let token_count_raw = get_int_opt(row, 6, "token_count")?;
    let metadata_str = get_text_opt(row, 7, "metadata")?;

    let token_count = token_count_raw
        .map(usize::try_from)
        .transpose()
        .map_err(|e| crate::Error::Migration(e.to_string()))?;

    let metadata = metadata_str
        .map(|s| serde_json::from_str::<serde_json::Value>(&s))
        .transpose()
        .map_err(|e| crate::Error::Migration(e.to_string()))?
        // json_patch('{}', json_object(nulls)) produces "{}"; treat empty object as None.
        .and_then(|v| {
            if v.as_object().is_some_and(|m| m.is_empty()) {
                None
            } else {
                Some(v)
            }
        });

    Ok(ContextEntry {
        id,
        content,
        timestamp,
        kind,
        scope,
        session_id,
        token_count,
        metadata,
    })
}

fn get_text(row: &turso::Row, idx: usize, field: &str) -> crate::Result<String> {
    match row.get_value(idx)? {
        turso::Value::Text(s) => Ok(s),
        other => Err(crate::Error::Migration(format!(
            "{field}: expected text, got {other:?}"
        ))),
    }
}

fn get_text_opt(row: &turso::Row, idx: usize, field: &str) -> crate::Result<Option<String>> {
    match row.get_value(idx)? {
        turso::Value::Text(s) => Ok(Some(s)),
        turso::Value::Null => Ok(None),
        other => Err(crate::Error::Migration(format!(
            "{field}: expected text or null, got {other:?}"
        ))),
    }
}

fn get_int(row: &turso::Row, idx: usize, field: &str) -> crate::Result<i64> {
    match row.get_value(idx)? {
        turso::Value::Integer(i) => Ok(i),
        other => Err(crate::Error::Migration(format!(
            "{field}: expected integer, got {other:?}"
        ))),
    }
}

fn get_int_opt(row: &turso::Row, idx: usize, field: &str) -> crate::Result<Option<i64>> {
    match row.get_value(idx)? {
        turso::Value::Integer(i) => Ok(Some(i)),
        turso::Value::Null => Ok(None),
        other => Err(crate::Error::Migration(format!(
            "{field}: expected integer or null, got {other:?}"
        ))),
    }
}

#[async_trait]
impl ContextStorage for TursoStorage {
    async fn save(&self, entry: &ContextEntry) -> crate::Result<()> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let max_entries = self.max_entries;

        let metadata_json = entry
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| crate::Error::InvalidEntry(format!("metadata is not valid JSON: {e}")))?;

        let result = async {
            conn.execute("BEGIN IMMEDIATE", ()).await?;

            let mut rows = conn
                .query(
                    "SELECT EXISTS(SELECT 1 FROM entries WHERE id = ?1)",
                    (entry.id.clone(),),
                )
                .await?;
            let exists: bool = match rows.next().await? {
                Some(row) => matches!(row.get_value(0)?, turso::Value::Integer(1)),
                None => false,
            };

            if !exists {
                let mut count_rows =
                    conn.query("SELECT COUNT(*) FROM entries", ()).await?;
                let count: i64 = match count_rows.next().await? {
                    Some(row) => match row.get_value(0)? {
                        turso::Value::Integer(n) => n,
                        _ => 0,
                    },
                    None => 0,
                };

                if count >= max_entries as i64 {
                    conn.execute(
                        "DELETE FROM entries WHERE id = \
                         (SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1)",
                        (),
                    )
                    .await?;
                }
            }

            conn.execute(
                "INSERT OR REPLACE INTO entries \
                 (id, content, timestamp, kind, scope, session_id, token_count, metadata) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                (
                    entry.id.clone(),
                    entry.content.clone(),
                    entry.timestamp,
                    entry.kind.clone(),
                    entry.scope.clone(),
                    entry.session_id.clone(),
                    entry.token_count.map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
                    metadata_json,
                ),
            )
            .await?;

            conn.execute("COMMIT", ()).await?;
            Ok(())
        }
        .await;

        if result.is_err() {
            let _ = conn.execute("ROLLBACK", ()).await;
        }

        result
    }

    async fn get_top_k(&self, k: usize) -> crate::Result<Vec<ContextEntry>> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
                 FROM entries ORDER BY timestamp DESC LIMIT ?1",
                (i64::try_from(k).unwrap_or(i64::MAX),),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(turso_row_to_entry(&row)?);
        }
        Ok(result)
    }

    async fn get_all(&self) -> crate::Result<Vec<ContextEntry>> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
                 FROM entries ORDER BY timestamp DESC",
                (),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(turso_row_to_entry(&row)?);
        }
        Ok(result)
    }

    async fn delete(&self, id: &str) -> crate::Result<bool> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let affected = conn
            .execute("DELETE FROM entries WHERE id = ?1", (id.to_owned(),))
            .await?;

        Ok(affected > 0)
    }

    async fn clear(&self) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let affected = conn.execute("DELETE FROM entries", ()).await?;
        Ok(affected as usize)
    }

    async fn clear_scope(&self, scope: &str) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let affected = conn
            .execute(
                "DELETE FROM entries WHERE scope = ?1",
                (scope.to_owned(),),
            )
            .await?;

        Ok(affected as usize)
    }

    async fn count(&self) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn.query("SELECT COUNT(*) FROM entries", ()).await?;
        match rows.next().await? {
            Some(row) => match row.get_value(0)? {
                turso::Value::Integer(n) => Ok(n as usize),
                _ => Ok(0),
            },
            None => Ok(0),
        }
    }
}
