//! Embedding abstraction for semantic search.
//!
//! The [`Embedder`] trait is always available. The [`FasEmbedder`] concrete
//! implementation (backed by fastembed + ONNX Runtime) is compiled only when
//! the `semantic` feature is enabled.

/// Sync embedding interface.
///
/// ONNX inference is CPU-bound and cannot yield to the async runtime.
/// Callers must wrap calls in [`tokio::task::spawn_blocking`].
pub trait Embedder: Send + Sync {
    /// Embed a single text into a dense float vector.
    ///
    /// # Errors
    ///
    /// Returns an error if the embedding model fails.
    fn embed(&self, text: &str) -> crate::Result<Vec<f32>>;

    /// Embed a batch of texts. Default implementation calls [`Self::embed`]
    /// once per text; concrete types may override for batched inference.
    ///
    /// # Errors
    ///
    /// Returns an error if any embedding fails.
    fn embed_batch(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
}

#[cfg(feature = "semantic")]
mod fastembed_impl;
#[cfg(feature = "semantic")]
pub use fastembed_impl::FasEmbedder;
