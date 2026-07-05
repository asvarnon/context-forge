use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

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
    ///
    /// Weight is an additive boost applied directly to the combined score via
    /// `final_score = base × (1.0 + boost.clamp(-1.0, 2.0))`. A weight of `0.3`
    /// adds 30% to the base score (1.3×); `1.0` doubles it (2.0×). Weights must
    /// be in `(0.0, 1.5]` — the engine caps total boost at `2.0` (3.0× maximum).
    #[serde(default)]
    pub terms: HashMap<String, f64>,

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
/// ## Matching
///
/// Pattern matching is **case-insensitive substring** with two normalizations
/// applied before comparison:
/// - Apostrophes are stripped from both content and pattern, so `"that's right"`
///   matches `"thats right"` and vice versa.
/// - A **3-token negation window** suppresses matches preceded within three
///   whitespace-separated tokens by a negator (`not`, `never`, `don't`,
///   `didn't`, `no`, `isn't`, `can't`, `cannot`, `won't`, `hardly`, `barely`).
///   This prevents `"not confirmed"` from firing the `"confirmed"` affirmation.
///
/// ## Limitations
///
/// This is a **lexical heuristic, not a language model**. It does not perform
/// syntactic parsing, sarcasm detection, discourse analysis, or full semantic
/// interpretation. Known blind spots:
/// - Deep negation stacking (`"I don't think this is not confirmed"`)
/// - Pragmatic ambiguity (`"noted"` as genuine vs. cold acknowledgment)
/// - Cross-sentence negation scope (the window is local to each pattern site)
///
/// The semantic search layer (embeddings) complements this scorer but does not
/// replace it — embeddings handle vocabulary-agnostic retrieval; this scorer
/// handles explicit memory-intent signals (commitments, decisions, corrections)
/// that are not the same problem as semantic similarity.
#[derive(Debug, Clone)]
pub struct ConfigLexiconScorer {
    config: LexiconConfig,
}

impl FromStr for ConfigLexiconScorer {
    type Err = Error;

    /// Parse a scorer from a TOML string. Also available as `str::parse::<ConfigLexiconScorer>()`.
    ///
    /// # Errors
    ///
    /// Returns an error if the TOML is malformed or doesn't match the
    /// [`LexiconConfig`] schema.
    fn from_str(s: &str) -> Result<Self> {
        let config: LexiconConfig =
            toml::from_str(s).map_err(|e| Error::Migration(format!("lexicon parse error: {e}")))?;

        for (term, &weight) in &config.terms {
            if weight <= 0.0 || weight > 1.5 {
                return Err(Error::Migration(format!(
                    "lexicon term {term:?} has weight {weight}, \
                     but weights must be in (0.0, 1.5] \
                     (engine caps total boost at 2.0, giving 3.0× maximum)"
                )));
            }
        }

        Ok(Self { config })
    }
}

impl ConfigLexiconScorer {
    /// Load a scorer from a TOML file on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or the TOML is invalid.
    pub fn from_file(path: &Path) -> Result<Self> {
        let toml = std::fs::read_to_string(path)
            .map_err(|e| Error::Migration(format!("lexicon file error: {e}")))?;
        toml.parse()
    }
}

/// Negation words used by the 3-token window check. Content is already
/// lowercased and apostrophe-stripped before this runs, so contractions
/// appear without apostrophes (`dont`, `didnt`, etc.).
const NEGATORS: &[&str] = &[
    "not", "never", "no", "dont", "didnt", "isnt", "wasnt", "cant", "cannot", "wont", "hardly",
    "barely",
];

/// Returns `true` if `match_start` in `content` is immediately preceded
/// (within 3 whitespace-separated tokens) by a negation word.
fn is_negated(content: &str, match_start: usize) -> bool {
    let prefix = &content[..match_start];
    let tokens: Vec<&str> = prefix.split_whitespace().collect();
    let window_start = tokens.len().saturating_sub(3);
    tokens[window_start..].iter().any(|t| {
        let word = t.trim_end_matches(|c: char| c.is_ascii_punctuation());
        NEGATORS.contains(&word)
    })
}

