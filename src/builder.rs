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
/// **Lexicon scoring is opt-in.** By default the engine ranks on relevance
/// (BM25, plus semantic when an embedding model is set) with no lexicon layer.
/// Lexicon scoring applies a *query-independent* importance boost, which suits
/// persona/importance use cases but degrades pure relevance retrieval — so it
/// must be requested explicitly:
///
/// - [`with_default_english_scorer`](Self::with_default_english_scorer) enables
///   the built-in [`DefaultEnglishScorer`] (plain-English commitment/
///   confirmation/decision markers).
/// - [`with_persona_scorer`](Self::with_persona_scorer) adds a domain scorer.
///
/// When both are set they compose additively via [`CompositeLexiconScorer`].
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
///     // Relevance only, no lexicon (default):
///     let cf = ContextForge::builder(config.clone()).build().await?;
///
///     // Opt into the English importance scorer:
///     let cf = ContextForge::builder(config.clone())
///         .with_default_english_scorer()
///         .build()
///         .await?;
///
///     // English + a persona scorer loaded from a TOML string:
///     let toml = "[affirmations]\npatterns = [\"for the emperor\"]";
///     let persona: ConfigLexiconScorer = toml.parse()?;
///     let cf = ContextForge::builder(config)
///         .with_default_english_scorer()
///         .with_persona_scorer(persona)
///         .build()
///         .await?;
///
///     Ok(())
/// }
/// ```
pub struct ContextForgeBuilder {
    config: Config,
    english_defaults: bool,
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
            english_defaults: false,
            persona_scorer: None,
            #[cfg(feature = "semantic")]
            embedding_cache_dir: None,
        }
    }

    /// Enable the built-in [`DefaultEnglishScorer`] (opt-in).
    ///
    /// Applies a query-independent importance boost to entries containing
    /// plain-English commitment/confirmation/decision/correction markers
    /// ("i'll fix it", "confirmed", "we decided", "never mind"). This is the
    /// right signal for persona/importance use cases (surfacing what matters to
    /// a user) but **hurts pure relevance retrieval** — a factual-QA benchmark
    /// showed it lowering recall by burying evidence under important-*sounding*
    /// distractors. Off by default for that reason; enable it when importance,
    /// not just relevance, is what you want to rank on.
    #[must_use]
    pub fn with_default_english_scorer(mut self) -> Self {
        self.english_defaults = true;
        self
    }

    /// Add a persona scorer (opt-in).
    ///
    /// Typically a [`crate::ConfigLexiconScorer`] loaded from a domain-specific
    /// TOML file. If [`with_default_english_scorer`](Self::with_default_english_scorer)
    /// was also called, the two compose additively via [`CompositeLexiconScorer`]
    /// and the engine applies a `-1.0` floor after fusion; otherwise the persona
    /// scorer is used alone.
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

    /// Open the database and build a [`ContextForge`] with the configured scorers.
    ///
    /// Lexicon scoring is applied only if
    /// [`with_default_english_scorer`](Self::with_default_english_scorer) and/or
    /// [`with_persona_scorer`](Self::with_persona_scorer) were called; with both,
    /// they compose via [`CompositeLexiconScorer`]. With neither, the engine ranks
    /// on relevance (BM25 + semantic) only.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or migrations fail.
    pub async fn build(self) -> Result<ContextForge> {
        let db_path = self.config.db_path.clone();
        let max_entries = self.config.max_entries;
        let scrub_config: ScrubConfig = self.config.scrub.clone();

        let (storage, searcher) = open_storage(Path::new(&db_path), max_entries).await?;

        // Compose only the opted-in scorers; default is none (relevance only).
        let mut scorers: Vec<Arc<dyn LexiconScorer>> = Vec::new();
        if self.english_defaults {
            scorers.push(Arc::new(DefaultEnglishScorer::default()));
        }
        if let Some(persona) = self.persona_scorer {
            scorers.push(persona);
        }
        let scorer: Option<Arc<dyn LexiconScorer>> = match scorers.len() {
            0 => None,
            1 => scorers.pop(),
            _ => Some(Arc::new(CompositeLexiconScorer::new(scorers))),
        };

        #[cfg_attr(not(feature = "semantic"), allow(unused_mut))]
        let mut engine = ContextEngine::new(Box::new(storage), Box::new(searcher), self.config);
        if let Some(scorer) = scorer {
            engine = engine.with_scorer(scorer);
        }

        #[cfg(feature = "semantic")]
        if let Some(cache_dir) = self.embedding_cache_dir {
            let embedder = crate::semantic::FasEmbedder::new(&cache_dir)?;
            engine = engine.with_embedder(Arc::new(embedder));
        }

        Ok(ContextForge::from_parts(engine, scrub_config))
    }
}
