use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, process};

use clap::{Parser, Subcommand, ValueEnum};

use cf_core::config::DEFAULT_RECENCY_HALF_LIFE_SECS;
use cf_core::engine::MATCH_ALL_QUERY;
use cf_core::traits::ContextStorage;
use cf_core::{ContextEngine, CoreConfig, EntryKind, EvictionPolicy, SaveOptions};
use cf_storage::open_storage;

mod transcript;

/// Default maximum entries when not specified by the user.
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Default token budget for assembly.
const DEFAULT_TOKEN_BUDGET: usize = 16_000;

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// Maximum allowed length for a session_id from stdin JSON.
const MAX_SESSION_ID_LEN: usize = 512;

/// Return the default database path: `~/.context-forge/context.db`.
fn default_db_path() -> PathBuf {
    let base_dir = dirs::home_dir()
        .or_else(dirs::data_dir)
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir);

    base_dir.join(".context-forge").join("context.db")
}

/// context-forge CLI — manage the persistent context store.
#[derive(Parser)]
#[command(name = "cf", version, about)]
struct Cli {
    /// Timeout in milliseconds for the operation.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_MS)]
    timeout_ms: u64,

    #[command(subcommand)]
    command: Command,
}

/// Output format for query results.
#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Text,
}

/// Entry kind selectable from the CLI.
#[derive(Clone, ValueEnum)]
enum CliEntryKind {
    Auto,
    Manual,
    PreCompact,
}

impl From<CliEntryKind> for EntryKind {
    fn from(k: CliEntryKind) -> Self {
        match k {
            CliEntryKind::Auto => EntryKind::Auto,
            CliEntryKind::Manual => EntryKind::Manual,
            CliEntryKind::PreCompact => EntryKind::PreCompact,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Read context from stdin and save a pre-compact snapshot.
    PreCompact {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
        /// Maximum number of entries to retain.
        #[arg(long, default_value_t = DEFAULT_MAX_ENTRIES)]
        max_entries: usize,
    },
    /// Save context from stdin (supports Claude Code PostCompact JSON).
    Save {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
        /// Entry kind to store.
        #[arg(long, default_value = "auto")]
        kind: CliEntryKind,
        /// Maximum number of entries to retain.
        #[arg(long, default_value_t = DEFAULT_MAX_ENTRIES)]
        max_entries: usize,
    },
    /// Query context entries from the store.
    Query {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
        /// Optional search query (FTS5 syntax). Omit for all entries.
        #[arg(long)]
        query: Option<String>,
        /// Number of entries to return.
        #[arg(long)]
        top_k: Option<usize>,
        /// Token budget for assembly.
        #[arg(long)]
        token_budget: Option<usize>,
        /// Output format.
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },
    /// Delete all entries from the store.
    Clear {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
    },
    /// Print diagnostics about the store.
    Info {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let timeout_ms = cli.timeout_ms;

    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result = run(cli);
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(Ok(())) => process::exit(0),
        Ok(Err(e)) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!("error: operation timed out");
            process::exit(2);
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!("error: worker thread panicked");
            process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::PreCompact { db, max_entries } => cmd_pre_compact(&db, max_entries),
        Command::Save {
            db,
            kind,
            max_entries,
        } => cmd_save(&db, kind.into(), max_entries),
        Command::Query {
            db,
            query,
            top_k,
            token_budget,
            format,
        } => cmd_query(&db, query.as_deref(), top_k, token_budget, &format),
        Command::Clear { db } => cmd_clear(&db),
        Command::Info { db } => cmd_info(&db),
    }
}

fn cmd_pre_compact(db: &Path, max_entries: usize) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("stdin was empty; nothing to save".into());
    }

    let parsed_json = serde_json::from_str::<serde_json::Value>(trimmed).ok();
    let session_id = parsed_json
        .as_ref()
        .and_then(|obj| obj.get("session_id"))
        .and_then(|v| v.as_str())
        .filter(|s| s.len() <= MAX_SESSION_ID_LEN)
        .map(str::to_owned);

    // Try to parse stdin as JSON metadata with transcript_path.
    // If present, read and format the JSONL transcript file.
    // Otherwise, fall back to storing stdin verbatim (backward compat).
    let content = if let Some(obj) = parsed_json.as_ref() {
        if let Some(path_str) = obj.get("transcript_path").and_then(|v| v.as_str()) {
            let path_str = path_str.trim();
            if path_str.is_empty() {
                // Empty/whitespace-only transcript_path: treat as missing, fall back to stdin.
                trimmed.to_owned()
            } else {
                let validated_path = validate_transcript_path(Path::new(path_str))?;
                transcript::read_transcript(&validated_path)?
            }
        } else {
            trimmed.to_owned()
        }
    } else {
        trimmed.to_owned()
    };

