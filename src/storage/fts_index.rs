use std::sync::{Arc, Mutex};

use tantivy::schema::{Field, Schema, STORED, STRING, TEXT};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

const WRITER_HEAP_BYTES: usize = 50_000_000; // 50 MB

/// Shared in-memory tantivy index for BM25 search over entry content.
///
/// Rebuilt from turso at startup. turso is the source of truth for persistence;
/// this index is a derived, read-acceleration layer.
pub(crate) struct FtsIndex {
    writer: Mutex<IndexWriter>,
    pub(crate) reader: IndexReader,
    pub(crate) id_field: Field,
    pub(crate) content_field: Field,
    pub(crate) scope_field: Field,
}

impl FtsIndex {
    /// Build a new empty in-memory index. Caller feeds documents in before
    /// the first search by calling `add` for each existing entry and then `commit`.
    pub(crate) fn new() -> crate::Result<Arc<Self>> {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STORED);
        let content_field = schema_builder.add_text_field("content", TEXT);
        // STRING = raw/keyword tokenizer: indexes the whole value as one term so
        // "discord:guild:123456789" is one token, not three. Required for exact-match
        // TermQuery filtering by scope.
        let scope_field = schema_builder.add_text_field("scope", STRING);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema);
        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;

        Ok(Arc::new(Self {
            writer: Mutex::new(writer),
            reader,
            id_field,
            content_field,
            scope_field,
        }))
    }

    /// Add a document. Does not commit — call `commit` when the batch is done.
    pub(crate) fn add(&self, id: &str, content: &str, scope: Option<&str>) -> crate::Result<()> {
        let writer = self
            .writer
            .lock()
            .map_err(|e| crate::Error::Migration(format!("fts writer lock poisoned: {e}")))?;

        // Delete any existing doc with this id so INSERT OR REPLACE is idempotent.
        let id_term = Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);

        let mut doc = TantivyDocument::default();
        doc.add_text(self.id_field, id);
        doc.add_text(self.content_field, content);
        if let Some(s) = scope {
            doc.add_text(self.scope_field, s);
        }
        writer
            .add_document(doc)
            .map_err(|e| crate::Error::Migration(e.to_string()))?;

        Ok(())
    }

    /// Remove a single document by id. Commits immediately.
    pub(crate) fn remove(&self, id: &str) -> crate::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| crate::Error::Migration(format!("fts writer lock poisoned: {e}")))?;
        let id_term = Term::from_field_text(self.id_field, id);
        writer.delete_term(id_term);
        writer
            .commit()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        Ok(())
    }

    /// Remove all documents. Commits immediately.
    pub(crate) fn clear(&self) -> crate::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| crate::Error::Migration(format!("fts writer lock poisoned: {e}")))?;
        writer
            .delete_all_documents()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        writer
            .commit()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        Ok(())
    }

    /// Commit all pending adds/deletes and refresh the reader so searches see them.
    pub(crate) fn commit(&self) -> crate::Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|e| crate::Error::Migration(format!("fts writer lock poisoned: {e}")))?;
        writer
            .commit()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| crate::Error::Migration(e.to_string()))?;
        Ok(())
    }
}
