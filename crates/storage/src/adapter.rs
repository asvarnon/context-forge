use std::collections::HashMap;

use rusqlite::Connection;
use serde_json::Value;

use cf_core::error::CoreError;

/// A resolved field mapping row from `runtime_field_mappings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FieldMapping {
    pub canonical_field: String,
    pub source_path: String,
    pub transform: Option<String>,
}

/// Detect the runtime from a parsed JSON payload.
///
/// `runtime_hint` overrides detection when present.
#[must_use]
pub(crate) fn detect_runtime(json: &Value, runtime_hint: Option<&str>) -> Option<String> {
    if let Some(hint) = runtime_hint {
        return Some(hint.to_owned());
    }

    if json.get("threadId").and_then(Value::as_str).is_some() {
        return Some("codex".to_owned());
    }
    if json.get("sessionKey").and_then(Value::as_str).is_some() {
        return Some("openclaw".to_owned());
    }
    if json.get("sessionId").and_then(Value::as_str).is_some() {
        return Some("cline".to_owned());
    }

    if let Some(source) = json.get("source").and_then(Value::as_str) {
        if matches!(source, "startup" | "resume" | "compact" | "clear") {
            return Some("claude-code".to_owned());
        }
    }

    if json.get("session_id").and_then(Value::as_str).is_some() {
        return Some("gemini".to_owned());
    }

    None
}

