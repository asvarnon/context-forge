use crate::entry::ContextEntry;

use super::config::ConfigLexiconScorer;
use super::LexiconScorer;

const DEFAULT_ENGLISH_TOML: &str = include_str!("english_defaults.toml");

/// Always-on baseline scorer for plain-English importance signals.
///
/// Recognizes common English affirmations (`"confirmed"`, `"that's correct"`,
/// `"remember this"`) and negations (`"incorrect"`, `"never mind"`,
/// `"disregard"`). Applied by the builder automatically alongside any persona
/// scorer — no configuration required.
///
/// Domain-specific terms and persona vocabulary belong in
/// [`ConfigLexiconScorer`]; this scorer handles the English layer that exists
/// regardless of persona. A user speaking plain English to a Warhammer 40k
/// bot still uses phrases like "confirmed" and "that's wrong" — those signals
/// should not be invisible to the importance scorer just because no persona
/// config is loaded.
#[derive(Debug, Clone)]
pub struct DefaultEnglishScorer(ConfigLexiconScorer);

impl Default for DefaultEnglishScorer {
    fn default() -> Self {
        Self(
            ConfigLexiconScorer::from_str(DEFAULT_ENGLISH_TOML)
                .expect("built-in English config is valid TOML"),
        )
    }
}

impl LexiconScorer for DefaultEnglishScorer {
    fn score(&self, entry: &ContextEntry, query: &str) -> f32 {
        self.0.score(entry, query)
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

    #[test]
    fn affirmation_boosts_english_confirmation() {
        let scorer = DefaultEnglishScorer::default();
        let boost = scorer.score(&entry("confirmed, that will work"), "");
        assert!(boost > 0.0, "expected affirmation boost, got {boost}");
    }

    #[test]
    fn negation_reduces_english_denial() {
        let scorer = DefaultEnglishScorer::default();
        let boost = scorer.score(&entry("never mind, ignore that approach"), "");
        assert!(boost < 0.0, "expected negation reduction, got {boost}");
    }

    #[test]
    fn neutral_content_scores_zero() {
        let scorer = DefaultEnglishScorer::default();
        let boost = scorer.score(&entry("the quick brown fox jumps over the lazy dog"), "");
        assert_eq!(boost, 0.0);
    }
}
