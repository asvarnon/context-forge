//! [`ContextForgeBuilder`] — opinionated construction path for [`crate::ContextForge`].

use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::engine::ContextEngine;
use crate::lexicon::{CompositeLexiconScorer, DefaultEnglishScorer, LexiconScorer};
use crate::scrub::ScrubConfig;
use crate::storage::open_storage;
use crate::traits::Result;
use crate::ContextForge;

/// Builder for [`ContextForge`].
///
/// The builder always pre-seeds [`DefaultEnglishScorer`] so plain-English
/// importance signals ("confirmed", "we decided", "never mind", etc.) are
/// active without any additional configuration. A caller-provided persona
/// scorer is optional and stacks additively on top via
/// [`CompositeLexiconScorer`].
///
/// # Example
///
/// ```no_run
/// use context_forge::{Config, ContextForge, ConfigLexiconScorer};
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> Result<(), context_forge::Error> {
///     let mut config = Config::default();
///     config.db_path = PathBuf::from("memory.db");
///
///     // English scorer only (most common case):
///     let cf = ContextForge::builder(config.clone()).build().await?;
///
///     // English + persona scorer loaded from a TOML string:
///     let toml = "[affirmations]\npatterns = [\"for the emperor\"]";
///     let persona: ConfigLexiconScorer = toml.parse()?;
///     let cf = ContextForge::builder(config).with_persona_scorer(persona).build().await?;
///
///     Ok(())
/// }
/// ```
pub struct ContextForgeBuilder {
    config: Config,
    persona_scorer: Option<Arc<dyn LexiconScorer>>,
    #[cfg(feature = "semantic")]
    embedding_cache_dir: Option<std::path::PathBuf>,
}

impl ContextForgeBuilder {
    /// Create a new builder with the given config.
    ///
    /// Prefer [`ContextForge::builder`] over calling this directly.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            persona_scorer: None,
            #[cfg(feature = "semantic")]
            embedding_cache_dir: None,
        }
    }

    /// Stack a persona scorer on top of the always-on [`DefaultEnglishScorer`].
    ///
    /// The persona scorer is typically a [`crate::ConfigLexiconScorer`] loaded
    /// from a domain-specific TOML file. Its boosts are summed with the English
    /// layer; the engine applies a `-1.0` floor after fusion.
    #[must_use]
    pub fn with_persona_scorer(mut self, scorer: impl LexiconScorer + 'static) -> Self {
        self.persona_scorer = Some(Arc::new(scorer));
        self
    }

    /// Enable semantic search using the all-MiniLM-L6-v2 model.
    ///
    /// `cache_dir` is where fastembed stores the downloaded ONNX weights
    /// (~22 MB). The model is downloaded automatically on first use; subsequent
    /// starts load from the local cache.
    ///
    /// Requires the `semantic` Cargo feature.
    #[cfg(feature = "semantic")]
    #[must_use]
    pub fn with_embedding_model(mut self, cache_dir: impl AsRef<std::path::Path>) -> Self {
        self.embedding_cache_dir = Some(cache_dir.as_ref().to_path_buf());
        self
    }

    /// Open the database and build a [`ContextForge`] with the configured scorer.
    ///
    /// The [`DefaultEnglishScorer`] is always included. If
    /// [`Self::with_persona_scorer`] was called, both scorers are composed via
    /// [`CompositeLexiconScorer`].
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or migrations fail.
    pub async fn build(self) -> Result<ContextForge> {
        let db_path = self.config.db_path.clone();
        let max_entries = self.config.max_entries;
        let scrub_config: ScrubConfig = self.config.scrub.clone();

        let (storage, searcher) = open_storage(Path::new(&db_path), max_entries).await?;

        let english: Arc<dyn LexiconScorer> = Arc::new(DefaultEnglishScorer::default());
        let scorer: Arc<dyn LexiconScorer> = match self.persona_scorer {
            Some(persona) => Arc::new(CompositeLexiconScorer::new(vec![english, persona])),
            None => english,
        };

        #[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
        let mut engine = ContextEngine::new(Box::new(storage), Box::new(searcher), self.config)
            .with_scorer(scorer);

        #[cfg(feature = "semantic")]
        if let Some(cache_dir) = self.embedding_cache_dir {
            let embedder = crate::semantic::FasEmbedder::new(&cache_dir)?;
            engine = engine.with_embedder(Arc::new(embedder));
        }

        Ok(ContextForge::from_parts(engine, scrub_config))
    }
}
