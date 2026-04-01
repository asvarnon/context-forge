use std::path::Path;

/// Read a JSONL transcript file and convert to BM25-friendly plain text.
/// Returns the formatted transcript string.
pub fn read_transcript(path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read transcript file {}: {e}", path.display()))?;

    let mut turns: Vec<String> = Vec::new();

    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match format_turn(trimmed, i + 1) {
            Some(text) => turns.push(text),
            None => { /* filtered out or skipped */ }
        }
    }

    if turns.is_empty() {
        return Err("no conversation content found in transcript".into());
    }

    Ok(turns.join("\n\n"))
}

/// Parse a single JSONL line and return formatted text, or None if filtered out.
fn format_turn(line: &str, line_number: usize) -> Option<String> {
    let obj: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("skipping malformed JSONL line {line_number}: {e}");
            return None;
        }
    };

    let turn_type = obj.get("type")?.as_str()?;

    match turn_type {
        "user" | "assistant" => {}
        _ => return None, // system, file-history-snapshot, queue-operation, last-prompt, etc.
    }

    let header = format!("[{turn_type}]");
    let content = obj.get("message")?.get("content")?;

    let mut parts: Vec<String> = Vec::new();
    parts.push(header);

    if let Some(text) = content.as_str() {
        // Content is a plain string (common for user messages).
        parts.push(text.to_owned());
    } else if let Some(arr) = content.as_array() {
        // Content is an array of typed blocks.
        for block in arr {
            let block_type = match block.get("type").and_then(|v| v.as_str()) {
                Some(t) => t,
                None => continue,
            };

            match block_type {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                        parts.push(t.to_owned());
                    }
                }
                "thinking" => {
                    if let Some(t) = block.get("thinking").and_then(|v| v.as_str()) {
                        parts.push(t.to_owned());
                    }
                    // Skip `signature` field intentionally.
                }
                "tool_use" => {
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    parts.push(format!("[tool_use: {name}]"));
                    if let Some(input) = block.get("input") {
                        if let Ok(json) = serde_json::to_string(input) {
                            parts.push(json);
                        }
                    }
                }
                "tool_result" => {
                    parts.push("[tool_result]".to_owned());
                    if let Some(inner) = block.get("content").and_then(|v| v.as_array()) {
                        for item in inner {
                            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                                    parts.push(t.to_owned());
                                }
                            }
                        }
                    }
                }
                _ => {} // unknown block type, skip
            }
        }
    }

    // If we only have the header and nothing else, skip this turn.
    if parts.len() <= 1 {
        return None;
    }

    Some(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_jsonl(name: &str, content: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("cf_test_{name}_{}.jsonl", std::process::id()));
        fs::write(&path, content).expect("failed to write temp file");
        path
    }

    #[test]
    fn test_user_and_assistant_messages() {
        let jsonl = r#"{"type":"user","message":{"content":"Hello world"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Hi there!"}]}}"#;

        let path = temp_jsonl("user_assistant", jsonl);
        let result = read_transcript(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(result.contains("[user]"));
        assert!(result.contains("Hello world"));
        assert!(result.contains("[assistant]"));
        assert!(result.contains("Hi there!"));
    }

    #[test]
    fn test_tool_use_and_tool_result() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"read_file","input":{"path":"src/main.rs"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":[{"type":"text","text":"fn main() {}"}]}]}}"#;

        let path = temp_jsonl("tool_use_result", jsonl);
        let result = read_transcript(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(result.contains("[tool_use: read_file]"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("[tool_result]"));
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn test_thinking_without_signature() {
        let jsonl = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Let me analyze this...","signature":"encrypted_blob_here"},{"type":"text","text":"Here is my answer."}]}}"#;

        let path = temp_jsonl("thinking", jsonl);
        let result = read_transcript(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(result.contains("Let me analyze this..."));
        assert!(result.contains("Here is my answer."));
        assert!(!result.contains("encrypted_blob_here"));
    }

    #[test]
    fn test_system_lines_filtered() {
        let jsonl = r#"{"type":"system","message":{"content":"compaction marker"}}
{"type":"file-history-snapshot","data":{}}
{"type":"queue-operation","data":{}}
{"type":"last-prompt","data":{}}
{"type":"user","message":{"content":"actual content"}}"#;

        let path = temp_jsonl("system_filtered", jsonl);
        let result = read_transcript(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(!result.contains("compaction marker"));
        assert!(!result.contains("file-history-snapshot"));
        assert!(!result.contains("queue-operation"));
        assert!(!result.contains("last-prompt"));
        assert!(result.contains("actual content"));
    }

    #[test]
    fn test_malformed_line_skipped() {
        let jsonl = r#"this is not json
{"type":"user","message":{"content":"valid line"}}"#;

        let path = temp_jsonl("malformed", jsonl);
        let result = read_transcript(&path).unwrap();
        fs::remove_file(&path).ok();

        assert!(result.contains("valid line"));
    }

    #[test]
    fn test_empty_after_filtering() {
        let jsonl = r#"{"type":"system","message":{"content":"only system"}}
{"type":"file-history-snapshot","data":{}}"#;

        let path = temp_jsonl("empty_filtered", jsonl);
        let result = read_transcript(&path);
        fs::remove_file(&path).ok();

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "no conversation content found in transcript"
        );
    }
}
