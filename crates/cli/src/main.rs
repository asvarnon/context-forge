use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, process};

use clap::{Parser, Subcommand, ValueEnum};

use cf_analysis::{
    adjust_weights, build_session_term_maps, classify_passages, compute_recurrence,
    extract_passages, pack_segments, scale_budget, score_passages, strip_execution_artifacts,
    ClassificationConfig, ExtractionConfig, ExtractionEntry, ImportanceCategory, ImportanceSegment,
    InjectionConfig, Lexicons, PassageContext, PrefilterConfig, RecurrenceConfig, ScoringConfig,
    Tokenizer, TokenizerConfig,
};
use cf_core::config::DEFAULT_RECENCY_HALF_LIFE_SECS;
use cf_core::engine::MATCH_ALL_QUERY;
use cf_core::session::group_entries_by_session;
use cf_core::traits::ContextStorage;
use cf_core::{ContextEngine, ContextEntry, CoreConfig, EntryKind, EvictionPolicy, SaveOptions};
use cf_storage::open_storage;

mod transcript;

/// Default maximum entries when not specified by the user.
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Default token budget for assembly.
const DEFAULT_TOKEN_BUDGET: usize = 16_000;

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// Default token budget for the importance injection block.
const DEFAULT_IMPORTANCE_BUDGET: usize = 512;

/// Header for the importance context block in text output.
const IMPORTANCE_HEADER: &str = "=== Important Context ===";

/// Separator between output sections in text format.
const SECTION_SEPARATOR: &str = "---";

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

#[derive(Clone, ValueEnum)]
/// Session start trigger source for importance injection strategy.
enum QuerySource {
    /// First launch - broad importance context with default scoring weights.
    Startup,
    /// Resumed session - same as startup (broad context).
    Resume,
    /// Post-compaction - progressive injection with scaled budget and adjusted weights.
    Compact,
    /// Post-clear - skip importance injection entirely (fresh start).
    Clear,
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
        /// SessionStart trigger source. Controls importance injection strategy.
        #[arg(long, value_enum)]
        source: Option<QuerySource>,
        /// Fixed token ceiling for the importance injection block. Default: 512 tokens.
        #[arg(long, default_value_t = DEFAULT_IMPORTANCE_BUDGET)]
        importance_budget: usize,
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
    /// Run the importance-detection pipeline and output ranked segments.
    Analyze {
        /// Path to the SQLite database file.
        #[arg(long, default_value_os_t = default_db_path())]
        db: PathBuf,
        /// Maximum number of segments to return.
        #[arg(long, default_value_t = 20)]
        top_k: usize,
        /// Token budget for segment packing.
        #[arg(long, default_value_t = 2048)]
        token_budget: usize,
        /// Output format.
        #[arg(long, default_value = "text")]
        format: OutputFormat,
        /// Include pipeline statistics in output.
        #[arg(long)]
        stats: bool,
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
            source,
            importance_budget,
        } => cmd_query(
            &db,
            query.as_deref(),
            top_k,
            token_budget,
            &format,
            source.as_ref(),
            importance_budget,
        ),
        Command::Clear { db } => cmd_clear(&db),
        Command::Info { db } => cmd_info(&db),
        Command::Analyze {
            db,
            top_k,
            token_budget,
            format,
            stats,
        } => cmd_analyze(&db, top_k, token_budget, &format, stats),
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
    source: Option<&QuerySource>,
    importance_budget: usize,
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

    let should_inject_importance = match source {
        None | Some(QuerySource::Clear) => false,
        Some(QuerySource::Startup) | Some(QuerySource::Resume) | Some(QuerySource::Compact) => {
            importance_budget > 0
        }
    };

    let bm25_budget = if should_inject_importance {
        effective_budget.saturating_sub(importance_budget.min(effective_budget))
    } else {
        effective_budget
    };

    let importance_segments = if should_inject_importance {
        let (storage, _) = open_storage(db, DEFAULT_MAX_ENTRIES).map_err(|e| e.to_string())?;
        let all_entries = storage.get_all().map_err(|e| e.to_string())?;

        if matches!(source, Some(QuerySource::Compact)) {
            let max_compaction_count = all_entries.iter().filter_map(|e| e.compaction_count).max();
            let effective_importance_budget = scale_budget(
                importance_budget,
                max_compaction_count,
                &InjectionConfig::default(),
            );
            let scoring_config = adjust_weights(&ScoringConfig::default(), max_compaction_count);
            run_importance_pipeline(&all_entries, &scoring_config, effective_importance_budget)
        } else {
            run_importance_pipeline(&all_entries, &ScoringConfig::default(), importance_budget)
        }
    } else {
        Vec::new()
    };

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
        .assemble(&query_str, bm25_budget)
        .map_err(|e| e.to_string())?;