/// Load field mappings for `runtime` from the database.
pub(crate) fn load_mappings(
    conn: &Connection,
    runtime: &str,
) -> cf_core::Result<Vec<FieldMapping>> {
    let mut stmt = conn
        .prepare(
            "SELECT canonical_field, source_path, transform
             FROM runtime_field_mappings
             WHERE runtime = ?1",
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;

    let mappings = stmt
        .query_map([runtime], |row| {
            Ok(FieldMapping {
                canonical_field: row.get(0)?,
                source_path: row.get(1)?,
                transform: row.get(2)?,
            })
        })
        .map_err(|e| CoreError::Storage(e.to_string()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CoreError::Storage(e.to_string()))?;

    Ok(mappings)
}

/// Extract canonical field values from `json` using `mappings`.
#[must_use]
pub(crate) fn extract_fields(json: &Value, mappings: &[FieldMapping]) -> HashMap<String, String> {
    let mut fields = HashMap::new();

    for mapping in mappings {
        if let Some(raw_value) = extract_by_path(json, &mapping.source_path) {
            let value = apply_transform(raw_value, mapping.transform.as_deref());
            fields.insert(mapping.canonical_field.clone(), value);
        }
    }

    fields
}

/// Apply `transform` to `value`.
#[must_use]
pub(crate) fn apply_transform(value: &str, transform: Option<&str>) -> String {
    match transform {
        None | Some("") => value.to_owned(),
        Some("lowercase") => value.to_lowercase(),
        Some(rule) => {
            if let Some(prefix) = rule.strip_prefix("strip_prefix:") {
                if let Some(stripped) = value.strip_prefix(prefix) {
                    stripped.to_owned()
                } else {
                    value.to_owned()
                }
            } else {
                value.to_owned()
            }
        }
    }
}

/// Extract a string value from `json` using dot-notation `path`.
///
/// Supports top-level keys and single-depth dot-notation (e.g., `"git.branch"`).
/// Paths with more than one dot (e.g., `"a.b.c"`) are rejected and return `None`.
#[must_use]
pub(crate) fn extract_by_path<'a>(json: &'a Value, path: &str) -> Option<&'a str> {
    if let Some((first, second)) = path.split_once('.') {
        if first.is_empty() || second.is_empty() || second.contains('.') {
            return None;
        }

        json.get(first)?.get(second)?.as_str()
    } else {
        json.get(path)?.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn detect_runtime_uses_hint_override() {
        let payload = serde_json::json!({"threadId": "abc"});
        let runtime = detect_runtime(&payload, Some("claude-code"));
        assert_eq!(runtime.as_deref(), Some("claude-code"));
    }

    #[test]
    fn detect_runtime_signatures() {
        let codex = serde_json::json!({"threadId": "th-1"});
        assert_eq!(detect_runtime(&codex, None).as_deref(), Some("codex"));

        let openclaw = serde_json::json!({"sessionKey": "sk-1"});
        assert_eq!(detect_runtime(&openclaw, None).as_deref(), Some("openclaw"));

        let cline = serde_json::json!({"sessionId": "sid-1"});
        assert_eq!(detect_runtime(&cline, None).as_deref(), Some("cline"));

        let claude = serde_json::json!({"source": "compact"});
        assert_eq!(
            detect_runtime(&claude, None).as_deref(),
            Some("claude-code")
        );

        let gemini = serde_json::json!({"session_id": "sid-2"});
        assert_eq!(detect_runtime(&gemini, None).as_deref(), Some("gemini"));

        let unknown = serde_json::json!({"foo": "bar"});
        assert_eq!(detect_runtime(&unknown, None), None);
    }

    #[test]
    fn extract_by_path_supports_top_level_and_single_depth_dot_notation() {
        let payload = serde_json::json!({
            "session_id": "sess-1",
            "git": {
                "branch": "feature/runtime"
            }
        });

        assert_eq!(extract_by_path(&payload, "session_id"), Some("sess-1"));
        assert_eq!(
            extract_by_path(&payload, "git.branch"),
            Some("feature/runtime")
        );
    }

    #[test]
    fn extract_by_path_returns_none_for_missing_or_non_string_or_deep_paths() {
        let payload = serde_json::json!({
            "session_id": 100,
            "git": {
                "sha": 42,
                "nested": {
                    "value": "abc"
                }
            }
        });

        assert_eq!(extract_by_path(&payload, "missing"), None);
        assert_eq!(extract_by_path(&payload, "session_id"), None);
        assert_eq!(extract_by_path(&payload, "git.sha"), None);
        assert_eq!(extract_by_path(&payload, "git.nested.value"), None);
    }

    #[test]
    fn apply_transform_variants() {
        assert_eq!(apply_transform("Hello", None), "Hello");
        assert_eq!(apply_transform("Hello", Some("")), "Hello");
        assert_eq!(apply_transform("Hello", Some("lowercase")), "hello");
        assert_eq!(
            apply_transform("agent:assistant", Some("strip_prefix:agent:")),
            "assistant"
        );
        assert_eq!(
            apply_transform("assistant", Some("strip_prefix:agent:")),
            "assistant"
        );
        assert_eq!(apply_transform("Hello", Some("unknown")), "Hello");
    }

    #[test]
    fn extract_fields_applies_mappings_and_transforms() {
        let payload = serde_json::json!({
            "session_id": "session-77",
            "model": "GPT-5.3-CODEX",
            "cwd": "/workspace/context-forge",
            "matcher_value": "COMPACT",
            "agent_type": "agent:assistant",
            "agent_id": "node-1"
        });

        let mappings = vec![
            FieldMapping {
                canonical_field: "session_id".to_owned(),
                source_path: "session_id".to_owned(),
                transform: None,
            },
            FieldMapping {
                canonical_field: "model".to_owned(),
                source_path: "model".to_owned(),
                transform: Some("lowercase".to_owned()),
            },
            FieldMapping {
                canonical_field: "agent_type".to_owned(),
                source_path: "agent_type".to_owned(),
                transform: Some("strip_prefix:agent:".to_owned()),
            },
            FieldMapping {
                canonical_field: "compaction_trigger".to_owned(),
                source_path: "matcher_value".to_owned(),
                transform: None,
            },
            FieldMapping {
                canonical_field: "git_sha".to_owned(),
                source_path: "git.sha".to_owned(),
                transform: None,
            },
        ];

        let extracted = extract_fields(&payload, &mappings);

        assert_eq!(
            extracted.get("session_id").map(String::as_str),
            Some("session-77")
        );
        assert_eq!(
            extracted.get("model").map(String::as_str),
            Some("gpt-5.3-codex")
        );
        assert_eq!(
            extracted.get("agent_type").map(String::as_str),
            Some("assistant")
        );
        assert_eq!(
            extracted.get("compaction_trigger").map(String::as_str),
            Some("COMPACT")
        );
        assert_eq!(extracted.get("git_sha"), None);
    }

    #[test]
    fn load_mappings_reads_seeded_runtime_mappings() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        crate::schema::migrate(&conn).expect("migrate schema");

        let mappings = load_mappings(&conn, "codex").expect("load codex mappings");

        assert!(!mappings.is_empty());
        assert!(mappings.iter().any(|mapping| {
            mapping.canonical_field == "session_id" && mapping.source_path == "threadId"
        }));
    }
}
