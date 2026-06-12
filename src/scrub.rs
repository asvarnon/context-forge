//! Secret scrubbing applied to entry content before persistence.
//!
//! This module is pure (no I/O): it compiles a fixed set of regular
//! expressions once and applies them to redact common credential formats
//! (cloud provider keys, API tokens, private key blocks, JWTs, bearer
//! tokens) before content reaches storage.
//!
//! See the crate-level `# Security` section for the untrusted-memory
//! doctrine that governs how retrieved content must be treated by callers.

use std::borrow::Cow;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Configuration for secret scrubbing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ScrubConfig {
    /// Whether secret scrubbing is applied at save time. Defaults to `true`.
    ///
    /// Disabling this is an explicit opt-out: callers who set this to
    /// `false` are asserting that `content` passed to
    /// [`crate::ContextForge::save`] will never contain secrets, or that
    /// they have their own scrubbing in place.
    pub enabled: bool,
}

impl Default for ScrubConfig {
    /// Defaults to `enabled: true` — secret scrubbing is on unless a caller
    /// explicitly opts out.
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// A single compiled redaction pattern: a regex and the label used in its
/// `[REDACTED:<label>]` replacement.
struct Pattern {
    regex: Regex,
    label: &'static str,
}

/// Hardcoded source patterns: `(label, regex source)`.
///
/// The `openai-key` label intentionally appears twice (two distinct regex
/// shapes share one label).
const PATTERN_SOURCES: &[(&str, &str)] = &[
    ("aws-key", r"\bAKIA[0-9A-Z]{16}\b"),
    ("github-token", r"\bgh[pousr]_[A-Za-z0-9]{36,255}\b"),
    ("anthropic-key", r"\bsk-ant-[A-Za-z0-9\-_]{20,}\b"),
    (
        "openai-key",
        r"\bsk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20}\b",
    ),
    ("openai-key", r"\bsk-proj-[A-Za-z0-9\-_]{20,}\b"),
    ("slack-token", r"\bxox[baprs]-[A-Za-z0-9\-]{10,}\b"),
    (
        "discord-token",
        r"\b[MN][A-Za-z\d]{23,}\.[\w-]{6}\.[\w-]{27,}\b",
    ),
    (
        "private-key",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
    ),
    (
        "jwt",
        r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
    ),
    (
        "generic-bearer",
        r"(?i)\b(bearer|authorization:)\s+[A-Za-z0-9\-._~+/]{20,}=*",
    ),
];

/// Returns the compiled pattern set, building it on first use.
///
/// # Panics
///
/// Panics if any hardcoded pattern in [`PATTERN_SOURCES`] fails to compile.
/// All patterns are fixed literals validated by [`pattern_set_compiles`]; a
/// failure here indicates a programming error in this crate (a malformed
/// built-in security pattern), which must fail loudly rather than silently
/// disable scrubbing for that pattern.
#[allow(
    clippy::expect_used,
    reason = "hardcoded built-in security patterns must compile; a failure is a programming \
              error that must panic loudly rather than silently skip a redaction pattern"
)]
fn pattern_set() -> &'static Vec<Pattern> {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        PATTERN_SOURCES
            .iter()
            .map(|&(label, source)| Pattern {
                regex: Regex::new(source).unwrap_or_else(|err| {
                    panic!("built-in scrub pattern {label:?} failed to compile: {err}")
                }),
                label,
            })
            .collect()
    })
}

