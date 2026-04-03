/// Toggle for an individual pre-filter category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterToggle {
    /// Apply the category filter.
    #[default]
    Enabled,
    /// Skip the category filter.
    Disabled,
}

impl FilterToggle {
    #[must_use]
    fn is_enabled(self) -> bool {
        self == Self::Enabled
    }
}

/// Configuration for transcript content pre-filtering.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PrefilterConfig {
    /// Strip `[tool_use: ...]` blocks and their associated payload/`[tool_result]` lines.
    pub tool_call_blocks: FilterToggle,
    /// Strip markdown code fences and all lines between them.
    pub code_fence_blocks: FilterToggle,
    /// Strip file path tokens from otherwise preserved lines.
    pub file_path_literals: FilterToggle,
    /// Strip long raw JSON object/array lines.
    pub raw_json_lines: FilterToggle,
    /// Strip transcript role/marker lines (for example `[user]`, `Assistant:`).
    pub structural_markers: FilterToggle,
    /// Strip shell prompt/command lines.
    pub bash_command_lines: FilterToggle,
    /// Minimum character threshold for raw JSON line stripping.
    pub raw_json_min_chars: usize,
    /// Configurable relative path prefixes treated as file path literals.
    pub relative_path_prefixes: Vec<String>,
}

impl Default for PrefilterConfig {
    fn default() -> Self {
        Self {
            tool_call_blocks: FilterToggle::Enabled,
            code_fence_blocks: FilterToggle::Enabled,
            file_path_literals: FilterToggle::Enabled,
            raw_json_lines: FilterToggle::Enabled,
            structural_markers: FilterToggle::Enabled,
            bash_command_lines: FilterToggle::Enabled,
            raw_json_min_chars: 100,
            relative_path_prefixes: vec![
                "crates/".to_string(),
                "src/".to_string(),
                "docs/".to_string(),
                "scripts/".to_string(),
                "target/".to_string(),
                "extension/".to_string(),
                "node_modules/".to_string(),
                "build/".to_string(),
                "dist/".to_string(),
                "lib/".to_string(),
                "test/".to_string(),
                "tests/".to_string(),
            ],
        }
    }
}

/// Strip execution artifacts from transcript content.
#[must_use]
pub fn strip_execution_artifacts(content: &str, config: &PrefilterConfig) -> String {
    // Filter application order (highest priority first):
    //   1. tool-call blocks  (stateful: consumes until [tool_result])
    //   2. code-fence blocks (stateful: consumes until closing ```)
    //   3. raw JSON lines    (stateless, per-line)
    //   4. structural markers (stateless, per-line)
    //   5. bash command lines (stateless, per-line)
    //   6. file-path token stripping (in-place on preserved lines)
    let mut filtered_lines: Vec<String> = Vec::new();
    // State-machine invariant: `in_tool_block` and `in_code_fence` are mutually
    // exclusive by design. Tool-block handling runs first so payloads containing
    // ``` do not enter fence state. Conversely, if a [tool_use: ...] appears while
    // a fence is open, fence state is implicitly terminated because transcript
    // structure does not nest these constructs in practice.
    let mut in_code_fence = false;
    let mut in_tool_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Tool blocks take precedence over code fence state so [tool_result]
        // can always terminate a block even if payload text contains ```.
        if config.tool_call_blocks.is_enabled() {
            // (Tool blocks use asymmetric delimiters: [tool_use: ...] opens,
            // [tool_result] closes; code fences use the same delimiter for both.)
            if in_tool_block {
                if trimmed == "[tool_result]" {
                    in_tool_block = false;
                }
                push_blank_line_if_needed(&mut filtered_lines);
                continue;
            }

            if is_tool_use_line(trimmed) {
                in_tool_block = true;
                in_code_fence = false;
                push_blank_line_if_needed(&mut filtered_lines);
                continue;
            }
        }

        if config.code_fence_blocks.is_enabled() {
            if in_code_fence {
                if is_code_fence_delimiter(trimmed) {
                    in_code_fence = false;
                }
                push_blank_line_if_needed(&mut filtered_lines);
                continue;
            }

            if is_code_fence_delimiter(trimmed) {
                in_code_fence = true;
                push_blank_line_if_needed(&mut filtered_lines);
                continue;
            }
        }

        if config.raw_json_lines.is_enabled()
            && is_raw_json_line(trimmed, config.raw_json_min_chars)
        {
            push_blank_line_if_needed(&mut filtered_lines);
            continue;
        }

        if config.structural_markers.is_enabled() && is_structural_marker_line(trimmed) {
            push_blank_line_if_needed(&mut filtered_lines);
            continue;
        }

        if config.bash_command_lines.is_enabled() && is_bash_command_line(trimmed) {
            push_blank_line_if_needed(&mut filtered_lines);
            continue;
        }

        let cleaned = if config.file_path_literals.is_enabled() {
            strip_file_path_tokens(line, config)
        } else {
            line.to_string()
        };

        if cleaned.trim().is_empty() {
            push_blank_line_if_needed(&mut filtered_lines);
        } else {
            filtered_lines.push(cleaned);
        }
    }

    filtered_lines.join("\n")
}

