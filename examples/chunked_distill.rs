//! Demonstrates chunked distillation: `ChunkingDistiller` splits a long
//! transcript into budget-sized pieces, distills each piece independently
//! through the wrapped `Distiller`, and merges the partial results into one
//! `DistilledMemory` before `distill_and_save` persists it.
//!
//! Uses a tiny hand-rolled `Distiller` instead of the HTTP-backed
//! `OpenAiCompatDistiller` so this example needs no model server and no
//! feature flags — `ChunkingDistiller` wraps *any* `Distiller`. Run with:
//!
//! ```bash
//! cargo run --example chunked_distill
//! ```

use context_forge::{
    ChunkingDistiller, Config, ContextForge, DistilledMemory, Distiller, Fact, FactKind,
    SaveOptions,
};
use std::path::PathBuf;

/// A `Distiller` that treats each non-empty line of its input as one fact,
/// and summarizes by line count. Stands in for a real model call: deployed
/// code would use `OpenAiCompatDistiller` (behind the `distill-http`
/// feature) pointed at Ollama, llama-server, or any OpenAI-compatible
/// endpoint instead.
struct LineCountDistiller;

impl Distiller for LineCountDistiller {
    fn distill(&self, transcript: &str) -> context_forge::Result<DistilledMemory> {
        let lines: Vec<&str> = transcript.lines().filter(|l| !l.is_empty()).collect();
        eprintln!("  -> distilling a chunk of {} line(s)", lines.len());

        Ok(DistilledMemory {
            summary: format!(
                "This chunk covered {} line(s) of conversation.",
                lines.len()
            ),
            facts: lines
                .into_iter()
                .map(|line| Fact {
                    kind: FactKind::State,
                    text: line.to_owned(),
                })
                .collect(),
        })
    }
}

fn main() -> Result<(), context_forge::Error> {
    let mut config = Config::default();
    config.db_path = PathBuf::from("chunked-distill-example.db");
    let cf = ContextForge::open(config)?;

    // Long enough that a small budget below forces multiple chunks.
    let transcript = "\
User: I want to switch our deploy pipeline to use canary releases.
Assistant: Sounds good -- what rollout percentage do you want to start with?
User: Let's start at 5% and ramp to 100% over an hour if metrics stay green.
Assistant: Noted, I'll wire that into the pipeline config.
User: Also, remember I prefer terse commit messages, one line max.
";

    // A real caller sizes this from the target model's context window and
    // host RAM -- that's deployment policy, not something this crate
    // decides. Small here purely to force multiple chunks in one example.
    const MAX_CHUNK_CHARS: usize = 120;

    let distiller = ChunkingDistiller::new(LineCountDistiller, MAX_CHUNK_CHARS);

    let opts = SaveOptions {
        scope: Some("project:demo".to_owned()),
        ..SaveOptions::default()
    };

    println!(
        "Distilling a {}-character transcript in chunks of <= {MAX_CHUNK_CHARS} characters:",
        transcript.len()
    );
    let ids = cf.distill_and_save(transcript, &distiller, &opts)?;
    println!(
        "Saved {} entries (1 merged summary + {} facts).",
        ids.len(),
        ids.len() - 1
    );

    let hits = cf.query("canary commit", Some("project:demo"), 2048)?;
    println!("\nQuery results for \"canary commit\":");
    for hit in &hits {
        println!("{}: {}", hit.id, hit.content);
    }

    Ok(())
}
