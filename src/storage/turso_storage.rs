use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::entry::ContextEntry;
use crate::storage::fts_index::FtsIndex;
use crate::traits::ContextStorage;

const TURSO_SCHEMA: &str = r"
CREATE TABLE IF NOT EXISTS entries (
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

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);
CREATE INDEX IF NOT EXISTS idx_entries_scope ON entries(scope);
CREATE INDEX IF NOT EXISTS idx_entries_session_id ON entries(session_id);
";

/// Turso-backed implementation of [`crate::traits::ContextStorage`].
pub struct TursoStorage {
    pub(crate) db: Arc<turso::Database>,
    pub(crate) fts: Arc<FtsIndex>,
    max_entries: usize,
}

impl TursoStorage {
    /// Open (or create) a turso database at `db_path` and run migrations.
    ///
    /// Rebuilds the in-memory tantivy FTS index from all existing entries so
    /// BM25 corpus statistics are accurate from the first query.
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

        // Rebuild tantivy index from all existing entries.
        let fts = FtsIndex::new()?;
        let mut rows = conn
            .query("SELECT id, content, scope FROM entries", ())
            .await?;
        while let Some(row) = rows.next().await? {
            let turso::Value::Text(id) = row.get_value(0)? else {
                continue;
            };
            let turso::Value::Text(content) = row.get_value(1)? else {
                continue;
            };
            let scope = match row.get_value(2)? {
                turso::Value::Text(s) => Some(s),
                _ => None,
            };
            fts.add(&id, &content, scope.as_deref())?;
        }
        fts.commit()?;

        Ok(Self {
            db: Arc::new(db),
            fts,
            max_entries,
        })
    }
}

pub(crate) async fn turso_migrate(conn: &turso::Connection) -> crate::Result<()> {
    conn.execute_batch(TURSO_SCHEMA).await?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_turso_fts ON entries USING fts (content)",
        (),
    )
    .await?;
    // Additive migration: add embedding column for vector search.
    // ALTER TABLE ... ADD COLUMN doesn't support IF NOT EXISTS in SQLite;
    // we intentionally discard the error if the column already exists.
    let _ = conn
        .execute("ALTER TABLE entries ADD COLUMN embedding BLOB", ())
        .await;
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
            if v.as_object().is_some_and(serde_json::Map::is_empty) {
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

impl TursoStorage {
    /// Persist one entry to turso and stage it in the tantivy index **without**
    /// committing the index. Callers commit afterward — [`save`](ContextStorage::save)
    /// once per entry, [`save_batch`](ContextStorage::save_batch) once per batch.
    async fn persist_one(&self, entry: &ContextEntry) -> crate::Result<()> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let max_entries = self.max_entries;

        let metadata_json = entry
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| crate::Error::InvalidEntry(format!("metadata is not valid JSON: {e}")))?;

        let mut evicted_id: Option<String> = None;

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
                let mut count_rows = conn.query("SELECT COUNT(*) FROM entries", ()).await?;
                let count: i64 = match count_rows.next().await? {
                    Some(row) => match row.get_value(0)? {
                        turso::Value::Integer(n) => n,
                        _ => 0,
                    },
                    None => 0,
                };

                if count >= i64::try_from(max_entries).unwrap_or(i64::MAX) {
                    let mut id_rows = conn
                        .query("SELECT id FROM entries ORDER BY timestamp ASC LIMIT 1", ())
                        .await?;
                    if let Some(row) = id_rows.next().await? {
                        if let turso::Value::Text(id) = row.get_value(0)? {
                            conn.execute("DELETE FROM entries WHERE id = ?1", (id.clone(),))
                                .await?;
                            evicted_id = Some(id);
                        }
                    }
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
                    entry
                        .token_count
                        .map(|n| i64::try_from(n).unwrap_or(i64::MAX)),
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
            return result;
        }

        // Turso write committed — stage in tantivy (no commit; the caller
        // commits). If this fails the entry is still persisted in turso; the
        // next startup rebuild will re-sync the index.
        if let Some(id) = evicted_id {
            self.fts.remove(&id)?;
        }
        self.fts
            .add(&entry.id, &entry.content, entry.scope.as_deref())?;

        Ok(())
    }
}

#[async_trait]
impl ContextStorage for TursoStorage {
    async fn save(&self, entry: &ContextEntry) -> crate::Result<()> {
        self.persist_one(entry).await?;
        self.fts.commit()?;
        Ok(())
    }

    async fn save_batch(&self, entries: &[ContextEntry]) -> crate::Result<()> {
        for entry in entries {
            self.persist_one(entry).await?;
        }
        // Single index commit for the whole batch — the point of this method.
        self.fts.commit()?;
        Ok(())
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

        if affected > 0 {
            self.fts.remove(id)?;
        }
        Ok(affected > 0)
    }

    async fn clear(&self) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let affected = conn.execute("DELETE FROM entries", ()).await?;
        self.fts.clear()?;
        Ok(usize::try_from(affected).unwrap_or(usize::MAX))
    }

    async fn clear_scope(&self, scope: &str) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        // Collect the ids being deleted so we can remove them from tantivy.
        let mut id_rows = conn
            .query(
                "SELECT id FROM entries WHERE scope = ?1",
                (scope.to_owned(),),
            )
            .await?;
        let mut ids: Vec<String> = Vec::new();
        while let Some(row) = id_rows.next().await? {
            if let turso::Value::Text(id) = row.get_value(0)? {
                ids.push(id);
            }
        }

        let affected = conn
            .execute("DELETE FROM entries WHERE scope = ?1", (scope.to_owned(),))
            .await?;

        for id in &ids {
            self.fts.remove(id)?;
        }
        Ok(usize::try_from(affected).unwrap_or(usize::MAX))
    }

    async fn count(&self) -> crate::Result<usize> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn.query("SELECT COUNT(*) FROM entries", ()).await?;
        match rows.next().await? {
            Some(row) => match row.get_value(0)? {
                turso::Value::Integer(n) => Ok(usize::try_from(n).unwrap_or(0)),
                _ => Ok(0),
            },
            None => Ok(0),
        }
    }

    async fn save_embedding(&self, id: &str, embedding: &[f32]) -> crate::Result<()> {
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute(
            "UPDATE entries SET embedding = vector32(?1) WHERE id = ?2",
            (vec_str, id.to_owned()),
        )
        .await?;
        tracing::trace!(id = %id, dims = embedding.len(), "embedding saved to turso");
        Ok(())
    }

    async fn get_unembedded(&self, limit: usize) -> crate::Result<Vec<crate::entry::ContextEntry>> {
        let conn = self.db.connect()?;
        conn.busy_timeout(Duration::from_secs(5))?;

        let mut rows = conn
            .query(
                "SELECT id, content, timestamp, kind, scope, session_id, token_count, metadata \
                 FROM entries WHERE embedding IS NULL LIMIT ?1",
                (i64::try_from(limit).unwrap_or(i64::MAX),),
            )
            .await?;

        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(turso_row_to_entry(&row)?);
        }
        tracing::trace!(count = %result.len(), "get_unembedded: returning entries");
        Ok(result)
    }
}
