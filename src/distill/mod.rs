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

use std::collections::HashSet;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Reduces several [`DistilledMemory`] partial results — one per transcript
/// chunk — into a single [`DistilledMemory`].
///
/// Facts are concatenated in input order and deduplicated on
/// `(kind, normalized text)`, where normalization trims whitespace and
/// lowercases; the first occurrence of each duplicate is kept. Summaries are
/// joined with a blank line between them. The combined result is passed
/// through [`cap_distilled_memory`], so the output is bounded the same way a
/// single distillation's output would be.
///
/// This is the pure, deterministic reduce: no model call, safe to call with
/// an empty `Vec` (returns an empty [`DistilledMemory`]) or a single-element
/// `Vec` (returns that element's content, capped).
#[must_use]
pub fn merge_distilled(parts: Vec<DistilledMemory>) -> DistilledMemory {
    let mut summaries = Vec::with_capacity(parts.len());
    let mut facts = Vec::new();
    let mut seen = HashSet::new();

    for part in parts {
        if !part.summary.is_empty() {
            summaries.push(part.summary);
        }
        for fact in part.facts {
            let key = (fact.kind, fact.text.trim().to_lowercase());
            if seen.insert(key) {
                // "insert into the seen-set, and only
                //keep this fact if that insert was new." One line does both the membership check and the recording
                facts.push(fact);
            }
        }
    }

    cap_distilled_memory(DistilledMemory {
        summary: summaries.join("\n\n"),
        facts,
    })
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
        cap_distilled_memory, merge_distilled, DistilledMemory, Fact, FactKind, MAX_FACTS,
        MAX_FACT_CHARS, MAX_SUMMARY_CHARS,
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

    #[test]
    fn merge_empty_input_returns_empty_memory() {
        let merged = merge_distilled(vec![]);

        assert_eq!(merged.summary, "");
        assert!(merged.facts.is_empty());
    }

    #[test]
    fn merge_single_part_is_passthrough() {
        let part = DistilledMemory {
            summary: "A short summary.".to_owned(),
            facts: vec![fact("A short fact.")],
        };

        let merged = merge_distilled(vec![part.clone()]);

        assert_eq!(merged.summary, part.summary);
        assert_eq!(merged.facts.len(), 1);
        assert_eq!(merged.facts[0].text, part.facts[0].text);
    }

    #[test]
    fn merge_concatenates_facts_from_multiple_parts_in_order() {
        let part1 = DistilledMemory {
            summary: "First.".to_owned(),
            facts: vec![fact("Fact A")],
        };
        let part2 = DistilledMemory {
            summary: "Second.".to_owned(),
            facts: vec![fact("Fact B")],
        };

        let merged = merge_distilled(vec![part1, part2]);

        assert_eq!(merged.facts.len(), 2);
        assert_eq!(merged.facts[0].text, "Fact A");
        assert_eq!(merged.facts[1].text, "Fact B");
    }

    #[test]
    fn merge_dedups_facts_with_same_kind_and_normalized_text() {
        let part1 = DistilledMemory {
            summary: "First.".to_owned(),
            facts: vec![fact("We decided to roll back the deploy.")],
        };
        let part2 = DistilledMemory {
            summary: "Second.".to_owned(),
            // Same fact, different case and trailing whitespace.
            facts: vec![fact("we decided to roll back the deploy.  ")],
        };

        let merged = merge_distilled(vec![part1, part2]);

        // Only the first occurrence survives, with its original text intact.
        assert_eq!(merged.facts.len(), 1);
        assert_eq!(merged.facts[0].text, "We decided to roll back the deploy.");
    }

    #[test]
    fn merge_does_not_dedup_same_text_across_different_kinds() {
        let part = DistilledMemory {
            summary: "summary".to_owned(),
            facts: vec![
                Fact {
                    kind: FactKind::Decision,
                    text: "Same text.".to_owned(),
                },
                Fact {
                    kind: FactKind::State,
                    text: "Same text.".to_owned(),
                },
            ],
        };

        let merged = merge_distilled(vec![part]);

        // Different kind means a different dedup key, even with identical text.
        assert_eq!(merged.facts.len(), 2);
    }

    #[test]
    fn merge_joins_summaries_with_blank_line_then_caps() {
        // Exactly at the cap already, so nothing from the second summary
        // should survive truncation.
        let summary_a = "a".repeat(MAX_SUMMARY_CHARS);
        let summary_b = "b".repeat(1_000);
        let part1 = DistilledMemory {
            summary: summary_a.clone(),
            facts: vec![],
        };
        let part2 = DistilledMemory {
            summary: summary_b,
            facts: vec![],
        };

        let merged = merge_distilled(vec![part1, part2]);

        assert_eq!(merged.summary, summary_a);
        assert_eq!(merged.summary.chars().count(), MAX_SUMMARY_CHARS);
    }

    #[test]
    fn merge_caps_total_facts_at_max_facts() {
        let parts: Vec<DistilledMemory> = (0..MAX_FACTS + 20)
            .map(|i| DistilledMemory {
                summary: String::new(),
                facts: vec![fact(format!("fact number {i}"))],
            })
            .collect();

        let merged = merge_distilled(parts);

        assert_eq!(merged.facts.len(), MAX_FACTS);
    }
}
