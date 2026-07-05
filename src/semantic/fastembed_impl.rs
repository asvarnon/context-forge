//! fastembed-backed [`super::Embedder`] using all-MiniLM-L6-v2.

use std::path::Path;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Embedding model wrapping fastembed's `TextEmbedding`.
///
/// Uses `all-MiniLM-L6-v2` (384-dim, ~22 MB ONNX weights). The model is
/// downloaded automatically on first use to `cache_dir`; subsequent startups
/// load from cache. Network access is required on first use (and on any
/// subsequent start where fastembed performs its model-presence check).
///
/// The inner `TextEmbedding` takes `&mut self` on inference calls, so it is
/// guarded by a `Mutex` to satisfy the `Send + Sync` bound on `Embedder`.
/// CPU inference is blocking; callers must use [`tokio::task::spawn_blocking`].
pub struct FasEmbedder {
    model: Mutex<TextEmbedding>,
}

impl FasEmbedder {
    /// Load the model, downloading weights to `cache_dir` if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the model cannot be initialised.
    pub fn new(cache_dir: impl AsRef<Path>) -> crate::Result<Self> {
        let opts = InitOptions::new(EmbeddingModel::AllMiniLML6V2)
            .with_cache_dir(cache_dir.as_ref().to_path_buf())
            .with_show_download_progress(true);
        let model = TextEmbedding::try_new(opts)
            .map_err(|e| crate::Error::Migration(format!("fastembed init: {e}")))?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }
}

impl super::Embedder for FasEmbedder {
    fn embed(&self, text: &str) -> crate::Result<Vec<f32>> {
        let mut guard = self
            .model
            .lock()
            .map_err(|_| crate::Error::Migration("fastembed model mutex poisoned".into()))?;
        let mut results = guard
            .embed(vec![text], None)
            .map_err(|e| crate::Error::Migration(format!("embed: {e}")))?;
        results
            .pop()
            .ok_or_else(|| crate::Error::Migration("embed returned empty result".into()))
    }

    fn embed_batch(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
        let mut guard = self
            .model
            .lock()
            .map_err(|_| crate::Error::Migration("fastembed model mutex poisoned".into()))?;
        guard
            .embed(texts.to_vec(), Some(32))
            .map_err(|e| crate::Error::Migration(format!("embed_batch: {e}")))
    }
}
