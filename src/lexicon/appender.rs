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
    /// Human-readable rationale produced by the LLM reasoner.
    pub rationale: String,
    /// IDs of the entries that provided evidence for this proposal.
    pub source_ids: Vec<String>,
}

/// Atomically appends a new term to a TOML lexicon file.
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

    /// Append `proposal.term` with `proposal.weight` to the lexicon file.
    ///
    /// Reads the current file (creating an empty config if absent), inserts
    /// the new term, serializes back to TOML, and atomically replaces the
    /// file via a temp-file rename.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, parsed, serialized, or
    /// written.
    pub fn append(&self, proposal: &LexiconProposal) -> Result<()> {
        let mut config: LexiconConfig = if self.path.exists() {
            let raw = std::fs::read_to_string(&self.path)
                .map_err(|e| Error::Migration(format!("lexicon read error: {e}")))?;
            toml::from_str(&raw)
                .map_err(|e| Error::Migration(format!("lexicon parse error: {e}")))?
        } else {
            LexiconConfig::default()
        };

        config.terms.insert(proposal.term.clone(), proposal.weight);

        let serialized = toml::to_string_pretty(&config)
            .map_err(|e| Error::Migration(format!("lexicon serialize error: {e}")))?;

        let tmp_path = self.path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &serialized)
            .map_err(|e| Error::Migration(format!("lexicon write error: {e}")))?;
        std::fs::rename(&tmp_path, &self.path)
            .map_err(|e| Error::Migration(format!("lexicon rename error: {e}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appender_creates_and_updates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lexicon.toml");

        let appender = LexiconAppender::new(path.clone());
        appender
            .append(&LexiconProposal {
                term: "Battle-Sister".into(),
                weight: 1.2,
                rationale: "high-salience domain noun".into(),
                source_ids: vec!["id-1".into()],
            })
            .unwrap();

        assert!(path.exists());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("Battle-Sister"));

        // Second append must not corrupt the file.
        appender
            .append(&LexiconProposal {
                term: "Inquisitor".into(),
                weight: 1.1,
                rationale: "authority figure".into(),
                source_ids: vec![],
            })
            .unwrap();

        let config: LexiconConfig =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config.terms.len(), 2);
        assert!(config.terms.contains_key("Battle-Sister"));
        assert!(config.terms.contains_key("Inquisitor"));
    }
}