    entries.truncate(effective_top_k);

    match format {
        OutputFormat::Json => {
            if should_inject_importance {
                let importance_json: Vec<serde_json::Value> = importance_segments
                    .iter()
                    .map(|segment| {
                        serde_json::json!({
                            "text": &segment.text,
                            "categories": segment
                                .categories
                                .iter()
                                .map(category_title_case)
                                .collect::<Vec<_>>(),
                            "importance_score": segment.importance_score,
                            "session_frequency": segment.session_frequency,
                            "triggering_terms": &segment.triggering_terms,
                            "session_id": &segment.session_id,
                            "timestamp": segment.timestamp,
                            "estimated_tokens": segment.token_estimate,
                        })
                    })
                    .collect();

                let output = serde_json::json!({
                    "version": 2,
                    "importance": importance_json,
                    "bm25": entries,
                });

                let json = serde_json::to_string_pretty(&output)
                    .map_err(|e| format!("json error: {e}"))?;
                println!("{json}");
            } else {
                let json = serde_json::to_string_pretty(&entries)
                    .map_err(|e| format!("json error: {e}"))?;
                println!("{json}");
            }
        }
        OutputFormat::Text => {
            if importance_segments.is_empty() {
                let texts: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
                println!("{}", texts.join(&format!("\n{SECTION_SEPARATOR}\n")));
            } else {
                println!("{IMPORTANCE_HEADER}\n");
                for (idx, segment) in importance_segments.iter().enumerate() {
                    println!(
                        "[{}] (recurring across {} sessions)",
                        top_category_label(&segment.categories),
                        segment.session_frequency
                    );
                    println!("{}", segment.text);
                    if idx + 1 < importance_segments.len() {
                        println!();
                    }
                }

                if !entries.is_empty() {
                    let texts: Vec<&str> = entries.iter().map(|e| e.content.as_str()).collect();
                    println!("\n{SECTION_SEPARATOR}");
                    println!("{}", texts.join(&format!("\n{SECTION_SEPARATOR}\n")));
                }
            }
        }
    }

    Ok(())
}

fn category_title_case(category: &ImportanceCategory) -> &'static str {
    match category {
        ImportanceCategory::Corrective => "Corrective",
        ImportanceCategory::Decisive => "Decisive",
        ImportanceCategory::Stateful => "Stateful",
        ImportanceCategory::Reinforcing => "Reinforcing",
        _ => "Uncategorized",
    }
}

fn top_category_label(categories: &[ImportanceCategory]) -> &'static str {
    categories
        .iter()
        .map(|category| match category {
            ImportanceCategory::Corrective => (4, "CORRECTIVE"),
            ImportanceCategory::Decisive => (3, "DECISIVE"),
            ImportanceCategory::Stateful => (2, "STATEFUL"),
            ImportanceCategory::Reinforcing => (1, "REINFORCING"),
            _ => (0, "UNCATEGORIZED"),
        })
        .max_by_key(|(priority, _)| *priority)
        .map_or("UNCATEGORIZED", |(_, label)| label)
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