/// Scrub known secret formats from `text`, replacing each match with
/// `[REDACTED:<label>]`.
///
/// If `config.enabled` is `false`, returns `text` unchanged (borrowed, no
/// allocation). Otherwise applies every built-in redaction pattern in
/// order; the result is allocation-free (`Cow::Borrowed`) when no pattern
/// matches.
#[must_use]
pub fn scrub_secrets<'a>(text: &'a str, config: &ScrubConfig) -> Cow<'a, str> {
    if !config.enabled {
        return Cow::Borrowed(text);
    }

    let mut current = Cow::Borrowed(text);
    for pattern in pattern_set() {
        if pattern.regex.is_match(&current) {
            let replacement = format!("[REDACTED:{}]", pattern.label);
            let replaced = pattern
                .regex
                .replace_all(&current, replacement.as_str())
                .into_owned();
            current = Cow::Owned(replaced);
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ScrubConfig {
        ScrubConfig::default()
    }

    #[test]
    fn pattern_set_compiles() {
        // Forces initialization; panics if any hardcoded pattern is malformed.
        let patterns = pattern_set();
        assert_eq!(patterns.len(), PATTERN_SOURCES.len());
    }

    #[test]
    fn disabled_config_returns_borrowed_unchanged() {
        let text = "this has a secret AKIAABCDEFGHIJKLMNOP in it";
        let cfg = ScrubConfig { enabled: false };
        let result = scrub_secrets(text, &cfg);
        assert_eq!(result, text);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn no_match_returns_borrowed() {
        let text = "nothing sensitive here, just plain text";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, text);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    // -- aws-key --------------------------------------------------------

    #[test]
    fn aws_key_positive() {
        let text = "key=AKIAABCDEFGHIJKLMNOP end";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, "key=[REDACTED:aws-key] end");
    }

    #[test]
    fn aws_key_near_miss() {
        // Too short (15 chars after AKIA instead of 16).
        let text = "key=AKIAABCDEFGHIJKLMNO end";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, text);
    }

    // -- github-token -----------------------------------------------------

    #[test]
    fn github_token_positive() {
        let token = format!("ghp_{}", "a".repeat(36));
        let text = format!("token: {token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, "token: [REDACTED:github-token]");
    }

    #[test]
    fn github_token_near_miss() {
        // Too short (35 chars after ghp_).
        let token = format!("ghp_{}", "a".repeat(35));
        let text = format!("token: {token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- anthropic-key ------------------------------------------------------

    #[test]
    fn anthropic_key_positive() {
        let token = format!("sk-ant-{}", "A".repeat(20));
        let text = format!("ANTHROPIC_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, "ANTHROPIC_API_KEY=[REDACTED:anthropic-key]");
    }

    #[test]
    fn anthropic_key_near_miss() {
        let token = format!("sk-ant-{}", "A".repeat(19));
        let text = format!("ANTHROPIC_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- openai-key (legacy form) ---------------------------------------

    #[test]
    fn openai_key_legacy_positive() {
        let token = format!("sk-{}T3BlbkFJ{}", "a".repeat(20), "b".repeat(20));
        let text = format!("OPENAI_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, "OPENAI_API_KEY=[REDACTED:openai-key]");
    }

    #[test]
    fn openai_key_legacy_near_miss() {
        // Missing the T3BlbkFJ marker.
        let token = format!("sk-{}X3BlbkFJ{}", "a".repeat(20), "b".repeat(20));
        let text = format!("OPENAI_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- openai-key (project form) ---------------------------------------

    #[test]
    fn openai_key_proj_positive() {
        let token = format!("sk-proj-{}", "A".repeat(20));
        let text = format!("OPENAI_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, "OPENAI_API_KEY=[REDACTED:openai-key]");
    }

    #[test]
    fn openai_key_proj_near_miss() {
        let token = format!("sk-proj-{}", "A".repeat(19));
        let text = format!("OPENAI_API_KEY={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- slack-token ---------------------------------------------------------

    #[test]
    fn slack_token_positive() {
        let text = "SLACK_TOKEN=xoxb-1234567890-abcdefghij";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, "SLACK_TOKEN=[REDACTED:slack-token]");
    }

    #[test]
    fn slack_token_near_miss() {
        // Too short suffix (9 chars instead of >=10).
        let text = "SLACK_TOKEN=xoxb-123456789";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, text);
    }

    // -- discord-token -------------------------------------------------------

    #[test]
    fn discord_token_positive() {
        let token = format!(
            "{}.{}.{}",
            format!("M{}", "A".repeat(23)),
            "a".repeat(6),
            "b".repeat(27)
        );
        let text = format!("DISCORD_TOKEN={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, "DISCORD_TOKEN=[REDACTED:discord-token]");
    }

    #[test]
    fn discord_token_near_miss() {
        // Wrong first character (not M/N).
        let token = format!(
            "{}.{}.{}",
            format!("X{}", "A".repeat(23)),
            "a".repeat(6),
            "b".repeat(27)
        );
        let text = format!("DISCORD_TOKEN={token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- private-key ----------------------------------------------------------

    #[test]
    fn private_key_positive() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\nmore lines\n-----END RSA PRIVATE KEY-----";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, "[REDACTED:private-key]");
    }

    #[test]
    fn private_key_near_miss() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAK\nno end marker here";
        let result = scrub_secrets(text, &cfg());
        assert_eq!(result, text);
    }

    // -- jwt ------------------------------------------------------------------

    #[test]
    fn jwt_positive() {
        let token = format!(
            "eyJ{}.{}.{}",
            "a".repeat(10),
            "b".repeat(10),
            "c".repeat(10)
        );
        let text = format!("Authorization-payload: {token}");
        let result = scrub_secrets(&text, &cfg());
        assert!(result.contains("[REDACTED:jwt]"));
        assert!(!result.contains(&token));
    }

    #[test]
    fn jwt_near_miss() {
        // Only two segments, not three.
        let token = format!("eyJ{}.{}", "a".repeat(10), "b".repeat(10));
        let text = format!("payload: {token}");
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- generic-bearer ---------------------------------------------------------

    #[test]
    fn generic_bearer_positive() {
        let text = format!("Authorization: Bearer {}", "a".repeat(25));
        let result = scrub_secrets(&text, &cfg());
        assert!(result.contains("[REDACTED:generic-bearer]"));
        assert!(!result.to_lowercase().contains("bearer aaaa"));
    }

    #[test]
    fn generic_bearer_near_miss() {
        // Token too short (< 20 chars).
        let text = format!("Bearer {}", "a".repeat(10));
        let result = scrub_secrets(&text, &cfg());
        assert_eq!(result, text);
    }

    // -- multi-secret and idempotency -----------------------------------------

    #[test]
    fn multi_secret_string() {
        let aws = "AKIAABCDEFGHIJKLMNOP";
        let gh_token = format!("ghp_{}", "a".repeat(36));
        let text = format!("aws={aws} github={gh_token} done");
        let result = scrub_secrets(&text, &cfg());
        assert!(result.contains("[REDACTED:aws-key]"));
        assert!(result.contains("[REDACTED:github-token]"));
        assert!(!result.contains(aws));
        assert!(!result.contains(&gh_token));
    }

    #[test]
    fn idempotent() {
        let aws = "AKIAABCDEFGHIJKLMNOP";
        let gh_token = format!("ghp_{}", "a".repeat(36));
        let text = format!("aws={aws} github={gh_token} bearer={}", "x".repeat(25));

        let once = scrub_secrets(&text, &cfg()).into_owned();
        let twice = scrub_secrets(&once, &cfg()).into_owned();
        assert_eq!(once, twice);
    }
}