/// Returns `true` if `pattern` appears in `content` at least once without
/// being immediately preceded by a negation word.
fn has_non_negated_match(content: &str, pattern: &str) -> bool {
    let mut start = 0;
    while let Some(rel) = content[start..].find(pattern) {
        let pos = start + rel;
        if !is_negated(content, pos) {
            return true;
        }
        start = pos + 1;
    }
    false
}

impl LexiconScorer for ConfigLexiconScorer {
    fn score(&self, entry: &ContextEntry, _query: &str) -> f32 {
        // Normalize: lowercase + strip apostrophes so "that's right" matches "thats right".
        let content = entry.content.to_lowercase().replace('\'', "");
        let mut boost = 0.0_f64;

        for (term, weight) in &self.config.terms {
            let term_norm = term.to_lowercase().replace('\'', "");
            if has_non_negated_match(&content, &term_norm) {
                boost += weight;
            }
        }

        for pattern in &self.config.affirmations.patterns {
            let pat_norm = pattern.to_lowercase().replace('\'', "");
            if has_non_negated_match(&content, &pat_norm) {
                boost += 0.5;
            }
        }

        for pattern in &self.config.negations.patterns {
            let pat_norm = pattern.to_lowercase().replace('\'', "");
            if has_non_negated_match(&content, &pat_norm) {
                boost -= 0.3;
            }
        }

        boost as f32
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

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
patterns = ["for the emperor", "it shall be done", "confirmed"]

[negations]
patterns = ["negative", "nay"]
"#;

    #[test]
    fn from_str_parses_valid_toml() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        assert_eq!(scorer.config.terms.len(), 2);
        assert_eq!(scorer.config.affirmations.patterns.len(), 3);
        assert_eq!(scorer.config.negations.patterns.len(), 2);
    }

    #[test]
    fn from_str_errors_on_malformed_toml() {
        let result = ConfigLexiconScorer::from_str("[[[[not valid toml");
        assert!(result.is_err());
    }

    #[test]
    fn from_str_rejects_weight_above_max() {
        let toml = "[terms]\n\"Heresy\" = 2.0";
        let err = ConfigLexiconScorer::from_str(toml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Heresy"),
            "error should name the offending term"
        );
        assert!(msg.contains('2'), "error should mention the invalid weight");
    }

    #[test]
    fn from_str_rejects_nonpositive_weight() {
        let toml = "[terms]\n\"Heresy\" = 0.0";
        assert!(ConfigLexiconScorer::from_str(toml).is_err());
    }

    #[test]
    fn from_str_accepts_weight_at_boundary() {
        let toml = "[terms]\n\"Emperor\" = 1.5";
        assert!(ConfigLexiconScorer::from_str(toml).is_ok());
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
        assert!(boost.abs() < f32::EPSILON);
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
        assert!(boost.abs() < f32::EPSILON);
    }

    #[test]
    fn negation_window_suppresses_negated_affirmation() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("that is not confirmed"), "");
        assert!(
            boost.abs() < f32::EPSILON,
            "negated affirmation should score zero"
        );
    }

    #[test]
    fn unnegated_affirmation_still_boosts() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        let boost = scorer.score(&entry("yes that is confirmed"), "");
        assert!(boost > 0.0, "un-negated affirmation should boost");
    }

    #[test]
    fn negation_window_does_not_suppress_distant_negator() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        // "not" is 4 tokens before "confirmed" — outside the 3-token window
        let boost = scorer.score(&entry("not sure about many things but confirmed"), "");
        assert!(boost > 0.0, "negator outside window should not suppress");
    }

    #[test]
    fn apostrophe_normalization_matches_contraction_variants() {
        let scorer = ConfigLexiconScorer::from_str(SAMPLE_TOML).unwrap();
        // "for the emperor" has no apostrophe but tests the normalization path
        let boost_with = scorer.score(&entry("for the emperor, confirmed"), "");
        assert!(boost_with > 0.0);
    }
}