fn cmd_analyze(
    db: &Path,
    top_k: usize,
    token_budget: usize,
    format: &OutputFormat,
    stats: bool,
) -> Result<(), String> {
    ensure_db_dir(db)?;
    let (storage, _) = open_storage(db, DEFAULT_MAX_ENTRIES).map_err(|e| e.to_string())?;

    // Step 1: Load all entries
    let entries = storage.get_all().map_err(|e| e.to_string())?;
    if entries.is_empty() {
        match format {
            OutputFormat::Json => {
                let stats_value = if stats {
                    serde_json::json!({
                        "entry_count": 0,
                        "session_count": 0,
                        "high_recurrence_terms": 0,
                        "passages_extracted": 0,
                        "segments_scored": 0,
                        "segments_packed": 0,
                        "compaction_depth": null,
                        "effective_budget": token_budget,
                        "budget_scale_factor": 1.0,
                    })
                } else {
                    serde_json::Value::Null
                };
                let output = serde_json::json!({"segments": [], "stats": stats_value});
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output)
                        .map_err(|e| format!("json error: {e}"))?
                );
            }
            OutputFormat::Text => {
                println!("No entries in database.");
                if stats {
                    println!("\n--- Stats ---");
                    println!("Entries:             0");
                    println!("Compaction depth:    N/A");
                    println!("Effective budget:    {token_budget}");
                    println!("Budget scale factor: 1.00");
                }
            }
        }
        return Ok(());
    }
    let entry_count = entries.len();

    // Step 2: Pre-filter
    let prefilter_config = PrefilterConfig::default();
    let filtered: Vec<(String, String)> = entries
        .iter()
        .map(|e| {
            let clean = strip_execution_artifacts(&e.content, &prefilter_config);
            (e.id.clone(), clean)
        })
        .filter(|(_, content)| !content.trim().is_empty())
        .collect();
    let filtered_count = filtered.len();

    // Step 3: Initialize tokenizer (used by Step 5)
    let tokenizer = Tokenizer::new(&TokenizerConfig::default());

    // Step 4: Group entries by session
    let session_groups = group_entries_by_session(&entries, 180);
    let session_count = session_groups.len();

    // Step 5: Build per-session term count maps for recurrence
    let session_contents: Vec<Vec<&str>> = session_groups
        .iter()
        .map(|group| group.entries.iter().map(|e| e.content.as_str()).collect())
        .collect();
    let session_term_maps =
        build_session_term_maps(&session_contents, &tokenizer, &prefilter_config);

    // Step 6: Compute recurrence
    let recurrence_config = RecurrenceConfig::default();
    let recurrence_results = compute_recurrence(&session_term_maps, &recurrence_config);
    let recurrence_term_count = recurrence_results.len();

    if recurrence_results.is_empty() {
        match format {
            OutputFormat::Json => {
                let stats_value = if stats {
                    serde_json::json!({
                        "entry_count": entry_count,
                        "filtered_entries": filtered_count,
                        "session_count": session_count,
                        "high_recurrence_terms": 0,
                        "passages_extracted": 0,
                        "segments_scored": 0,
                        "segments_packed": 0,
                        "compaction_depth": null,
                        "effective_budget": token_budget,
                        "budget_scale_factor": 1.0,
                    })
                } else {
                    serde_json::Value::Null
                };
                let output = serde_json::json!({
                    "segments": [],
                    "stats": stats_value,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output)
                        .map_err(|e| format!("json error: {e}"))?
                );
            }
            OutputFormat::Text => {
                println!("No high-recurrence terms found across {session_count} sessions.");
                if stats {
                    println!("\n--- Stats ---");
                    println!("Entries:              {entry_count}");
                    println!("Filtered entries:    {filtered_count}");
                    println!("Sessions:             {session_count}");
                    println!("High-recurrence terms: 0");
                    println!("Compaction depth:    N/A");
                    println!("Effective budget:    {token_budget}");
                    println!("Budget scale factor: 1.00");
                }
            }
        }
        return Ok(());
    }

    // Build recurrence map for scoring
    let recurrence_map: HashMap<String, cf_analysis::RecurrenceResult> = recurrence_results
        .into_iter()
        .map(|r| (r.term.clone(), r))
        .collect();

    let high_recurrence_terms: Vec<String> = recurrence_map.keys().cloned().collect();

    // Step 7: Extract passages
    let extraction_entries: Vec<ExtractionEntry> = filtered
        .iter()
        .map(|(id, content)| ExtractionEntry {
            entry_id: id.clone(),
            content: content.clone(),
        })
        .collect();
    let extraction_config = ExtractionConfig::default();
    let passages = extract_passages(
        &extraction_entries,
        &high_recurrence_terms,
        &extraction_config,
    );
    let passage_count = passages.len();

    if passages.is_empty() {
        match format {
            OutputFormat::Json => {
                let stats_value = if stats {
                    serde_json::json!({
                        "entry_count": entry_count,
                        "filtered_entries": filtered_count,
                        "session_count": session_count,
                        "high_recurrence_terms": recurrence_term_count,
                        "passages_extracted": 0,
                        "segments_scored": 0,
                        "segments_packed": 0,
                        "compaction_depth": null,
                        "effective_budget": token_budget,
                        "budget_scale_factor": 1.0,
                    })
                } else {
                    serde_json::Value::Null
                };
                let output = serde_json::json!({
                    "segments": [],
                    "stats": stats_value,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output)
                        .map_err(|e| format!("json error: {e}"))?
                );
            }
            OutputFormat::Text => {
                println!("No passages extracted from {filtered_count} entries.");
                if stats {
                    println!("\n--- Stats ---");
                    println!("Entries:              {entry_count}");
                    println!("Filtered entries:    {filtered_count}");
                    println!("Sessions:             {session_count}");
                    println!("High-recurrence terms: {recurrence_term_count}");
                    println!("Passages extracted:   0");
                    println!("Compaction depth:    N/A");
                    println!("Effective budget:    {token_budget}");
                    println!("Budget scale factor: 1.00");
                }
            }
        }
        return Ok(());
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system time error: {e}"))?
        .as_secs();
    #[allow(clippy::cast_possible_wrap)]
    let now_timestamp = now as i64;

    // Step 8: Build PassageContext for classification
    // Map entry_id -> (session_id, timestamp) for bridging
    let entry_session_map: HashMap<String, (String, i64)> = session_groups
        .iter()
        .flat_map(|group| {
            group
                .entries
                .iter()
                .map(move |e| (e.id.clone(), (group.session_id.clone(), e.timestamp)))
        })
        .collect();

    let passage_contexts: Vec<PassageContext> = passages
        .iter()
        .map(|p| {
            let (session_id, timestamp) = entry_session_map
                .get(&p.source_entry_id)
                .cloned()
                .unwrap_or_else(|| {
                    debug_assert!(
                        false,
                        "entry_session_map missing entry: {}",
                        p.source_entry_id
                    );
                    ("unknown".to_string(), now_timestamp)
                });
            PassageContext {
                passage_text: p.text.clone(),
                triggering_terms: p.triggering_terms.clone(),
                session_id,
                timestamp,
            }
        })
        .collect();

    // Step 9: Classify
    let lexicons = Lexicons::default();
    let classification_config = ClassificationConfig::default();
    let classified = classify_passages(&passage_contexts, &lexicons, &classification_config);

    // Step 10: Progressive injection — adjust weights and budget
    let max_compaction_count: Option<i64> = entries.iter().filter_map(|e| e.compaction_count).max();

    let injection_config = InjectionConfig::default();
    let effective_budget = scale_budget(token_budget, max_compaction_count, &injection_config);

    let scoring_config = ScoringConfig::default();
    let adjusted_scoring_config = adjust_weights(&scoring_config, max_compaction_count);

    // Step 11: Score with adjusted weights
    let segments = score_passages(
        &classified,
        &recurrence_map,
        &adjusted_scoring_config,
        now_timestamp,
    );
    let scored_count = segments.len();

    // Step 12: Pack into effective budget
    let packed = pack_segments(&segments, effective_budget);
    let packed_count = packed.len();

    // Apply top_k limit
    let final_segments: Vec<&cf_analysis::ImportanceSegment> = packed.iter().take(top_k).collect();

    // Output
    match format {
        OutputFormat::Json => {
            let segments_json: Vec<serde_json::Value> = final_segments
                .iter()
                .enumerate()
                .map(|(i, seg)| {
                    serde_json::json!({
                        "rank": i + 1,
                        "text": &seg.text,
                        "importance_score": seg.importance_score,
                        "recurrence_score": seg.recurrence_score,
                        "category_weight": seg.category_weight,
                        "recency_factor": seg.recency_factor,
                        "categories": seg.categories.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>(),
                        "triggering_terms": &seg.triggering_terms,
                        "session_id": &seg.session_id,
                        "token_estimate": seg.token_estimate,
                    })
                })
                .collect();

            let stats_value = if stats {
                serde_json::json!({
                    "entry_count": entry_count,
                    "filtered_entries": filtered_count,
                    "session_count": session_count,
                    "high_recurrence_terms": recurrence_term_count,
                    "passages_extracted": passage_count,
                    "segments_scored": scored_count,
                    "segments_packed": packed_count,
                    "compaction_depth": max_compaction_count,
                    "effective_budget": effective_budget,
                    "budget_scale_factor": if token_budget == 0 {
                        1.0
                    } else {
                        effective_budget as f64 / token_budget as f64
                    },
                })
            } else {
                serde_json::Value::Null
            };

            let output = serde_json::json!({
                "segments": segments_json,
                "stats": stats_value,
            });

            let json =
                serde_json::to_string_pretty(&output).map_err(|e| format!("json error: {e}"))?;
            println!("{json}");
        }
        OutputFormat::Text => {
            for (i, seg) in final_segments.iter().enumerate() {
                let categories: Vec<String> =
                    seg.categories.iter().map(|c| format!("{c:?}")).collect();
                let cat_str = if categories.is_empty() {
                    "Uncategorized".to_string()
                } else {
                    categories.join(", ")
                };

                println!(
                    "#{} [score: {:.4}] [{}]",
                    i + 1,
                    seg.importance_score,
                    cat_str
                );
                println!(
                    "  Session: {} | Tokens: ~{}",
                    seg.session_id, seg.token_estimate
                );
                println!("  Terms: {}", seg.triggering_terms.join(", "));
                println!("  {}", seg.text.replace('\n', "\n  "));
                if i < final_segments.len() - 1 {
                    println!("---");
                }
            }

            if stats {
                println!("\n--- Stats ---");
                println!("Entries:              {entry_count}");
                println!("Filtered entries:    {filtered_count}");
                println!("Sessions:             {session_count}");
                println!("High-recurrence terms: {recurrence_term_count}");
                println!("Passages extracted:   {passage_count}");
                println!("Segments scored:      {scored_count}");
                println!("Segments packed:      {packed_count}");
                println!(
                    "Compaction depth:    {}",
                    max_compaction_count.map_or("N/A".to_string(), |c| c.to_string())
                );
                println!("Effective budget:    {effective_budget}");
                println!(
                    "Budget scale factor: {:.2}",
                    if token_budget == 0 {
                        1.0
                    } else {
                        effective_budget as f64 / token_budget as f64
                    }
                );
                println!("Segments returned:    {}", final_segments.len());
            }
        }
    }

    Ok(())
}

