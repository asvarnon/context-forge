//! Lexicon importance scoring — end-to-end walkthrough.
//!
//! Demonstrates:
//!  - Generating a persona lexicon bootstrap prompt via `bootstrap_prompt`
//!  - Constructing a `ConfigLexiconScorer` from a TOML string
//!  - Wiring it into `ContextForge` via the builder
//!  - Observing that entries with importance signals rank above neutral ones
//!
//! Run with:
//!
//! ```bash
//! cargo run --example lexicon
//! ```

use context_forge::{
    bootstrap_prompt, kind, ConfigLexiconScorer, ContextForge, SaveOptions, MATCH_ALL_QUERY,
};

#[tokio::main]
async fn main() -> Result<(), context_forge::Error> {
    // ── 1. bootstrap_prompt ──────────────────────────────────────────────────
    //
    // In a real app, pass this to your LLM endpoint. The model returns a fenced
    // TOML block you extract, save to disk, and load on subsequent runs.
    // No LLM call happens here — we're just showing what the caller would send.

    let prompt = bootstrap_prompt("A Space Marine Chaplain from Warhammer 40k");
    println!("=== bootstrap prompt (first 300 chars) ===");
    println!("{}\n", &prompt[..300.min(prompt.len())]);

    // ── 2. Build a ConfigLexiconScorer from TOML ─────────────────────────────
    //
    // In production this comes from ConfigLexiconScorer::from_file("lexicon.toml").
    // Here we inline it to keep the example self-contained.

    let persona_toml = r#"
[terms]
"Omnissiah"  = 0.9   # critical proper noun — almost always in high-value content
"Astartes"   = 0.6   # strong domain noun
"bolter"     = 0.3   # mild domain term

[affirmations]
patterns = [
    "for the emperor",       # confirmation / commitment
    "it shall be done",      # future obligation
    "affirmative, brother",  # agreement
]

[negations]
patterns = [
    "negative, battle-brother",       # disagreement
    "the emperor frowns upon this",   # dismissal
]
"#;

    let persona: ConfigLexiconScorer = persona_toml.parse()?;

    // ── 3. Wire into ContextForge via the builder ─────────────────────────────
    //
    // The builder always pre-seeds DefaultEnglishScorer. with_persona_scorer
    // stacks the WH40k lexicon on top via CompositeLexiconScorer.

    let mut config = context_forge::Config::default();
    // :memory: so the example leaves no files behind.
    config.db_path = std::path::PathBuf::from(":memory:");

    let cf = ContextForge::builder(config)
        .with_persona_scorer(persona)
        .build()
        .await?;

    // ── 4. Save entries with different importance signal density ──────────────

    let opts = SaveOptions::default();

    // Neutral — no lexicon signals.
    let neutral_id = cf
        .save(
            "the routine maintenance check was completed",
            kind::SNAPSHOT,
            &opts,
        )
        .await?;

    // English signal — "confirmed" fires DefaultEnglishScorer (+0.5).
    let english_id = cf
        .save(
            "confirmed, the patrol route is clear",
            kind::SNAPSHOT,
            &opts,
        )
        .await?;

    // Persona signal — "for the emperor" fires the WH40k affirmation (+0.5)
    // and "Astartes" fires the term weight (+0.6). Total boost ≈ 1.1.
    let persona_id = cf
        .save(
            "for the emperor — the Astartes have secured the sector",
            kind::SNAPSHOT,
            &opts,
        )
        .await?;

    // ── 5. Query and inspect ranking ──────────────────────────────────────────

    let hits = cf.query(MATCH_ALL_QUERY, None, 10_000).await?;

    println!("=== query results (highest importance first) ===");
    for (i, hit) in hits.iter().enumerate() {
        let label = if hit.id == persona_id {
            "persona signal"
        } else if hit.id == english_id {
            "english signal"
        } else if hit.id == neutral_id {
            "neutral"
        } else {
            "unknown"
        };
        println!("  {}. [{}] {}", i + 1, label, hit.content);
    }

    // The persona entry should rank first (highest combined boost), the English
    // entry second, and the neutral entry last.
    assert_eq!(
        hits[0].id, persona_id,
        "persona-signaled entry should rank first"
    );
    assert_eq!(hits[2].id, neutral_id, "neutral entry should rank last");

    println!("\nRanking is correct — lexicon scoring is working.");
    Ok(())
}
