use std::collections::HashMap;

use crate::ngrams::{bigrams, trigrams};

/// Compute term counts for a list of tokens.
///
/// Returns a map from term to count of occurrences.
#[must_use]
pub fn term_counts(tokens: &[String]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for token in tokens {
        *counts.entry(token.clone()).or_insert(0) += 1;
    }

    counts
}

/// Compute term counts including n-grams.
///
/// Computes occurrence counts for unigrams, bigrams, and trigrams combined.
/// N-grams are space-separated strings (e.g., "system openssl").
#[must_use]
pub fn term_counts_with_ngrams(tokens: &[String]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for token in tokens {
        *counts.entry(token.clone()).or_insert(0) += 1;
    }

    for bigram in bigrams(tokens) {
        *counts.entry(bigram).or_insert(0) += 1;
    }

    for trigram in trigrams(tokens) {
        *counts.entry(trigram).or_insert(0) += 1;
    }

    counts
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{term_counts, term_counts_with_ngrams};

    #[test]
    fn test_term_counts() {
        let tokens = vec![
            "hello".to_string(),
            "world".to_string(),
            "hello".to_string(),
        ];
        let counts = term_counts(&tokens);

        assert_eq!(counts.get("hello"), Some(&2));
        assert_eq!(counts.get("world"), Some(&1));
        assert_eq!(counts.len(), 2);
    }

    #[test]
    fn test_term_counts_empty() {
        let tokens: Vec<String> = Vec::new();
        let counts = term_counts(&tokens);
        assert!(counts.is_empty());
    }

    #[test]
    fn test_term_counts_with_ngrams() {
        let tokens = vec![
            "system".to_string(),
            "openssl".to_string(),
            "upgrade".to_string(),
        ];
        let counts = term_counts_with_ngrams(&tokens);

        assert_eq!(counts.get("system"), Some(&1));
        assert_eq!(counts.get("openssl"), Some(&1));
        assert_eq!(counts.get("upgrade"), Some(&1));
        assert_eq!(counts.get("system openssl"), Some(&1));
        assert_eq!(counts.get("openssl upgrade"), Some(&1));
        assert_eq!(counts.get("system openssl upgrade"), Some(&1));
        assert_eq!(counts.len(), 6);
    }

    #[test]
    fn test_single_token_with_ngrams() {
        let tokens = vec!["solo".to_string()];
        let counts = term_counts_with_ngrams(&tokens);

        let mut expected = HashMap::new();
        expected.insert("solo".to_string(), 1_usize);
        assert_eq!(counts, expected);
    }
}
