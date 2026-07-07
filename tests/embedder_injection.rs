//! Integration test for the public `Embedder` injection path (Phase 1c).
//!
//! Verifies that a caller-defined [`Embedder`] can be injected via
//! `ContextForgeBuilder::with_embedder` — the API that lets a single loaded
//! model be shared across many builds, or an alternate backend be plugged in.
//! Semantic-gated: the injection API and semantic search require the feature.
#![cfg(feature = "semantic")]

use std::path::PathBuf;
use std::sync::Arc;

use context_forge::{kind, Config, ContextForge, Embedder, SaveOptions};

/// A trivial, deterministic embedder — proves an arbitrary `Embedder` impl (not
/// just the built-in `FasEmbedder`) is accepted and driven end to end.
struct ConstantEmbedder {
    dims: usize,
}

impl Embedder for ConstantEmbedder {
    fn embed(&self, text: &str) -> context_forge::Result<Vec<f32>> {
        // Deterministic vector from the text so distinct inputs differ slightly.
        let seed = (text.len() % 13) as f32 / 13.0;
        Ok(vec![seed; self.dims])
    }
}

fn in_memory_config() -> Config {
    let mut cfg = Config::default();
    cfg.db_path = PathBuf::from(":memory:");
    cfg
}

#[tokio::test]
async fn builder_accepts_injected_custom_embedder() {
    // all-MiniLM-L6-v2 is 384-dim; the schema's F32_BLOB is sized for that.
    let embedder: Arc<dyn Embedder> = Arc::new(ConstantEmbedder { dims: 384 });

    let cf = ContextForge::builder(in_memory_config())
        .with_embedder(embedder)
        .build()
        .await
        .expect("build with injected embedder");

    // Saving drives the injected embedder (embed_and_store) without error.
    let opts = SaveOptions::default();
    cf.save("the deploy used a canary rollout", kind::SNAPSHOT, &opts)
        .await
        .expect("save with injected embedder");
    cf.save("lunch options near the office", kind::SNAPSHOT, &opts)
        .await
        .expect("save second entry");

    // Query runs BM25 + semantic (via the injected embedder) and returns without
    // error — proving the injected embedder is wired through the whole path.
    let hits = cf
        .query("deploy rollout", None, 4096)
        .await
        .expect("query with injected embedder");
    assert!(
        hits.iter().any(|e| e.content.contains("canary")),
        "the relevant entry should be retrieved"
    );
}

#[tokio::test]
async fn injected_embedder_shared_across_builds() {
    // The core 1c use case: one Arc shared across multiple ContextForge builds
    // (load once, clone the Arc) rather than reconstructing per instance.
    let shared: Arc<dyn Embedder> = Arc::new(ConstantEmbedder { dims: 384 });

    for _ in 0..3 {
        let cf = ContextForge::builder(in_memory_config())
            .with_embedder(Arc::clone(&shared))
            .build()
            .await
            .expect("build sharing one embedder");
        cf.save(
            "shared embedder entry",
            kind::SNAPSHOT,
            &SaveOptions::default(),
        )
        .await
        .expect("save");
    }
}