    if content.is_empty() {
        return Err("no conversation content found in transcript".into());
    }

    ensure_db_dir(db)?;
    let engine = make_engine(
        db,
        max_entries,
        DEFAULT_TOKEN_BUDGET,
        DEFAULT_RECENCY_HALF_LIFE_SECS,
    )?;
    let options = SaveOptions { session_id };
    let id = engine
        .save_snapshot(&content, EntryKind::PreCompact, &options)
        .map_err(|e| e.to_string())?;

    println!("{id}");
    Ok(())
}

/// Save context from stdin. If the input is JSON with a `compact_summary` field
/// (Claude Code PostCompact payload), extract that field. Otherwise save the raw text.
fn cmd_save(db: &Path, kind: EntryKind, max_entries: usize) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("stdin was empty; nothing to save".into());
    }

    let parsed_json = serde_json::from_str::<serde_json::Value>(trimmed).ok();
    let session_id = parsed_json
        .as_ref()
        .and_then(|obj| obj.get("session_id"))
        .and_then(|v| v.as_str())
        .filter(|s| s.len() <= MAX_SESSION_ID_LEN)
        .map(str::to_owned);

    // Try to extract compact_summary from Claude Code PostCompact JSON.
    let content = if let Some(obj) = parsed_json.as_ref() {
        if let Some(summary) = obj.get("compact_summary").and_then(|v| v.as_str()) {
            summary.to_owned()
        } else {
            trimmed.to_owned()
        }
    } else {
        trimmed.to_owned()
    };

    if content.is_empty() {
        return Err("extracted content was empty; nothing to save".into());
    }

    ensure_db_dir(db)?;
    let engine = make_engine(
        db,
        max_entries,
        DEFAULT_TOKEN_BUDGET,
        DEFAULT_RECENCY_HALF_LIFE_SECS,
    )?;
    let options = SaveOptions { session_id };
    let id = engine
        .save_snapshot(&content, kind, &options)
        .map_err(|e| e.to_string())?;

    println!("{id}");
    Ok(())
}

fn cmd_query(
    db: &Path,
    query: Option<&str>,
    top_k: Option<usize>,
    token_budget: Option<usize>,
    format: &OutputFormat,
) -> Result<(), String> {
    ensure_db_dir(db)?;

    let file_cfg = load_config()?;

    // Validate recency_half_life_hours from config file.
    if let Some(h) = file_cfg.recency_half_life_hours {
        if h <= 0.0 || !h.is_finite() {
            return Err(format!(
                "recency_half_life_hours must be a positive finite number, got {h}"
            ));
        }
    }

    // CLI flags > config file > compile-time defaults.
    let effective_budget = token_budget
        .or(file_cfg.token_budget)
        .unwrap_or(DEFAULT_TOKEN_BUDGET);
    let effective_top_k = top_k.or(file_cfg.top_k).unwrap_or(10);
    let recency_half_life_secs = file_cfg
        .recency_half_life_hours
        .map_or(DEFAULT_RECENCY_HALF_LIFE_SECS, |h| h * 3600.0);

    let engine = make_engine(
        db,
        DEFAULT_MAX_ENTRIES,
        effective_budget,
        recency_half_life_secs,
    )?;

    let query_str = match query {
        Some(raw) => preprocess_query(raw),
        None => MATCH_ALL_QUERY.to_owned(),
    };

    let mut entries = engine
        .assemble(&query_str, effective_budget)
        .map_err(|e| e.to_string())?;

    entries.truncate(effective_top_k);

    match format {
        OutputFormat::Json => {
            let json =
                serde_json::to_string_pretty(&entries).map_err(|e| format!("json error: {e}"))?;
            println!("{json}");
        }
        OutputFormat::Text => {
            let texts: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
            println!("{}", texts.join("\n---\n"));
        }
    }

    Ok(())
}

fn cmd_clear(db: &Path) -> Result<(), String> {
    ensure_db_dir(db)?;
    let (storage, _) = open_storage(db, DEFAULT_MAX_ENTRIES).map_err(|e| e.to_string())?;
    let n = storage.clear().map_err(|e| e.to_string())?;
    println!("Cleared {n} entries");
    Ok(())
}

