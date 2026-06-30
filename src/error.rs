//! Crate-wide error type.

use thiserror::Error;

/// All errors that can originate from this crate.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the underlying Turso database.
    #[error("storage error: {0}")]
    Turso(#[from] turso::Error),

    /// An entry failed validation.
    #[error("invalid entry: {0}")]
    InvalidEntry(String),

    /// A schema migration failed.
    #[error("migration error: {0}")]
    Migration(String),

    /// A distillation operation failed.
    #[error("distillation error: {0}")]
    Distill(String),
}
