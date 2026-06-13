//! Distillation: turning a raw conversation transcript into durable memory.
//!
//! This module defines the [`Distiller`] trait and the data types it
//! produces. The trait itself is always available (not feature-gated) so
//! that callers can implement their own distillers — for example, a remote
//! API client, a different local model runtime, or a test stub.
//!
//! The `OpenAiCompatDistiller` implementation in the `openai_compat`
//! submodule (behind the `distill-http` feature) talks to an
//! OpenAI-compatible chat completions endpoint such as Ollama or
//! llama-server. It is the only place in this crate that performs HTTP, and
//! only when that feature is enabled.

#[cfg(feature = "distill-http")]
pub mod openai_compat;

use serde::{Deserialize, Serialize};

use crate::traits::Result;

/// The result of distilling a conversation transcript into durable memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistilledMemory {
    /// A summary of the conversation, intended to be under 150 words.
    pub summary: String,
    /// Individual facts worth remembering across future sessions.
    pub facts: Vec<Fact>,
}

/// A single distilled fact extracted from a transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// The category of this fact.
    pub kind: FactKind,
    /// One self-contained sentence describing the fact, understandable
    /// without the original transcript.
    pub text: String,
}

/// The category of a distilled [`Fact`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum FactKind {
    /// A decision that was made, and (ideally) why.
    Decision,
    /// A correction the user gave.
    Correction,
    /// A user preference.
    Preference,
    /// A state change ("X is now Y").
    State,
}

/// Maximum number of facts persisted from a single distillation. Excess
/// facts (untrusted model output) are dropped.
pub const MAX_FACTS: usize = 64;
/// Maximum character length of a single distilled fact's text. Longer text
/// is truncated on a `char` boundary.
pub const MAX_FACT_CHARS: usize = 2_048;
/// Maximum character length of a distilled summary. Longer text is
/// truncated on a `char` boundary.
pub const MAX_SUMMARY_CHARS: usize = 8_192;

/// Truncates `text` to at most `max_chars` Unicode scalar values, keeping
/// the beginning and dropping the tail. Truncation always lands on a `char`
/// boundary.
fn truncate_keep_start(text: &str, max_chars: usize) -> &str {
    match text.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &text[..byte_idx],
        None => text,
    }
}

/// Caps a [`DistilledMemory`] produced by an untrusted [`Distiller`] so that
/// it cannot persist unbounded data.
///
/// The summary is truncated to at most [`MAX_SUMMARY_CHARS`] characters, the
/// fact list is truncated to at most [`MAX_FACTS`] entries, and each
/// surviving fact's text is truncated to at most [`MAX_FACT_CHARS`]
/// characters. All truncation keeps the beginning of the text and is
/// silent: no error, no marker, no logging.
pub(crate) fn cap_distilled_memory(memory: DistilledMemory) -> DistilledMemory {
    let DistilledMemory { summary, mut facts } = memory;

    let summary = truncate_keep_start(&summary, MAX_SUMMARY_CHARS).to_owned();

    facts.truncate(MAX_FACTS);
    for fact in &mut facts {
        if fact.text.chars().count() > MAX_FACT_CHARS {
            fact.text = truncate_keep_start(&fact.text, MAX_FACT_CHARS).to_owned();
        }
    }

    DistilledMemory { summary, facts }
}

/// Produces [`DistilledMemory`] from a raw conversation transcript.
///
/// Implementations must be thread-safe (`Send + Sync`) so a single
/// distiller instance can be shared across worker threads.
pub trait Distiller: Send + Sync {
    /// Distill `transcript` into a summary and a list of durable facts.
    ///
    /// # Security
    ///
    /// Implementations transmit `transcript` verbatim to the underlying
    /// model or service — no secret scrubbing is applied at this layer.
    /// [`ContextForge::distill_and_save`](crate::ContextForge::distill_and_save)
    /// is the only entry point that scrubs secrets (via
    /// [`scrub_secrets`](crate::scrub_secrets)) before a transcript reaches
    /// a [`Distiller`]; callers invoking [`Distiller::distill`] directly are
    /// responsible for scrubbing first.
    ///
    /// # Errors
    ///
    /// Returns an error if distillation fails (e.g. the backing model or
    /// service is unavailable, or its response cannot be parsed).
    fn distill(&self, transcript: &str) -> Result<DistilledMemory>;
}

#[cfg(test)]
mod tests {
    use super::{
        cap_distilled_memory, DistilledMemory, Fact, FactKind, MAX_FACTS, MAX_FACT_CHARS,
        MAX_SUMMARY_CHARS,
    };

    fn fact(text: impl Into<String>) -> Fact {
        Fact {
            kind: FactKind::State,
            text: text.into(),
        }
    }

    #[test]
    fn cap_drops_excess_facts() {
        let facts = (0..MAX_FACTS + 50)
            .map(|i| fact(format!("fact {i}")))
            .collect();
        let memory = DistilledMemory {
            summary: "summary".to_owned(),
            facts,
        };

        let capped = cap_distilled_memory(memory);

        assert_eq!(capped.facts.len(), MAX_FACTS);
    }

    #[test]
    fn cap_truncates_long_fact_text() {
        let long_text = "a".repeat(MAX_FACT_CHARS + 1000);
        let memory = DistilledMemory {
            summary: "summary".to_owned(),
            facts: vec![fact(long_text.clone())],
        };

        let capped = cap_distilled_memory(memory);

        assert_eq!(capped.facts[0].text.chars().count(), MAX_FACT_CHARS);
        assert!(long_text.starts_with(&capped.facts[0].text));
    }

    #[test]
    fn cap_truncates_long_summary() {
        let long_summary = "b".repeat(MAX_SUMMARY_CHARS + 1000);
        let memory = DistilledMemory {
            summary: long_summary.clone(),
            facts: vec![],
        };

        let capped = cap_distilled_memory(memory);

        assert_eq!(capped.summary.chars().count(), MAX_SUMMARY_CHARS);
        assert!(long_summary.starts_with(&capped.summary));
    }

    #[test]
    fn cap_respects_char_boundaries() {
        let long_text = "é".repeat(MAX_FACT_CHARS + 10);
        let memory = DistilledMemory {
            summary: "🎉".repeat(MAX_SUMMARY_CHARS + 10),
            facts: vec![fact(long_text)],
        };

        let capped = cap_distilled_memory(memory);

        assert_eq!(capped.facts[0].text.chars().count(), MAX_FACT_CHARS);
        assert_eq!(capped.summary.chars().count(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn cap_leaves_compliant_memory_unchanged() {
        let memory = DistilledMemory {
            summary: "A short summary.".to_owned(),
            facts: vec![fact("A short fact.")],
        };

        let capped = cap_distilled_memory(memory.clone());

        assert_eq!(capped.summary, memory.summary);
        assert_eq!(capped.facts.len(), memory.facts.len());
        assert_eq!(capped.facts[0].text, memory.facts[0].text);
    }
}
