use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::entry::ContextEntry;
use crate::{Error, Result};

use super::LexiconScorer;

/// TOML schema for [`ConfigLexiconScorer`].
///
/// ```toml
/// [terms]
/// "Omnissiah" = 1.3
/// "Astartes"  = 1.4
///
/// [affirmations]
/// patterns = ["for the emperor", "it shall be done"]
///
/// [negations]
/// patterns = ["negative", "nay"]
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LexiconConfig {
    /// Domain-specific terms mapped to their importance weight.
    /// Weight is additive boost (e.g. `1.3` means 30% extra importance).
    #[serde(default)]
    pub terms: HashMap<String, f32>,

    /// Phrases that signal affirmation/confirmation in this persona's dialect.
    /// Each match adds a fixed `+0.5` boost.
    #[serde(default)]
    pub affirmations: LexiconPatterns,

    /// Phrases that signal negation/rejection. Each match subtracts `0.3`.
    #[serde(default)]
    pub negations: LexiconPatterns,
}

/// A list of case-insensitive substring patterns.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LexiconPatterns {
    /// The pattern strings to match against entry content.
    #[serde(default)]
    pub patterns: Vec<String>,
}

/// [`LexiconScorer`] backed by a TOML config file.
///
/// Term matching is **case-insensitive substring** — "Astartes" matches
/// "the Astartes warriors" and "astartes legion". Multi-word terms work the
/// same way.
///
/// Score is additive and uncapped on the positive side. On the negative side,
/// the engine clamps the boost floor to `-1.0` so entries are never assigned
/// a negative combined score.
#[derive(Debug, Clone)]
pub struct ConfigLexiconScorer {
    config: LexiconConfig,
}

impl ConfigLexiconScorer {
    /// Parse a scorer from a TOML string.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is malformed or doesn't match the
    /// [`LexiconConfig`] schema.
    pub fn from_str(toml: &str) -> Result<Self> {
        let config = toml::from_str(toml)
            .map_err(|e| Error::Migration(format!("lexicon parse error: {e}")))?;
        Ok(Self { config })
    }

    /// Load a scorer from a TOML file on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or the TOML is invalid.
    pub fn from_file(path: &Path) -> Result<Self> {
        let toml = std::fs::read_to_string(path)
            .map_err(|e| Error::Migration(format!("lexicon file error: {e}")))?;
        Self::from_str(&toml)
    }
}

impl LexiconScorer for ConfigLexiconScorer {
    fn score(&self, entry: &ContextEntry, _query: &str) -> f32 {
        // Normalize apostrophes so "that's right" matches "thats right" and vice versa.
        let content_lower = entry.content.to_lowercase().replace('\'', "");
        let mut boost = 0.0_f32;

        for (term, weight) in &self.config.terms {
            let term_norm = term.to_lowercase().replace('\'', "");
            if content_lower.contains(term_norm.as_str()) {
                boost += weight;
            }
        }

        for pattern in &self.config.affirmations.patterns {
            let pat_norm = pattern.to_lowercase().replace('\'', "");
            if content_lower.contains(pat_norm.as_str()) {
                boost += 0.5;
            }
        }

        for pattern in &self.config.negations.patterns {
            let pat_norm = pattern.to_lowercase().replace('\'', "");
            if content_lower.contains(pat_norm.as_str()) {
                boost -= 0.3;
            }
        }

        boost
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::kind;

    fn entry(content: &str) -> ContextEntry {
        ContextEntry {
            id: "test".into(),
            content: content.into(),
            timestamp: 0,
            kind: kind::MANUAL.to_owned(),
            scope: None,
            session_id: None,
            token_count: None,
            metadata: None,
        }
    }

    const SAMPLE_TOML: &str = r#"
[terms]
"Omnissiah" = 1.3
"Astartes"  = 1.4

[affirmations]
patterns = ["for the emperor", "it shall be done"]

[negations]
patterns = ["negative", "nay"]
"#;

    #[test]
    fn from_str_parses_valid_toml() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        assert_eq!(scorer.config.terms.len(), 2);
        assert_eq!(scorer.config.affirmations.patterns.len(), 2);
        assert_eq!(scorer.config.negations.patterns.len(), 2);
    }

    #[test]
    fn from_str_errors_on_malformed_toml() {
        let result = ConfigLexiconScorer::from_str("[[[[not valid toml");
        assert!(result.is_err());
    }

    #[test]
    fn term_match_is_case_insensitive() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("the omnissiah guides our path"), "");
        assert!(boost > 0.0, "expected positive boost, got {boost}");
    }

    #[test]
    fn no_match_returns_zero() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("nothing relevant here"), "");
        assert_eq!(boost, 0.0);
    }

    #[test]
    fn affirmation_adds_boost() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("for the emperor, we march"), "");
        assert!(boost > 0.0);
    }

    #[test]
    fn negation_reduces_boost() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("negative, we cannot proceed"), "");
        assert!(boost < 0.0);
    }

    #[test]
    fn empty_config_scores_zero() {
        let scorer = ConfigLexiconScorer::from_str("").unwrap();
        let boost = scorer.score(&entry("for the emperor and Astartes"), "");
        assert_eq!(boost, 0.0);
    }
}
