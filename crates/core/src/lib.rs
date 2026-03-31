//! `cf-core` — types, traits, and error definitions for the context-forge engine.
//!
//! This crate contains **zero I/O**. It defines the data model, trait contracts,
//! configuration structs, and error types that the rest of the workspace depends on.

pub mod config;
pub mod entry;
pub mod error;
pub mod traits;

// Re-export primary types at crate root for convenience.
pub use config::{CoreConfig, EvictionPolicy};
pub use entry::{ContextEntry, EntryKind, ScoredEntry};
pub use error::CoreError;
pub use traits::{ContextStorage, Result, Searcher};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn context_entry_json_roundtrip() {
        let entry = ContextEntry {
            id: "e1".into(),
            content: "hello world".into(),
            timestamp: 1_700_000_000,
            source: "manual".into(),
            token_count: Some(3),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ContextEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "e1");
        assert_eq!(back.token_count, Some(3));
    }

    #[test]
    fn core_error_display_messages() {
        let storage = CoreError::Storage("disk full".into());
        assert_eq!(storage.to_string(), "storage error: disk full");

        let budget = CoreError::TokenBudgetExceeded {
            requested: 500,
            budget: 100,
        };
        assert_eq!(
            budget.to_string(),
            "token budget exceeded: requested 500, budget 100"
        );

        let invalid = CoreError::InvalidEntry("empty content".into());
        assert_eq!(invalid.to_string(), "invalid entry: empty content");

        let config = CoreError::Config("missing field".into());
        assert_eq!(config.to_string(), "configuration error: missing field");
    }

    #[test]
    fn core_config_json_roundtrip() {
        let cfg = CoreConfig {
            max_entries: 1000,
            token_budget: 8192,
            db_path: PathBuf::from("/tmp/cf.db"),
            eviction_policy: EvictionPolicy::Lru,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CoreConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_entries, 1000);
        assert_eq!(back.eviction_policy, EvictionPolicy::Lru);
    }

    #[test]
    fn trait_objects_are_object_safe() {
        // This test verifies that the traits compile as trait objects.
        fn _assert_storage(_s: Box<dyn ContextStorage>) {}
        fn _assert_searcher(_s: Box<dyn Searcher>) {}
    }

    #[test]
    fn entry_kind_equality() {
        assert_eq!(EntryKind::Manual, EntryKind::Manual);
        assert_eq!(EntryKind::PreCompact, EntryKind::PreCompact);
        assert_eq!(EntryKind::Auto, EntryKind::Auto);
        assert_ne!(EntryKind::Manual, EntryKind::Auto);
    }

    #[test]
    fn scored_entry_json_roundtrip() {
        let scored = ScoredEntry {
            entry: ContextEntry {
                id: "s1".into(),
                content: "search hit".into(),
                timestamp: 1_700_000_001,
                source: "auto".into(),
                token_count: None,
            },
            score: 0.95,
        };
        let json = serde_json::to_string(&scored).unwrap();
        let back: ScoredEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.entry.id, "s1");
        assert!((back.score - 0.95).abs() < f64::EPSILON);
    }
}
