//! LongMemEval dataset schema and loader.
//!
//! Schema reconstructed from the LongMemEval repo/paper documentation
//! (<https://github.com/xiaowu0162/LongMemEval>, ICLR 2025). The loader is
//! intentionally permissive (`#[serde(default)]` on non-essential fields) so
//! minor schema drift in the published files does not hard-fail the run.
//!
//! **Verify against the real file on first use** — these field names are from
//! documentation, not a byte-for-byte read of the JSON. If deserialization
//! fails, diff this struct against one instance of the downloaded dataset.

use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

/// One evaluation instance: a question plus the full multi-session "haystack"
/// it must be answered from, with gold evidence annotations.
///
/// Mirrors the dataset schema in full. Some fields (`answer`, `question_date`,
/// `haystack_dates`) are retained for planned work — `answer` for the Track 2
/// end-to-end QA run, the date fields for temporal-reasoning analysis — and are
/// not yet read by the Track 1 retrieval scorer.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Instance {
    /// Unique id. Ids ending in `_abs` are abstention questions (the answer is
    /// not present in the haystack; the correct behavior is to decline).
    pub question_id: String,
    /// One of: single-session-user, single-session-assistant,
    /// single-session-preference, temporal-reasoning, knowledge-update,
    /// multi-session.
    #[serde(default)]
    pub question_type: String,
    pub question: String,
    /// The gold answer. Type varies in the real data (string, integer, list),
    /// so it is kept as an untyped JSON value; only Track 2 (QA judging) reads it.
    #[serde(default)]
    pub answer: serde_json::Value,
    #[serde(default)]
    pub question_date: String,
    /// Session id per haystack session (parallel to `haystack_sessions`).
    pub haystack_session_ids: Vec<String>,
    /// Timestamp per haystack session (parallel to `haystack_sessions`).
    #[serde(default)]
    pub haystack_dates: Vec<String>,
    /// The chat history: a list of sessions, each a list of turns.
    pub haystack_sessions: Vec<Vec<Turn>>,
    /// Gold: the session ids that actually contain the answer evidence.
    #[serde(default)]
    pub answer_session_ids: Vec<String>,
}

/// A single conversational turn within a session.
#[derive(Debug, Deserialize)]
pub struct Turn {
    pub role: String,
    pub content: String,
    /// Gold turn-level label: `true` if this turn holds answer evidence.
    /// Retained from the schema for a planned turn-level recall metric; not yet
    /// read by the session-level scorer.
    #[serde(default)]
    #[allow(dead_code)]
    pub has_answer: bool,
}

impl Instance {
    /// Abstention questions carry no retrievable evidence, so they are excluded
    /// from recall averaging (there is no gold set to recall).
    #[must_use]
    pub fn is_abstention(&self) -> bool {
        self.question_id.ends_with("_abs") || self.answer_session_ids.is_empty()
    }

    /// The gold evidence sessions as a set, for membership checks.
    #[must_use]
    pub fn gold_sessions(&self) -> HashSet<String> {
        self.answer_session_ids.iter().cloned().collect()
    }

    /// `(session_id, session_turns)` pairs, zipping the parallel id list with
    /// the session list. Extra ids or sessions past the shorter length are
    /// dropped (defensive against ragged data).
    pub fn sessions(&self) -> impl Iterator<Item = (&String, &Vec<Turn>)> {
        self.haystack_session_ids
            .iter()
            .zip(self.haystack_sessions.iter())
    }
}

/// Load a LongMemEval JSON file (e.g. `longmemeval_s.json`) into instances.
pub fn load(path: &Path) -> anyhow::Result<Vec<Instance>> {
    let bytes =
        std::fs::read(path).map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let instances: Vec<Instance> = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("parsing {} as LongMemEval JSON: {e}", path.display()))?;
    Ok(instances)
}
