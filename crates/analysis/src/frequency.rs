use std::collections::HashMap;

use crate::ngrams::{bigrams, trigrams};

/// Compute term frequency for a list of tokens.
///
/// Returns a map from term to count of occurrences.
#[must_use]
pub fn term_counts(tokens: &[String]) -> HashMap<String, usize> {
    let mut frequencies: HashMap<String, usize> = HashMap::new();

    for token in tokens {
        *frequencies.entry(token.clone()).or_insert(0) += 1;
    }

    frequencies
}

/// Compute term frequency including n-grams.
///
/// Computes frequency counts for unigrams, bigrams, and trigrams combined.
/// N-grams are space-separated strings (e.g., "system openssl").
#[must_use]
pub fn term_counts_with_ngrams(tokens: &[String]) -> HashMap<String, usize> {
    let mut combined_terms: Vec<String> = tokens.to_vec();
    combined_terms.extend(bigrams(tokens));
    combined_terms.extend(trigrams(tokens));

    term_counts(&combined_terms)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{term_counts, term_counts_with_ngrams};

    #[test]
    fn test_term_frequency() {
        let tokens = vec![
            "hello".to_string(),
            "world".to_string(),
            "hello".to_string(),
        ];
        let frequencies = term_counts(&tokens);

        assert_eq!(frequencies.get("hello"), Some(&2));
        assert_eq!(frequencies.get("world"), Some(&1));
        assert_eq!(frequencies.len(), 2);
    }

    #[test]
    fn test_term_frequency_empty() {
        let tokens: Vec<String> = Vec::new();
        let frequencies = term_counts(&tokens);
        assert!(frequencies.is_empty());
    }

    #[test]
    fn test_term_frequency_with_ngrams() {
        let tokens = vec![
            "system".to_string(),
            "openssl".to_string(),
            "upgrade".to_string(),
        ];
        let frequencies = term_counts_with_ngrams(&tokens);

        assert_eq!(frequencies.get("system"), Some(&1));
        assert_eq!(frequencies.get("openssl"), Some(&1));
        assert_eq!(frequencies.get("upgrade"), Some(&1));
        assert_eq!(frequencies.get("system openssl"), Some(&1));
        assert_eq!(frequencies.get("openssl upgrade"), Some(&1));
        assert_eq!(frequencies.get("system openssl upgrade"), Some(&1));
        assert_eq!(frequencies.len(), 6);
    }

    #[test]
    fn test_single_token_with_ngrams() {
        let tokens = vec!["solo".to_string()];
        let frequencies = term_counts_with_ngrams(&tokens);

        let mut expected = HashMap::new();
        expected.insert("solo".to_string(), 1_usize);
        assert_eq!(frequencies, expected);
    }
}
