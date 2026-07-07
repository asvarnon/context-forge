//! PersonaMem-v2 dataset: benchmark CSV + per-persona chat-history JSON.
//!
//! Source: HuggingFace `bowen-upenn/PersonaMem-v2` (see README). Verified
//! 2026-07-07 against the real files: `related_conversation_snippet` is a JSON
//! array of `{role, content}` whose `content` matches a chat-history message
//! **exactly**, so retrieval scoring maps snippet → ingested entry by content
//! equality with no normalization.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One benchmark row: a query about a persona's current preference, with the
/// evidence turn(s) annotated. Only the columns the retrieval eval uses are
/// deserialized (csv+serde ignores the rest by header name).
#[derive(Debug, Deserialize)]
pub struct Row {
    pub persona_id: String,
    pub chat_history_32k_link: String,
    pub user_query: String,
    /// JSON array string of `{role, content}` — the turn(s) where the user
    /// implicitly stated the current preference. This is the retrieval ground
    /// truth. Parse with [`Row::gold_contents`].
    pub related_conversation_snippet: String,
    /// Whether this preference supersedes an earlier one (`"True"`/`"False"`).
    /// The `updated == true` slice is the sharpest lexicon-benefit signal
    /// (current-vs-stale preference evolution).
    #[serde(default)]
    pub updated: String,
    /// Preference category (stereotypical / anti-stereotypical / neutral /
    /// therapy / health). Retained from the schema for future per-type slicing;
    /// not yet read by the overall/updated scorer.
    #[serde(default)]
    #[allow(dead_code)]
    pub pref_type: String,
}

impl Row {
    /// True if this row is a preference-*update* case.
    #[must_use]
    pub fn is_updated(&self) -> bool {
        self.updated.eq_ignore_ascii_case("true")
    }

    /// The gold evidence contents: the `content` field of each turn in
    /// `related_conversation_snippet`. Returns empty if the JSON can't be parsed
    /// (row is then excluded from scoring, like an abstention).
    #[must_use]
    pub fn gold_contents(&self) -> Vec<String> {
        #[derive(Deserialize)]
        struct Turn {
            content: String,
        }
        serde_json::from_str::<Vec<Turn>>(&self.related_conversation_snippet)
            .map(|turns| turns.into_iter().map(|t| t.content).collect())
            .unwrap_or_default()
    }
}

/// A chat-history file: `{ metadata, chat_history: [{role, content}, ...] }`.
#[derive(Debug, Deserialize)]
pub struct ChatHistory {
    pub chat_history: Vec<Message>,
}

#[derive(Debug, Deserialize)]
pub struct Message {
    /// Speaker role. Present in the schema; the eval ingests content regardless
    /// of role, so this is not currently read.
    #[allow(dead_code)]
    pub role: String,
    pub content: String,
}

/// All rows for one persona, plus its (lazily loaded) chat history. Grouping by
/// persona lets us ingest the ~189-message history **once** and run all of the
/// persona's queries against it (queries are ~1ms; ingest is the cost).
pub struct Persona {
    pub persona_id: String,
    pub chat_history_link: String,
    pub rows: Vec<Row>,
}

/// Load `benchmark.csv` and group rows by persona, preserving first-seen order.
pub fn load_grouped(csv_path: &Path) -> anyhow::Result<Vec<Persona>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(csv_path)
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", csv_path.display()))?;

    let mut order: Vec<String> = Vec::new();
    let mut by_persona: HashMap<String, Persona> = HashMap::new();

    for result in reader.deserialize() {
        let row: Row = result.map_err(|e| anyhow::anyhow!("parsing benchmark.csv row: {e}"))?;
        let pid = row.persona_id.clone();
        let entry = by_persona.entry(pid.clone()).or_insert_with(|| {
            order.push(pid.clone());
            Persona {
                persona_id: pid.clone(),
                chat_history_link: row.chat_history_32k_link.clone(),
                rows: Vec::new(),
            }
        });
        entry.rows.push(row);
    }

    Ok(order
        .into_iter()
        .filter_map(|pid| by_persona.remove(&pid))
        .collect())
}

/// Load a persona's chat history. `link` is dataset-relative (e.g.
/// `data/chat_history_32k/...json`); `data_root` is where the dataset lives.
pub fn load_chat_history(data_root: &Path, link: &str) -> anyhow::Result<ChatHistory> {
    let path: PathBuf = data_root.join(link);
    let bytes = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("reading chat history {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("parsing chat history {}: {e}", path.display()))
}
