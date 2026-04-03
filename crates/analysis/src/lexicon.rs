/// Configurable lexicon sets for category detection.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Lexicons {
    pub negation_markers: Vec<String>,
    pub comparison_markers: Vec<String>,
    pub causal_connectors: Vec<String>,
    pub confirmation_tokens: Vec<String>,
    pub state_operators: Vec<String>,
}

impl Default for Lexicons {
    fn default() -> Self {
        Self {
            negation_markers: vec![
                "not".to_string(),
                "no".to_string(),
                "never".to_string(),
                "neither".to_string(),
                "nor".to_string(),
                "don't".to_string(),
                "doesn't".to_string(),
                "didn't".to_string(),
                "won't".to_string(),
                "wouldn't".to_string(),
                "can't".to_string(),
                "cannot".to_string(),
                "couldn't".to_string(),
                "shouldn't".to_string(),
                "isn't".to_string(),
                "aren't".to_string(),
                "wasn't".to_string(),
                "weren't".to_string(),
                "nothing".to_string(),
                "nowhere".to_string(),
                "nobody".to_string(),
                "none".to_string(),
                "without".to_string(),
            ],
            comparison_markers: vec![
                "better".to_string(),
                "worse".to_string(),
                "over".to_string(),
                "instead".to_string(),
                "rather".to_string(),
                "prefer".to_string(),
                "chose".to_string(),
                "picked".to_string(),
                "replaced".to_string(),
                "switched".to_string(),
                "upgraded".to_string(),
                "downgraded".to_string(),
                "versus".to_string(),
                "vs".to_string(),
                "compared".to_string(),
                "alternative".to_string(),
                "option".to_string(),
                "decided".to_string(),
                "went with".to_string(),
                "selected".to_string(),
            ],
            causal_connectors: vec![
                "because".to_string(),
                "since".to_string(),
                "due to".to_string(),
                "caused by".to_string(),
                "reason".to_string(),
                "therefore".to_string(),
                "so that".to_string(),
                "as a result".to_string(),
                "which means".to_string(),
                "in order to".to_string(),
                "leads to".to_string(),
                "that's why".to_string(),
                "the reason is".to_string(),
                "this is because".to_string(),
            ],
            confirmation_tokens: vec![
                "yes".to_string(),
                "perfect".to_string(),
                "correct".to_string(),
                "exactly".to_string(),
                "good".to_string(),
                "right".to_string(),
                "agreed".to_string(),
                "confirmed".to_string(),
                "approved".to_string(),
                "lgtm".to_string(),
            ],
            state_operators: vec![
                "=".to_string(),
                ":=".to_string(),
                "set to".to_string(),
                "changed to".to_string(),
                "now uses".to_string(),
                "updated to".to_string(),
                "IP:".to_string(),
                "port:".to_string(),
                "is now".to_string(),
                "is set to".to_string(),
                "is changed to".to_string(),
                "is updated to".to_string(),
                "→".to_string(),
                "->".to_string(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Lexicons;

    #[test]
    fn default_lexicons_have_expected_counts() {
        let lexicons = Lexicons::default();

        assert_eq!(lexicons.negation_markers.len(), 23);
        assert_eq!(lexicons.comparison_markers.len(), 20);
        assert_eq!(lexicons.causal_connectors.len(), 14);
        assert_eq!(lexicons.confirmation_tokens.len(), 10);
        assert_eq!(lexicons.state_operators.len(), 14);
    }

    #[test]
    fn state_operators_do_not_contain_bare_is() {
        let lexicons = Lexicons::default();

        assert!(lexicons
            .state_operators
            .iter()
            .all(|operator| operator.to_lowercase() != "is"));
    }
}
