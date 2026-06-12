//! Live integration test for [`OpenAiCompatDistiller`], gated behind the
//! `distill-http` feature and `#[ignore]`d so it never runs in CI.
//!
//! Run manually against a real Ollama or llama-server instance:
//!
//! ```sh
//! CF_DISTILL_URL=http://127.0.0.1:11434/v1 CF_DISTILL_MODEL=llama3.1 \
//!     cargo test --features distill-http --test distill_live -- --ignored
//! ```

#![cfg(feature = "distill-http")]

use context_forge::distill::openai_compat::OpenAiCompatDistiller;
use context_forge::Distiller;

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

#[test]
#[ignore = "requires a live OpenAI-compatible endpoint; set CF_DISTILL_URL and CF_DISTILL_MODEL"]
fn distills_fixture_transcript_against_live_endpoint() {
    let (Ok(base_url), Ok(model)) = (
        std::env::var("CF_DISTILL_URL"),
        std::env::var("CF_DISTILL_MODEL"),
    ) else {
        eprintln!(
            "skipping: set CF_DISTILL_URL and CF_DISTILL_MODEL to run this test \
against a live endpoint"
        );
        return;
    };

    let distiller = OpenAiCompatDistiller::new(base_url, model).expect("construct distiller");

    let memory = distiller
        .distill(FIXTURE_TRANSCRIPT)
        .expect("distill transcript");

    assert!(
        !memory.summary.trim().is_empty(),
        "expected a non-empty summary"
    );
}
