use std::collections::HashSet;

use stop_words::{get, LANGUAGE};

/// Configuration for the tokenizer.
#[derive(Debug, Default, Clone)]
pub struct TokenizerConfig {
    /// Custom stopwords to add to the default set.
    pub extra_stopwords: Vec<String>,
    /// If true, use only `extra_stopwords` and skip the default English set.
    pub custom_only: bool,
}

/// A configured tokenizer that normalizes and filters text into terms.
#[derive(Debug, Clone)]
pub struct Tokenizer {
    stopwords: HashSet<String>,
}

impl Tokenizer {
    /// Create a new tokenizer with the given configuration.
    ///
    /// Loads the default English stopword list from `stop-words` crate
    /// unless `config.custom_only` is true. Merges `extra_stopwords`.
    #[must_use]
    pub fn new(config: &TokenizerConfig) -> Self {
        let mut stopwords: HashSet<String> = if config.custom_only {
            HashSet::new()
        } else {
            get(LANGUAGE::English)
                .iter()
                .map(|word| word.to_ascii_lowercase())
                .collect()
        };

        stopwords.extend(
            config
                .extra_stopwords
                .iter()
                .map(|word| word.to_ascii_lowercase()),
        );

        Self { stopwords }
    }

    /// Tokenize text into a list of normalized terms.
    ///
    /// 1. Lowercase the entire input (ASCII only)
    /// 2. Split on ASCII whitespace
    /// 3. Strip non-alphanumeric characters from each token (keep only a-z, 0-9)
    /// 4. Remove empty tokens
    /// 5. Remove stopwords
    ///
    /// **Note:** Input is treated as ASCII; non-ASCII characters are silently dropped.
    #[must_use]
    pub fn tokenize(&self, text: &str) -> Vec<String> {
        text.to_ascii_lowercase()
            .split_ascii_whitespace()
            .map(|token| {
                token
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
            })
            .filter(|token| !token.is_empty())
            .filter(|token| !self.is_stopword(token))
            .collect()
    }

    /// Return true if the given term is a stopword.
    #[must_use]
    pub fn is_stopword(&self, term: &str) -> bool {
        self.stopwords.contains(&term.to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::{Tokenizer, TokenizerConfig};

    #[test]
    fn test_basic_tokenization() {
        let config = TokenizerConfig {
            extra_stopwords: Vec::new(),
            custom_only: true,
        };
        let tokenizer = Tokenizer::new(&config);
        let tokens = tokenizer.tokenize("Hello World");

        assert_eq!(tokens, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn test_stopword_removal() {
        let config = TokenizerConfig {
            extra_stopwords: vec!["the".to_string(), "is".to_string()],
            custom_only: true,
        };
        let tokenizer = Tokenizer::new(&config);
        let tokens = tokenizer.tokenize("the quick brown fox is running");

        assert_eq!(
            tokens,
            vec![
                "quick".to_string(),
                "brown".to_string(),
                "fox".to_string(),
                "running".to_string()
            ]
        );
    }

    #[test]
    fn test_punctuation_stripping() {
        let config = TokenizerConfig {
            extra_stopwords: Vec::new(),
            custom_only: true,
        };
        let tokenizer = Tokenizer::new(&config);
        let tokens = tokenizer.tokenize("don't, can't; hello!");

        assert_eq!(
            tokens,
            vec!["dont".to_string(), "cant".to_string(), "hello".to_string()]
        );
    }

    #[test]
    fn test_empty_input() {
        let tokenizer = Tokenizer::new(&TokenizerConfig::default());
        let tokens = tokenizer.tokenize("");

        assert!(tokens.is_empty());
    }

    #[test]
    fn test_all_stopwords() {
        let tokenizer = Tokenizer::new(&TokenizerConfig::default());
        let tokens = tokenizer.tokenize("the is a an");

        assert!(tokens.is_empty());
    }

    #[test]
    fn test_custom_stopwords() {
        let config = TokenizerConfig {
            extra_stopwords: vec!["rust".to_string(), "tokio".to_string()],
            custom_only: true,
        };
        let tokenizer = Tokenizer::new(&config);
        let tokens = tokenizer.tokenize("Rust and Tokio are useful");

        assert_eq!(
            tokens,
            vec!["and".to_string(), "are".to_string(), "useful".to_string()]
        );
    }

    #[test]
    fn test_is_stopword() {
        let config = TokenizerConfig {
            extra_stopwords: vec!["custom".to_string()],
            custom_only: false,
        };
        let tokenizer = Tokenizer::new(&config);

        assert!(tokenizer.is_stopword("the"));
        assert!(tokenizer.is_stopword("custom"));
        assert!(!tokenizer.is_stopword("contextforge"));
    }
}