fn cmd_info(db: &Path) -> Result<(), String> {
    ensure_db_dir(db)?;
    let (storage, _) = open_storage(db, DEFAULT_MAX_ENTRIES).map_err(|e| e.to_string())?;
    let count = storage.count().map_err(|e| e.to_string())?;
    let version = storage.schema_version().map_err(|e| e.to_string())?;

    let size = fs::metadata(db)
        .map(|m| m.len())
        .map_err(|e| format!("failed to read db metadata: {e}"))?;

    println!("entries:  {count}");
    println!("schema:   v{version}");
    println!("db size:  {size} bytes");
    println!("db path:  {}", db.display());
    Ok(())
}

/// Validate that a transcript path is safe to read.
///
/// Claude Code transcripts are JSONL files stored under the user's home directory.
/// This prevents path traversal attacks where a crafted `transcript_path` could
/// read arbitrary files.
fn validate_transcript_path(path: &Path) -> Result<PathBuf, String> {
    // Canonicalize resolves symlinks and `..` components.
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("invalid transcript path {}: {e}", path.display()))?;

    // Must be a .jsonl file.
    match canonical.extension().and_then(|e| e.to_str()) {
        Some("jsonl") => {}
        _ => {
            return Err(format!(
                "transcript path must be a .jsonl file: {}",
                canonical.display()
            ))
        }
    }

    // Must be under the user's home directory.
    let home = dirs::home_dir()
        .ok_or_else(|| "cannot determine home directory for path validation".to_string())?;
    if !canonical.starts_with(&home) {
        return Err(format!(
            "transcript path must be under home directory: {}",
            canonical.display()
        ));
    }

    Ok(canonical)
}

/// Ensure the parent directory of the database file exists.
fn ensure_db_dir(db: &Path) -> Result<(), String> {
    if let Some(parent) = db.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create db directory {}: {e}", parent.display()))?;
        }
    }
    Ok(())
}

/// Expand a natural-language query for FTS5.
///
/// Multi-word queries without explicit FTS5 operators (AND, OR, NOT, NEAR)
/// are split into individual terms joined with OR for broader recall.
/// Single words and queries already using FTS5 syntax pass through unchanged.
///
/// Tokens containing FTS5 operator characters (hyphens, leading `+`) are
/// automatically wrapped in double quotes so FTS5 treats them as literals
/// rather than as boolean operators.
fn preprocess_query(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return MATCH_ALL_QUERY.to_owned();
    }
    // Pass through if the query already contains quoted phrases.
    if trimmed.contains('"') {
        return trimmed.to_owned();
    }
    // Check for whole-word FTS5 operators (case-insensitive).
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let has_operator = words.iter().any(|w| {
        let upper = w.to_uppercase();
        matches!(upper.as_str(), "AND" | "OR" | "NOT" | "NEAR")
    });
    if has_operator {
        return trimmed.to_owned();
    }
    // Quote tokens that contain FTS5 operator characters to prevent
    // misinterpretation (e.g. "release-please" → `"release-please"`
    // instead of `release NOT please`).
    let sanitized: Vec<String> = words.iter().map(|w| quote_fts5_token(w)).collect();
    if sanitized.len() <= 1 {
        return sanitized.into_iter().next().unwrap_or_default();
    }
    sanitized.join(" OR ")
}