fn run_importance_pipeline(
    entries: &[ContextEntry],
    scoring_config: &ScoringConfig,
    importance_budget: usize,
) -> Vec<ImportanceSegment> {
    if entries.is_empty() {
        return Vec::new();
    }

    // Pre-filter execution artifacts
    let prefilter_config = PrefilterConfig::default();
    let filtered: Vec<(String, String)> = entries
        .iter()
        .map(|e| {
            let clean = strip_execution_artifacts(&e.content, &prefilter_config);
            (e.id.clone(), clean)
        })
        .filter(|(_, content)| !content.trim().is_empty())
        .collect();

    // Initialize tokenizer
    let tokenizer = Tokenizer::new(&TokenizerConfig::default());

    // Group entries by session
    let session_groups = group_entries_by_session(entries, 180);

    // Build per-session term maps
    let session_contents: Vec<Vec<&str>> = session_groups
        .iter()
        .map(|group| group.entries.iter().map(|e| e.content.as_str()).collect())
        .collect();
    let session_term_maps =
        build_session_term_maps(&session_contents, &tokenizer, &prefilter_config);

    // Compute cross-session recurrence
    let recurrence_config = RecurrenceConfig::default();
    let recurrence_results = compute_recurrence(&session_term_maps, &recurrence_config);
    if recurrence_results.is_empty() {
        return Vec::new();
    }

    let recurrence_map: HashMap<String, cf_analysis::RecurrenceResult> = recurrence_results
        .into_iter()
        .map(|r| (r.term.clone(), r))
        .collect();
    let high_recurrence_terms: Vec<String> = recurrence_map.keys().cloned().collect();

    // Extract passages around recurrence terms
    let extraction_entries: Vec<ExtractionEntry> = filtered
        .iter()
        .map(|(id, content)| ExtractionEntry {
            entry_id: id.clone(),
            content: content.clone(),
        })
        .collect();
    let extraction_config = ExtractionConfig::default();
    let passages = extract_passages(
        &extraction_entries,
        &high_recurrence_terms,
        &extraction_config,
    );
    if passages.is_empty() {
        return Vec::new();
    }

    let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(err) => {
            eprintln!("failed to compute current system time for importance pipeline: {err}");
            return Vec::new();
        }
    };
    #[allow(clippy::cast_possible_wrap)]
    let now_timestamp = now as i64;

    // Build passage contexts for classification
    let entry_session_map: HashMap<String, (String, i64)> = session_groups
        .iter()
        .flat_map(|group| {
            group
                .entries
                .iter()
                .map(move |e| (e.id.clone(), (group.session_id.clone(), e.timestamp)))
        })
        .collect();

    let passage_contexts: Vec<PassageContext> = passages
        .iter()
        .map(|p| {
            let (session_id, timestamp) = entry_session_map
                .get(&p.source_entry_id)
                .cloned()
                .unwrap_or_else(|| {
                    debug_assert!(
                        false,
                        "entry_session_map missing entry: {}",
                        p.source_entry_id
                    );
                    ("unknown".to_string(), now_timestamp)
                });
            PassageContext {
                passage_text: p.text.clone(),
                triggering_terms: p.triggering_terms.clone(),
                session_id,
                timestamp,
            }
        })
        .collect();

    // Classify passages into importance categories
    let lexicons = Lexicons::default();
    let classification_config = ClassificationConfig::default();
    let classified = classify_passages(&passage_contexts, &lexicons, &classification_config);

    // Score classified passages
    let segments = score_passages(&classified, &recurrence_map, scoring_config, now_timestamp);

    // Pack into token budget
    pack_segments(&segments, importance_budget)
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
