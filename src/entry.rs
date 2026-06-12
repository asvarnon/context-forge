use serde::{Deserialize, Serialize};

/// A single context entry stored in the memory engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// Unique identifier for this entry.
    pub id: String,
    /// The text content of the entry.
    pub content: String,
    /// Unix timestamp (seconds) when the entry was created.
    pub timestamp: i64,
    /// Classification of how this entry was created.
    pub kind: EntryKind,
    /// Optional pre-computed token count.
    pub token_count: Option<usize>,
    /// Optional runtime session identifier.
    pub session_id: Option<String>,
    /// Optional number of compactions observed for this entry.
    pub compaction_count: Option<i64>,
    /// Optional trigger or matcher that caused compaction.
    pub compaction_trigger: Option<String>,
    /// Optional source runtime identifier.
    pub runtime: Option<String>,
    /// Optional model identifier for the generating runtime.
    pub model: Option<String>,
    /// Optional current working directory from the runtime.
    pub cwd: Option<String>,
    /// Optional git branch captured at save time.
    pub git_branch: Option<String>,
    /// Optional git revision captured at save time.
    pub git_sha: Option<String>,
    /// Optional runtime turn identifier.
    pub turn_id: Option<String>,
    /// Optional agent type for multi-agent runtimes.
    pub agent_type: Option<String>,
    /// Optional agent identifier for multi-agent runtimes.
    pub agent_id: Option<String>,
}

/// Classification of how an entry was created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum EntryKind {
    /// Manually inserted by the user or extension.
    #[default]
    Manual,
    /// Captured during a pre-compact hook.
    PreCompact,
    /// Automatically captured by the engine.
    Auto,
}

#[allow(clippy::derivable_impls)]
impl Default for ContextEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            content: String::new(),
            timestamp: 0,
            kind: EntryKind::default(),
            token_count: None,
            session_id: None,
            compaction_count: None,
            compaction_trigger: None,
            runtime: None,
            model: None,
            cwd: None,
            git_branch: None,
            git_sha: None,
            turn_id: None,
            agent_type: None,
            agent_id: None,
        }
    }
}

/// A search result with relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEntry {
    /// The matched entry.
    pub entry: ContextEntry,
    /// Relevance score (higher is more relevant).
    pub score: f64,
}
