//! Distillation: turning a raw conversation transcript into durable memory.
//!
//! This module defines the [`Distiller`] trait and the data types it
//! produces. The trait itself is always available (not feature-gated) so
//! that callers can implement their own distillers — for example, a remote
//! API client, a different local model runtime, or a test stub.
//!
//! The [`OpenAiCompatDistiller`](openai_compat::OpenAiCompatDistiller)
//! implementation (behind the `distill-http` feature) talks to an
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
