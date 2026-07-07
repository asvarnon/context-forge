//! Ingest strategies — how a LongMemEval haystack is loaded into CF before
//! querying. This is the key experimental dimension: raw turns vs. distilled
//! facts. Both tag every resulting entry with its source `session_id` so recall
//! scoring maps retrieved entries straight back to gold sessions.

use context_forge::{ContextForge, SaveOptions};

use crate::dataset::Instance;

/// Kind tag applied to raw-ingested turns. (Distilled entries get their kinds
/// set internally by `distill_and_save`.)
const TURN_KIND: &str = "message";

/// Which ingest path to exercise.
#[derive(Debug, Clone)]
pub enum Ingest {
    /// One CF entry per turn, verbatim content. Zero LLM calls, deterministic.
    /// Isolates the retrieval pipeline; this is the baseline + regression guard.
    RawTurns,
    /// One `distill_and_save` per session. Tests the full pipeline including
    /// distillation. Requires an OpenAI-compatible endpoint via env vars
    /// (`LLM_BASE_URL`, `LLM_MODEL`, optional `LLM_API_KEY`).
    DistillPerSession,
}

impl Ingest {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Ingest::RawTurns => "raw-turns",
            Ingest::DistillPerSession => "distill-session",
        }
    }

    /// Ingest one instance's entire haystack into `forge` under `scope`.
    ///
    /// `scope` isolates this instance from every other in a shared store (we use
    /// the instance's `question_id`), so one `ContextForge` — and one loaded
    /// embedder — serves the whole dataset. Each entry also carries its source
    /// `session_id` for recall scoring.
    pub async fn run(
        &self,
        forge: &ContextForge,
        instance: &Instance,
        scope: &str,
    ) -> anyhow::Result<()> {
        match self {
            Ingest::RawTurns => Self::ingest_raw(forge, instance, scope).await,
            Ingest::DistillPerSession => Self::ingest_distilled(forge, instance, scope).await,
        }
    }

    async fn ingest_raw(
        forge: &ContextForge,
        instance: &Instance,
        scope: &str,
    ) -> anyhow::Result<()> {
        for (session_id, turns) in instance.sessions() {
            for turn in turns {
                // Some LongMemEval turns have empty content; CF rejects empty
                // entries, so skip them rather than fail the whole instance.
                if turn.content.trim().is_empty() {
                    continue;
                }
                let opts = SaveOptions {
                    session_id: Some(session_id.clone()),
                    scope: Some(scope.to_owned()),
                    ..Default::default()
                };
                forge
                    .save(&turn.content, TURN_KIND, &opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("save turn in session {session_id}: {e}"))?;
            }
        }
        Ok(())
    }

    async fn ingest_distilled(
        forge: &ContextForge,
        instance: &Instance,
        scope: &str,
    ) -> anyhow::Result<()> {
        let distiller = build_distiller()?;
        for (session_id, turns) in instance.sessions() {
            let transcript = render_transcript(turns);
            if transcript.trim().is_empty() {
                continue;
            }
            let opts = SaveOptions {
                session_id: Some(session_id.clone()),
                scope: Some(scope.to_owned()),
                ..Default::default()
            };
            forge
                .distill_and_save(&transcript, &distiller, &opts)
                .await
                .map_err(|e| anyhow::anyhow!("distill session {session_id}: {e}"))?;
        }
        Ok(())
    }
}

/// Flatten a session's turns into a `role: content` transcript for distillation.
fn render_transcript(turns: &[crate::dataset::Turn]) -> String {
    let mut s = String::new();
    for turn in turns {
        s.push_str(&turn.role);
        s.push_str(": ");
        s.push_str(&turn.content);
        s.push('\n');
    }
    s
}

/// Construct the distiller from environment configuration.
fn build_distiller() -> anyhow::Result<context_forge::distill::openai_compat::OpenAiCompatDistiller>
{
    let base_url = std::env::var("LLM_BASE_URL").map_err(|_| {
        anyhow::anyhow!("distill ingest requires LLM_BASE_URL (e.g. http://localhost:11434/v1)")
    })?;
    let model = std::env::var("LLM_MODEL")
        .map_err(|_| anyhow::anyhow!("distill ingest requires LLM_MODEL"))?;
    let mut distiller =
        context_forge::distill::openai_compat::OpenAiCompatDistiller::new(base_url, model)
            .map_err(|e| anyhow::anyhow!("building distiller: {e}"))?;
    if let Ok(key) = std::env::var("LLM_API_KEY") {
        distiller = distiller.with_api_key(key);
    }
    Ok(distiller)
}