fn is_tool_use_line(trimmed_line: &str) -> bool {
    trimmed_line.starts_with("[tool_use")
}

fn is_code_fence_delimiter(trimmed_line: &str) -> bool {
    trimmed_line.starts_with("```")
}

fn is_raw_json_line(trimmed_line: &str, min_chars: usize) -> bool {
    if trimmed_line.len() <= min_chars {
        return false;
    }

    if trimmed_line.starts_with('{') {
        return true;
    }

    if let Some(remainder) = trimmed_line.strip_prefix('[') {
        return is_json_array_payload_start(remainder);
    }

    false
}

fn is_structural_marker_line(trimmed_line: &str) -> bool {
    // `[tool_result]` is also the tool-block terminator. When
    // `tool_call_blocks` is enabled, this branch is never reached because the
    // stateful tool-block handler continues first. It remains here so behavior
    // stays consistent when `tool_call_blocks` is disabled.
    if matches!(trimmed_line, "[user]" | "[assistant]" | "[tool_result]") {
        return true;
    }

    (trimmed_line.len() >= 6 && trimmed_line[..6].eq_ignore_ascii_case("human:"))
        || (trimmed_line.len() >= 10 && trimmed_line[..10].eq_ignore_ascii_case("assistant:"))
        || trimmed_line.eq_ignore_ascii_case("user:")
}

fn is_bash_command_line(trimmed_line: &str) -> bool {
    // Strategy 1: simple prompt prefix (e.g. `$ ls`, `zsh$ git status`)
    if has_prompt_with_command(trimmed_line, "$ ")
        || has_prompt_with_command(trimmed_line, "bash$ ")
        || has_prompt_with_command(trimmed_line, "sh$ ")
        || has_prompt_with_command(trimmed_line, "zsh$ ")
    {
        return true;
    }

    // Strategy 2: user@host:dir$ command (e.g. `user@host:~$ ls -la`)
    if let Some(index) = trimmed_line.find("$ ") {
        let prefix = &trimmed_line[..index];
        if prefix.contains('@') && prefix.contains(':') {
            return has_valid_command_start(trimmed_line[index + "$ ".len()..].chars().next());
        }
    }

    false
}

fn strip_file_path_tokens(line: &str, config: &PrefilterConfig) -> String {
    let mut removed_any = false;
    let kept_tokens = line
        .split_ascii_whitespace()
        .filter(|token| {
            let candidate = trim_token_wrapper_punctuation(token);
            let should_keep = candidate.is_empty() || !is_file_path_token(candidate, config);
            if !should_keep {
                // Side effect: track whether any tokens were removed
                // so we can skip the join allocation if the line is unchanged.
                removed_any = true;
            }
            should_keep
        })
        .collect::<Vec<&str>>();

    if removed_any {
        kept_tokens.join(" ")
    } else {
        line.to_string()
    }
}

fn trim_token_wrapper_punctuation(token: &str) -> &str {
    token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\''
                | '`'
                | ','
                | '.'
                | ';'
                | ':'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
        )
    })
}

fn is_file_path_token(token: &str, config: &PrefilterConfig) -> bool {
    if token.starts_with('/') || token.starts_with("~/") {
        return true;
    }

    if token.starts_with("./") || token.starts_with("../") {
        return true;
    }

    if is_windows_path(token) {
        return true;
    }

    config
        .relative_path_prefixes
        .iter()
        .any(|prefix| token.starts_with(prefix))
}

fn has_prompt_with_command(trimmed_line: &str, prompt_prefix: &str) -> bool {
    if let Some(remainder) = trimmed_line.strip_prefix(prompt_prefix) {
        return has_valid_command_start(remainder.chars().next());
    }

    false
}

