use std::path::PathBuf;

use crate::{Error, Result};

use super::config::LexiconConfig;

/// A candidate term proposed for addition to the lexicon.
///
/// Produced by the caller's growth-loop analyzer (outside this library)
/// and passed to [`LexiconAppender::append`] after operator approval.
#[derive(Debug, Clone)]
pub struct LexiconProposal {
    /// The term to add.
    pub term: String,
    /// Proposed importance weight (e.g. `1.3`).
    pub weight: f32,
    /// Human-readable rationale produced by the LLM reasoner. Written as a
    /// TOML inline comment on the term line when `Some`.
    pub rationale: Option<String>,
    /// IDs of the entries that provided evidence for this proposal.
    pub source_ids: Vec<String>,
}

/// Atomically appends or removes entries from a TOML lexicon file.
///
/// Uses a write-to-temp-then-rename pattern so a crash mid-write never
/// corrupts the existing file. The temp file is written to the same
/// directory as the target to ensure both are on the same filesystem
/// (required for atomic rename on most platforms).
#[derive(Debug, Clone)]
pub struct LexiconAppender {
    path: PathBuf,
}

impl LexiconAppender {
    /// Create an appender targeting `path`. The file need not exist yet.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    // --- internal helpers ---

    fn read_config(&self) -> Result<LexiconConfig> {
        if self.path.exists() {
            let raw = std::fs::read_to_string(&self.path)
                .map_err(|e| Error::Migration(format!("lexicon read error: {e}")))?;
            toml::from_str(&raw)
                .map_err(|e| Error::Migration(format!("lexicon parse error: {e}")))
        } else {
            Ok(LexiconConfig::default())
        }
    }

    fn write_config(&self, config: &LexiconConfig) -> Result<()> {
        let serialized = toml::to_string_pretty(config)
            .map_err(|e| Error::Migration(format!("lexicon serialize error: {e}")))?;
        self.write_raw(&serialized)
    }

