use thiserror::Error;

/// All errors that can originate from the core engine.
#[derive(Debug, Error)]
pub enum CoreError {
    /// An error from the storage layer.
    #[error("storage error: {0}")]
    Storage(String),

    /// The requested token count exceeds the configured budget.
    #[error("token budget exceeded: requested {requested}, budget {budget}")]
    TokenBudgetExceeded {
        /// Tokens requested.
        requested: usize,
        /// Configured budget limit.
        budget: usize,
    },

    /// An entry failed validation.
    #[error("invalid entry: {0}")]
    InvalidEntry(String),

    /// A configuration value is invalid.
    #[error("configuration error: {0}")]
    Config(String),
}
