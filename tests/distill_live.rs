//! Live integration tests for [`OpenAiCompatDistiller`] and
//! [`ChunkingDistiller`], gated behind the `distill-http` feature and
//! `#[ignore]`d so neither runs in CI.
//!
//! Run manually against a real Ollama or llama-server instance:
//!
//! ```sh
//! # Ollama (default OpenAi schema style):
//! CF_DISTILL_URL=http://127.0.0.1:11434/v1 CF_DISTILL_MODEL=llama3.1 \
//!     cargo test --features distill-http --test distill_live -- --ignored
//!
//! # llama-server (json_object+schema style; model value is ignored by the server):
//! CF_DISTILL_URL=http://127.0.0.1:8080/v1 CF_DISTILL_MODEL=any \
//!     CF_DISTILL_STYLE=llama-server \
//!     cargo test --features distill-http --test distill_live -- --ignored
//! ```
//!
//! The second test (`..._via_chunking_distiller_...`) is the one that
//! matters for confirming the original OOM bug stays fixed: watch host RAM
//! while it runs and confirm it never spikes the way a single oversized
//! prompt did before chunking existed. Run just that one with `--exact
//! distills_oversized_transcript_via_chunking_distiller_against_live_endpoint`.

#![cfg(feature = "distill-http")]

use context_forge::distill::openai_compat::{OpenAiCompatDistiller, SchemaStyle};
use context_forge::{split_on_budget, ChunkingDistiller, DistilledMemory, Distiller, Result};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

const FIXTURE_TRANSCRIPT: &str = "\
User: The staging deploy failed because the DATABASE_URL env var was unset.
Assistant: I see — the deployment script expects DATABASE_URL to be set in the \
staging environment's secrets manager. I'll add a check for it.
User: Good idea. Also, going forward, please always run `cargo test` before \
suggesting a deploy.
Assistant: Understood. I'll run the test suite before any deploy suggestion \
from now on.
User: One more thing — the staging database is now hosted on the new \
Postgres 16 cluster, not the old one.
";

/// A larger transcript with multiple distinct turns, sized so a small
/// `MAX_CHUNK_CHARS` forces several real chunks (and therefore several real
/// HTTP calls to the live endpoint) instead of hitting `ChunkingDistiller`'s
/// single-chunk fast path.
const CHUNKED_FIXTURE_TRANSCRIPT: &str = "\
User: I want to switch our deploy pipeline to use canary releases instead of an all-at-once rollout.
Assistant: Got it. What rollout percentage should the canary start at, and how long before promoting to 100%?
User: Start at 5%, hold for fifteen minutes, then ramp to 25%, 50%, then 100% if error rates stay under 1%.
Assistant: Understood. I'll wire that staged ramp into the pipeline config and add automatic rollback on the error-rate threshold.
User: Also, from now on please write commit messages as a single terse line, no multi-paragraph bodies.
Assistant: Noted, terse one-line commit messages going forward.
User: One more thing, the staging database moved to the new Postgres 16 cluster, the old one is being decommissioned next week.
Assistant: I'll update the staging connection string and flag any remaining references to the old cluster.
User: Last item, remember that I prefer Rust for any new service in this stack unless there's a hard reason not to.
Assistant: Understood, Rust is the default choice for new services going forward.
";
const MAX_CHUNK_CHARS: usize = 180;

/// Reads `CF_DISTILL_URL`/`CF_DISTILL_MODEL`/`CF_DISTILL_STYLE`, returning
/// `None` (after printing a skip message) if the endpoint isn't configured.
fn live_endpoint_config() -> Option<(String, String, SchemaStyle)> {
    let (Ok(base_url), Ok(model)) = (
        std::env::var("CF_DISTILL_URL"),
        std::env::var("CF_DISTILL_MODEL"),
    ) else {
        eprintln!(
            "skipping: set CF_DISTILL_URL and CF_DISTILL_MODEL to run this test \
against a live endpoint"
        );
        return None;
    };

    let style = match std::env::var("CF_DISTILL_STYLE").as_deref() {
        Ok("llama-server") => SchemaStyle::LlamaServer,
        _ => SchemaStyle::OpenAi,
    };

    Some((base_url, model, style))
}

#[test]
#[ignore = "requires a live OpenAI-compatible endpoint; set CF_DISTILL_URL and CF_DISTILL_MODEL"]
fn distills_fixture_transcript_against_live_endpoint() {
    let Some((base_url, model, style)) = live_endpoint_config() else {
        return;
    };

    let distiller = OpenAiCompatDistiller::new(base_url, model)
        .expect("construct distiller")
        .with_schema_style(style);

    let memory = distiller
        .distill(FIXTURE_TRANSCRIPT)
        .expect("distill transcript");

    assert!(
        !memory.summary.trim().is_empty(),
        "expected a non-empty summary"
    );
}

/// Wraps a [`Distiller`] and prints per-call progress (call number, size
/// sent, elapsed time, outcome) so a long-running multi-chunk live test
/// shows which call is in flight, instead of going silent until the final
/// success or failure.
struct LoggingDistiller<D: Distiller> {
    inner: D,
    next_call: AtomicUsize,
}

impl<D: Distiller> LoggingDistiller<D> {
    fn new(inner: D) -> Self {
        Self {
            inner,
            next_call: AtomicUsize::new(1),
        }
    }
}

impl<D: Distiller> Distiller for LoggingDistiller<D> {
    fn distill(&self, transcript: &str) -> Result<DistilledMemory> {
        let call_number = self.next_call.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "  call {call_number}: sending {} char(s)...",
            transcript.len()
        );
        let start = Instant::now();
        let result = self.inner.distill(transcript);
        match &result {
            Ok(_) => eprintln!(
                "  call {call_number}: ok in {:.1}s",
                start.elapsed().as_secs_f64()
            ),
            Err(e) => eprintln!(
                "  call {call_number}: FAILED in {:.1}s: {e}",
                start.elapsed().as_secs_f64()
            ),
        }
        result
    }
}

#[test]
#[ignore = "requires a live OpenAI-compatible endpoint; set CF_DISTILL_URL and CF_DISTILL_MODEL"]
fn distills_oversized_transcript_via_chunking_distiller_against_live_endpoint() {
    let Some((base_url, model, style)) = live_endpoint_config() else {
        return;
    };

    let chunk_count = split_on_budget(CHUNKED_FIXTURE_TRANSCRIPT, MAX_CHUNK_CHARS).len();
    eprintln!(
        "transcript is {} chars, split into {chunk_count} chunk(s) of <= {MAX_CHUNK_CHARS} chars \
each -- watch host RAM while this runs; it should stay flat across every chunk instead of \
spiking the way one oversized prompt did before chunking existed",
        CHUNKED_FIXTURE_TRANSCRIPT.len()
    );
    assert!(
        chunk_count > 1,
        "fixture transcript must be large enough to force multiple chunks -- adjust \
CHUNKED_FIXTURE_TRANSCRIPT or MAX_CHUNK_CHARS if this fails"
    );

    let inner = OpenAiCompatDistiller::new(base_url, model)
        .expect("construct distiller")
        .with_schema_style(style);
    let chunking = ChunkingDistiller::new(LoggingDistiller::new(inner), MAX_CHUNK_CHARS);

    let memory = chunking
        .distill(CHUNKED_FIXTURE_TRANSCRIPT)
        .expect("distill oversized transcript via ChunkingDistiller");

    eprintln!(
        "merged result: {}-char summary, {} fact(s)",
        memory.summary.len(),
        memory.facts.len()
    );
    assert!(
        !memory.summary.trim().is_empty(),
        "expected a non-empty merged summary"
    );
}