/// Wrap a token in double quotes if it contains characters that FTS5
/// would interpret as operators (`-` anywhere, `+` as prefix).
fn quote_fts5_token(token: &str) -> String {
    if token.contains('-') || token.starts_with('+') {
        format!("\"{token}\"")
    } else {
        token.to_owned()
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct FileConfig {
    token_budget: Option<usize>,
    top_k: Option<usize>,
    recency_half_life_hours: Option<f64>,
}

/// Load optional config from `~/.context-forge/config.toml`.
///
/// Returns the default config if the file does not exist.
fn load_config() -> Result<FileConfig, String> {
    let base_dir = dirs::home_dir()
        .or_else(dirs::data_dir)
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir);
    let config_path = base_dir.join(".context-forge").join("config.toml");
    if !config_path.exists() {
        return Ok(FileConfig::default());
    }
    let contents = fs::read_to_string(&config_path)
        .map_err(|e| format!("failed to read {}: {e}", config_path.display()))?;
    toml::from_str(&contents).map_err(|e| format!("invalid TOML in {}: {e}", config_path.display()))
}

fn make_engine(
    db: &Path,
    max_entries: usize,
    token_budget: usize,
    recency_half_life_secs: f64,
) -> Result<ContextEngine, String> {
    let (storage, searcher) = open_storage(db, max_entries).map_err(|e| e.to_string())?;
    let config = CoreConfig {
        max_entries,
        token_budget,
        db_path: db.to_path_buf(),
        eviction_policy: EvictionPolicy::Lru,
        recency_half_life_secs,
    };
    Ok(ContextEngine::new(
        Box::new(storage),
        Box::new(searcher),
        config,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_preprocess_empty() {
        assert_eq!(preprocess_query(""), MATCH_ALL_QUERY);
        assert_eq!(preprocess_query("   "), MATCH_ALL_QUERY);
    }

    #[test]
    fn test_preprocess_single_word() {
        assert_eq!(preprocess_query("security"), "security");
    }

    #[test]
    fn test_preprocess_multi_word_or_expansion() {
        assert_eq!(
            preprocess_query("security hardening"),
            "security OR hardening"
        );
        assert_eq!(preprocess_query("memory leak fix"), "memory OR leak OR fix");
    }

    #[test]
    fn test_preprocess_explicit_operators_passthrough() {
        assert_eq!(
            preprocess_query("security AND hardening"),
            "security AND hardening"
        );
        assert_eq!(
            preprocess_query("security OR hardening"),
            "security OR hardening"
        );
        assert_eq!(preprocess_query("NOT deprecated"), "NOT deprecated");
    }

    #[test]
    fn test_preprocess_operators_case_insensitive() {
        assert_eq!(
            preprocess_query("security and hardening"),
            "security and hardening"
        );
        assert_eq!(
            preprocess_query("security or hardening"),
            "security or hardening"
        );
    }

    #[test]
    fn test_preprocess_quoted_passthrough() {
        assert_eq!(preprocess_query("\"exact phrase\""), "\"exact phrase\"");
    }

    #[test]
    fn test_preprocess_operator_substring_no_false_positive() {
        // "MEMORY" contains "OR" as substring but should NOT trigger passthrough
        assert_eq!(preprocess_query("MEMORY leak"), "MEMORY OR leak");
        // "ANDROID" contains "AND" as substring
        assert_eq!(preprocess_query("ANDROID setup"), "ANDROID OR setup");
    }

    #[test]
    fn test_preprocess_hyphenated_single_word_quoted() {
        // Hyphens are FTS5 NOT operators — must be quoted to stay literal.
        assert_eq!(preprocess_query("release-please"), "\"release-please\"");
        assert_eq!(preprocess_query("pre-compact"), "\"pre-compact\"");
        assert_eq!(preprocess_query("context-forge"), "\"context-forge\"");
    }

    #[test]
    fn test_preprocess_hyphenated_multi_word() {
        // Hyphenated tokens quoted; plain tokens left alone; joined with OR.
        assert_eq!(
            preprocess_query("pre-compact hook"),
            "\"pre-compact\" OR hook"
        );
        assert_eq!(
            preprocess_query("context-forge token-budget"),
            "\"context-forge\" OR \"token-budget\""
        );
    }

    #[test]
    fn test_preprocess_plus_prefix_quoted() {
        // Leading '+' is FTS5 required-term operator — must be quoted.
        assert_eq!(preprocess_query("+foo"), "\"+foo\"");
        assert_eq!(preprocess_query("+foo bar"), "\"+foo\" OR bar");
    }

    #[test]
    fn test_preprocess_plain_words_unchanged() {
        // No special chars — should behave exactly as before.
        assert_eq!(preprocess_query("security"), "security");
        assert_eq!(
            preprocess_query("security hardening"),
            "security OR hardening"
        );
    }

    #[test]
    fn test_validate_transcript_path_rejects_non_jsonl() {
        // Create a temp file with a .txt extension — must be rejected.
        let dir = tempfile::tempdir().unwrap();
        let bad_file = dir.path().join("transcript.txt");
        std::fs::write(&bad_file, "content").unwrap();

        let result = validate_transcript_path(&bad_file);
        assert!(result.is_err());
        assert!(
            result.as_ref().unwrap_err().contains(".jsonl"),
            "error should mention .jsonl: {:?}",
            result
        );
    }

    #[test]
    fn test_validate_transcript_path_rejects_outside_home() {
        // A .jsonl file in a temp dir (outside $HOME) should be rejected.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("transcript.jsonl");
        std::fs::write(&file, "content").unwrap();

        let result = validate_transcript_path(&file);
        // On most systems temp dirs are outside $HOME, so this should fail.
        // If $HOME happens to contain the temp dir, this test is a no-op.
        let home = dirs::home_dir().unwrap();
        let canonical = file.canonicalize().unwrap();
        if !canonical.starts_with(&home) {
            assert!(result.is_err());
            assert!(
                result.as_ref().unwrap_err().contains("home directory"),
                "error should mention home directory: {:?}",
                result
            );
        }
    }

    #[test]
    fn test_validate_transcript_path_rejects_nonexistent() {
        let result = validate_transcript_path(Path::new("/nonexistent/transcript.jsonl"));
        assert!(result.is_err());
    }
}