fn has_valid_command_start(first: Option<char>) -> bool {
    first.is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | '~' | '-' | '('))
}

fn is_json_array_payload_start(after_open_bracket: &str) -> bool {
    let mut chars = after_open_bracket.chars().peekable();

    while chars.peek().is_some_and(|c| c.is_whitespace()) {
        chars.next();
    }

    chars
        .next()
        .is_some_and(|ch| ch == '{' || ch == '"' || ch == '[' || ch.is_ascii_digit())
}

fn is_windows_path(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn push_blank_line_if_needed(lines: &mut Vec<String>) {
    if lines.last().is_some_and(String::is_empty) {
        return;
    }

    lines.push(String::new());
}

#[cfg(test)]
mod tests {
    use super::{strip_execution_artifacts, FilterToggle, PrefilterConfig};

    #[test]
    fn strips_realistic_precompact_noise_and_keeps_semantic_content() {
        let content = r#"[user]
go ahead and tell me the most relevent context from the last session

[assistant]


[assistant]
[tool_use: Read]
{"file_path":"/home/ausvar/.claude/projects/-home-ausvar/6b3c074f/hook-c1ddfa0d-stdout.txt"}

[user]
[tool_result]

[assistant]


[assistant]
Here's what matters from the last session (which spanned 3 compaction cycles):

**Most recent work (Session 3):**
- Tested cf query with FTS5 search
"#;

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert!(filtered
            .contains("go ahead and tell me the most relevent context from the last session"));
        assert!(filtered.contains("Here's what matters from the last session"));
        assert!(filtered.contains("- Tested cf query with FTS5 search"));

        assert!(!filtered.contains("[tool_use: Read]"));
        assert!(!filtered.contains("[tool_result]"));
        assert!(!filtered.contains("/home/ausvar"));
        assert!(!filtered.contains("[assistant]"));
        assert!(!filtered.contains("[user]"));
    }

    #[test]
    fn strips_code_fences_raw_json_and_shell_prompts() {
        let content = r#"Human: summarize this

```rust
fn main() { println!("noise"); }
```

{"k":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
$ cargo test --workspace
user@host:~$ ls -la
Assistant: all done
Keep this line.
"#;

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert!(!filtered.contains("fn main"));
        assert!(!filtered.contains("cargo test"));
        assert!(!filtered.contains("user@host"));
        assert!(!filtered.contains("\"k\":\"aaaa"));
        assert!(!filtered.contains("Human:"));
        assert!(!filtered.contains("Assistant:"));
        assert!(filtered.contains("Keep this line."));
    }

    #[test]
    fn empty_input_returns_empty_output() {
        assert_eq!(
            strip_execution_artifacts("", &PrefilterConfig::default()),
            ""
        );
    }

    #[test]
    fn clean_content_passthrough_preserves_spacing_and_indentation() {
        let content = "  Keep   spacing\n\tIndented line\nNo artifacts here.";

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert_eq!(filtered, content);
    }

    #[test]
    fn unclosed_code_fence_strips_rest_of_document() {
        let content = "Keep this line\n```rust\nfn main() {}\nstill stripped";

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert_eq!(filtered, "Keep this line\n");
    }

    #[test]
    fn normalizes_crlf_line_endings_to_lf() {
        let content = "Line one\r\nLine two\r\n";

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert_eq!(filtered, "Line one\nLine two");
    }

    #[test]
    fn strips_paths_within_preserved_lines() {
        let content =
            "See /home/ausvar/dev/Projects/context-forge/README.md and crates/analysis/src/lib.rs and C:\\Users\\dev\\notes.txt now.";

        let filtered = strip_execution_artifacts(content, &PrefilterConfig::default());

        assert_eq!(filtered, "See and and now.");
    }

    #[test]
    fn honors_category_toggles() {
        let content = "[tool_use: Read]\n{\"file_path\":\"/home/ausvar/a.txt\"}\n[tool_result]";

        let config = PrefilterConfig {
            tool_call_blocks: FilterToggle::Disabled,
            structural_markers: FilterToggle::Disabled,
            raw_json_lines: FilterToggle::Disabled,
            file_path_literals: FilterToggle::Disabled,
            ..PrefilterConfig::default()
        };

        let filtered = strip_execution_artifacts(content, &config);

        assert!(filtered.contains("[tool_use: Read]"));
        assert!(filtered.contains("[tool_result]"));
        assert!(filtered.contains("/home/ausvar/a.txt"));
    }
}