    fn write_raw(&self, content: &str) -> Result<()> {
        let tmp_path = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, content)
            .map_err(|e| Error::Migration(format!("lexicon write error: {e}")))?;
        std::fs::rename(&tmp_path, &self.path)
            .map_err(|e| Error::Migration(format!("lexicon rename error: {e}")))
    }

    // --- public API ---

    /// Append `proposal.term` with `proposal.weight` to the lexicon file.
    ///
    /// Reads the current file (creating an empty config if absent), inserts
    /// the new term, serializes back to TOML, and atomically replaces the
    /// file via a temp-file rename. When `proposal.rationale` is `Some`, the
    /// rationale is written as an inline comment on the term's line.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or
    /// written.
    pub fn append(&self, proposal: &LexiconProposal) -> Result<()> {
        let mut config = self.read_config()?;
        config.terms.insert(proposal.term.clone(), proposal.weight);

        let serialized = toml::to_string_pretty(&config)
            .map_err(|e| Error::Migration(format!("lexicon serialize error: {e}")))?;

        let content = if let Some(rationale) = &proposal.rationale {
            // toml 0.8 quotes keys only when necessary (special chars, whitespace, etc.).
            // Match on both forms anchored by " =" so we don't match key prefixes.
            let bare = format!("{} =", proposal.term);
            let quoted = format!("\"{}\" =", proposal.term.replace('"', "\\\""));
            serialized
                .lines()
                .map(|line| {
                    let t = line.trim_start();
                    if t.starts_with(&bare) || t.starts_with(&quoted) {
                        format!("{line}   # {rationale}")
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
                + "\n"
        } else {
            serialized
        };

        self.write_raw(&content)
    }

    /// Append a pattern to `[affirmations]`, deduplicating case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or written.
    pub fn append_affirmation(&self, pattern: &str) -> Result<()> {
        let mut config = self.read_config()?;
        let lower = pattern.to_lowercase();
        if !config
            .affirmations
            .patterns
            .iter()
            .any(|p| p.to_lowercase() == lower)
        {
            config.affirmations.patterns.push(pattern.to_owned());
        }
        self.write_config(&config)
    }

    /// Append a pattern to `[negations]`, deduplicating case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or written.
    pub fn append_negation(&self, pattern: &str) -> Result<()> {
        let mut config = self.read_config()?;
        let lower = pattern.to_lowercase();
        if !config
            .negations
            .patterns
            .iter()
            .any(|p| p.to_lowercase() == lower)
        {
            config.negations.patterns.push(pattern.to_owned());
        }
        self.write_config(&config)
    }

    /// Remove a term from `[terms]`. No-op if the term is not present.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or written.
    pub fn remove_term(&self, term: &str) -> Result<()> {
        let mut config = self.read_config()?;
        config.terms.remove(term);
        self.write_config(&config)
    }

    /// Remove a pattern from `[affirmations]`, matching case-insensitively.
    /// No-op if the pattern is not present.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or written.
    pub fn remove_affirmation(&self, pattern: &str) -> Result<()> {
        let mut config = self.read_config()?;
        let lower = pattern.to_lowercase();
        config
            .affirmations
            .patterns
            .retain(|p| p.to_lowercase() != lower);
        self.write_config(&config)
    }

    /// Remove a pattern from `[negations]`, matching case-insensitively.
    /// No-op if the pattern is not present.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or written.
    pub fn remove_negation(&self, pattern: &str) -> Result<()> {
        let mut config = self.read_config()?;
        let lower = pattern.to_lowercase();
        config
            .negations
            .patterns
            .retain(|p| p.to_lowercase() != lower);
        self.write_config(&config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_appender() -> (LexiconAppender, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lexicon.toml");
        (LexiconAppender::new(path), dir)
    }

    #[test]
    fn appender_creates_and_updates_file() {
        let (appender, _dir) = make_appender();

        appender
            .append(&LexiconProposal {
                term: "Battle-Sister".into(),
                weight: 1.2,
                rationale: Some("high-salience domain noun".into()),
                source_ids: vec!["id-1".into()],
            })
            .unwrap();

        assert!(appender.path.exists());
        let raw = std::fs::read_to_string(&appender.path).unwrap();
        assert!(raw.contains("Battle-Sister"));
        assert!(raw.contains("high-salience domain noun"));

        appender
            .append(&LexiconProposal {
                term: "Inquisitor".into(),
                weight: 1.1,
                rationale: None,
                source_ids: vec![],
            })
            .unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.terms.len(), 2);
        assert!(config.terms.contains_key("Battle-Sister"));
        assert!(config.terms.contains_key("Inquisitor"));
    }

    #[test]
    fn append_affirmation_deduplicates_case_insensitively() {
        let (appender, _dir) = make_appender();

        appender.append_affirmation("For the Emperor").unwrap();
        appender.append_affirmation("for the emperor").unwrap(); // duplicate
        appender.append_affirmation("FOR THE EMPEROR").unwrap(); // duplicate

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.affirmations.patterns.len(), 1);
    }

    #[test]
    fn append_negation_deduplicates_case_insensitively() {
        let (appender, _dir) = make_appender();

        appender.append_negation("Cogitator returns null").unwrap();
        appender.append_negation("cogitator returns null").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.negations.patterns.len(), 1);
    }

    #[test]
    fn remove_term_noop_for_missing() {
        let (appender, _dir) = make_appender();

        appender
            .append(&LexiconProposal {
                term: "Omnissiah".into(),
                weight: 1.2,
                rationale: None,
                source_ids: vec![],
            })
            .unwrap();

        // Removing a term that doesn't exist must not error or disturb other terms.
        appender.remove_term("NonExistent").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.terms.len(), 1);
        assert!(config.terms.contains_key("Omnissiah"));
    }

    #[test]
    fn remove_term_removes_existing() {
        let (appender, _dir) = make_appender();

        appender
            .append(&LexiconProposal {
                term: "Omnissiah".into(),
                weight: 1.2,
                rationale: None,
                source_ids: vec![],
            })
            .unwrap();
        appender.remove_term("Omnissiah").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert!(config.terms.is_empty());
    }

    #[test]
    fn remove_affirmation_case_insensitive() {
        let (appender, _dir) = make_appender();

        appender.append_affirmation("For the Emperor").unwrap();
        appender.append_affirmation("It shall be done").unwrap();
        appender.remove_affirmation("FOR THE EMPEROR").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.affirmations.patterns.len(), 1);
        assert_eq!(config.affirmations.patterns[0], "It shall be done");
    }

    #[test]
    fn remove_negation_case_insensitive() {
        let (appender, _dir) = make_appender();

        appender.append_negation("Cogitator returns null").unwrap();
        appender.append_negation("The logic fails").unwrap();
        appender.remove_negation("COGITATOR RETURNS NULL").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.negations.patterns.len(), 1);
        assert_eq!(config.negations.patterns[0], "The logic fails");
    }

    #[test]
    fn remove_affirmation_noop_for_missing() {
        let (appender, _dir) = make_appender();

        appender.append_affirmation("For the Emperor").unwrap();
        appender.remove_affirmation("NonExistent").unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&appender.path).unwrap()).unwrap();
        assert_eq!(config.affirmations.patterns.len(), 1);
    }
}
