/// Generate n-grams from a token list.
///
/// Produces n-grams by sliding a window of size `n` over the tokens
/// and joining with a single space.
///
/// Returns an empty vec if `tokens.len()` < n.
#[must_use]
pub fn extract(tokens: &[String], n: usize) -> Vec<String> {
    if n == 0 || tokens.len() < n {
        return Vec::new();
    }

    tokens.windows(n).map(|window| window.join(" ")).collect()
}

/// Generate bigrams from a token list.
#[must_use]
pub fn bigrams(tokens: &[String]) -> Vec<String> {
    extract(tokens, 2)
}

/// Generate trigrams from a token list.
#[must_use]
pub fn trigrams(tokens: &[String]) -> Vec<String> {
    extract(tokens, 3)
}

#[cfg(test)]
mod tests {
    use super::{bigrams, extract, trigrams};

    fn sample_tokens() -> Vec<String> {
        ["a", "b", "c", "d"]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn test_bigrams() {
        let tokens = sample_tokens();
        let expected = vec!["a b".to_string(), "b c".to_string(), "c d".to_string()];
        assert_eq!(bigrams(&tokens), expected);
    }

    #[test]
    fn test_trigrams() {
        let tokens = sample_tokens();
        let expected = vec!["a b c".to_string(), "b c d".to_string()];
        assert_eq!(trigrams(&tokens), expected);
    }

    #[test]
    fn test_ngrams_insufficient_tokens() {
        let tokens = vec!["a".to_string()];
        assert!(extract(&tokens, 2).is_empty());
    }

    #[test]
    fn test_ngrams_exact_size() {
        let tokens = vec!["a".to_string(), "b".to_string()];
        assert_eq!(extract(&tokens, 2), vec!["a b".to_string()]);
    }

    #[test]
    fn test_unigrams() {
        let tokens = sample_tokens();
        assert_eq!(extract(&tokens, 1), tokens);
    }

    #[test]
    fn test_zero_gram() {
        let tokens = sample_tokens();
        assert!(extract(&tokens, 0).is_empty());
    }
}
