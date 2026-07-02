//! Integration tests for lexicon importance scoring through the full
//! `ContextForge::builder` → `ContextEngine::assemble` → real DB path.
//!
//! These complement the unit tests in `src/lexicon/config.rs` (pattern matching,
//! negation window) and `src/engine.rs` (mock-searcher scorer integration) by
//! exercising the complete wire from builder construction through turso + tantivy
//! storage to final ranked output.

use context_forge::{
    kind, ConfigLexiconScorer, ContextForge, DefaultEnglishScorer, SaveOptions, MATCH_ALL_QUERY,
};
use std::path::PathBuf;

fn in_memory_config() -> context_forge::Config {
    let mut cfg = context_forge::Config::default();
    cfg.db_path = PathBuf::from(":memory:");
    cfg
}

// ── builder wiring ────────────────────────────────────────────────────────────

#[tokio::test]
async fn builder_without_persona_scorer_still_applies_english_defaults() {
    let cf = ContextForge::builder(in_memory_config())
        .build()
        .await
        .unwrap();

    let opts = SaveOptions::default();
    let neutral_id = cf
        .save("nothing important here", kind::SNAPSHOT, &opts)
        .await
        .unwrap();
    let affirm_id = cf
        .save("confirmed, that is correct", kind::SNAPSHOT, &opts)
        .await
        .unwrap();

    let hits = cf.query(MATCH_ALL_QUERY, None, 10_000).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].id, affirm_id,
        "English affirmation entry should rank first even without a persona scorer"
    );
    let _ = neutral_id;
}

#[tokio::test]
async fn builder_with_persona_scorer_stacks_on_english_baseline() {
    let persona_toml = r#"
[terms]
"Omnissiah" = 0.9

[affirmations]
patterns = ["for the emperor"]

[negations]
patterns = ["the emperor frowns upon this"]
"#;
    let persona: ConfigLexiconScorer = persona_toml.parse().unwrap();

    let cf = ContextForge::builder(in_memory_config())
        .with_persona_scorer(persona)
        .build()
        .await
        .unwrap();

    let opts = SaveOptions::default();
    let neutral_id = cf
        .save("routine status update", kind::SNAPSHOT, &opts)
        .await
        .unwrap();
    // English signal only (+0.5 from "confirmed").
    let english_id = cf
        .save("confirmed, the plan is set", kind::SNAPSHOT, &opts)
        .await
        .unwrap();
    // Persona signal: "for the emperor" (+0.5) + "Omnissiah" (+0.9) = boost 1.4.
    let persona_id = cf
        .save(
            "for the emperor — the Omnissiah guides our path",
            kind::SNAPSHOT,
            &opts,
        )
        .await
        .unwrap();

    let hits = cf.query(MATCH_ALL_QUERY, None, 10_000).await.unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].id, persona_id, "persona-boosted entry should rank first");
    assert_eq!(hits[1].id, english_id, "English-boosted entry should rank second");
    assert_eq!(hits[2].id, neutral_id, "neutral entry should rank last");
}

// ── DefaultEnglishScorer via builder ─────────────────────────────────────────

#[tokio::test]
async fn english_scorer_boosts_commissive_over_neutral() {
    let cf = ContextForge::builder(in_memory_config())
        .build()
        .await
        .unwrap();

    let opts = SaveOptions::default();
    let neutral_id = cf
        .save("the meeting was rescheduled", kind::SNAPSHOT, &opts)
        .await
        .unwrap();
    // "i'll fix it" fires a commissive affirmation in the English defaults.
    let commit_id = cf
        .save("i'll fix it before the next release", kind::SNAPSHOT, &opts)
        .await
        .unwrap();

    let hits = cf.query(MATCH_ALL_QUERY, None, 10_000).await.unwrap();
    assert_eq!(hits[0].id, commit_id, "commissive entry should outrank neutral");
    let _ = neutral_id;
}

#[tokio::test]
async fn negation_window_suppresses_false_affirmation() {
    // "not confirmed" should not fire the "confirmed" affirmation.
    let cf = ContextForge::builder(in_memory_config())
        .build()
        .await
        .unwrap();

    let opts = SaveOptions::default();
    // This should score zero (negation suppresses "confirmed").
    let negated_id = cf
        .save("that is not confirmed", kind::SNAPSHOT, &opts)
        .await
        .unwrap();
    // This should score positive.
    let affirm_id = cf
        .save("yes, confirmed", kind::SNAPSHOT, &opts)
        .await
        .unwrap();

    let hits = cf.query(MATCH_ALL_QUERY, None, 10_000).await.unwrap();
    assert_eq!(
        hits[0].id, affirm_id,
        "un-negated affirmation should outrank negated one"
    );
    let _ = negated_id;
}

// ── ConfigLexiconScorer validation ───────────────────────────────────────────

#[test]
fn invalid_weight_above_max_is_rejected_at_parse_time() {
    let bad_toml = r#"[terms]
"Heresy" = 2.0"#;
    let err = bad_toml.parse::<ConfigLexiconScorer>().unwrap_err();
    assert!(
        err.to_string().contains("Heresy"),
        "error should name the offending term"
    );
}

#[test]
fn weight_at_boundary_is_accepted() {
    let toml = r#"[terms]
"Emperor" = 1.5"#;
    assert!(toml.parse::<ConfigLexiconScorer>().is_ok());
}

// ── builder default scorer (no persona) is DefaultEnglishScorer ──────────────

#[test]
fn default_english_scorer_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DefaultEnglishScorer>();
}
