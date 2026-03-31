use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::{fs, process};

use clap::{Parser, Subcommand, ValueEnum};

use cf_core::engine::MATCH_ALL_QUERY;
use cf_core::traits::ContextStorage;
use cf_core::{ContextEngine, CoreConfig, EntryKind, EvictionPolicy};
use cf_storage::open_storage;

/// Default maximum entries when not specified by the user.
const DEFAULT_MAX_ENTRIES: usize = 100;

/// Default token budget for assembly.
const DEFAULT_TOKEN_BUDGET: usize = 4096;

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 5000;

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

#[derive(Subcommand)]
enum Command {
    /// Read context from stdin and save a pre-compact snapshot.
    PreCompact {
        /// Path to the SQLite database file.
        #[arg(long)]
        db: PathBuf,
        /// Maximum number of entries to retain.
        #[arg(long, default_value_t = DEFAULT_MAX_ENTRIES)]
        max_entries: usize,
    },
    /// Query context entries from the store.
    Query {
        /// Path to the SQLite database file.
        #[arg(long)]
        db: PathBuf,
        /// Number of entries to return.
        #[arg(long)]
        top_k: usize,
        /// Token budget for assembly.
        #[arg(long, default_value_t = DEFAULT_TOKEN_BUDGET)]
        token_budget: usize,
        /// Output format.
        #[arg(long, default_value = "json")]
        format: OutputFormat,
    },
    /// Delete all entries from the store.
    Clear {
        /// Path to the SQLite database file.
        #[arg(long)]
        db: PathBuf,
    },
    /// Print diagnostics about the store.
    Info {
        /// Path to the SQLite database file.
        #[arg(long)]
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
        Err(_) => {
            eprintln!("error: operation timed out");
            process::exit(2);
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::PreCompact { db, max_entries } => cmd_pre_compact(&db, max_entries),
        Command::Query {
            db,
            top_k,
            token_budget,
            format,
        } => cmd_query(&db, top_k, token_budget, &format),
        Command::Clear { db } => cmd_clear(&db),
        Command::Info { db } => cmd_info(&db),
    }
}

fn cmd_pre_compact(db: &Path, max_entries: usize) -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    let content = input.trim();
    if content.is_empty() {
        return Err("stdin was empty; nothing to save".into());
    }

    let engine = make_engine(db, max_entries, DEFAULT_TOKEN_BUDGET)?;
    let id = engine
        .save_snapshot(content, EntryKind::PreCompact)
        .map_err(|e| e.to_string())?;

    println!("{id}");
    Ok(())
}

fn cmd_query(
    db: &Path,
    top_k: usize,
    token_budget: usize,
    format: &OutputFormat,
) -> Result<(), String> {
    let engine = make_engine(db, DEFAULT_MAX_ENTRIES, token_budget)?;
    let mut entries = engine
        .assemble(MATCH_ALL_QUERY, token_budget)
        .map_err(|e| e.to_string())?;

    entries.truncate(top_k);

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
    let (storage, _) = open_storage(db, DEFAULT_MAX_ENTRIES).map_err(|e| e.to_string())?;
    let n = storage.clear().map_err(|e| e.to_string())?;
    println!("Cleared {n} entries");
    Ok(())
}

fn cmd_info(db: &Path) -> Result<(), String> {
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

fn make_engine(
    db: &Path,
    max_entries: usize,
    token_budget: usize,
) -> Result<ContextEngine, String> {
    let (storage, searcher) = open_storage(db, max_entries).map_err(|e| e.to_string())?;
    let config = CoreConfig {
        max_entries,
        token_budget,
        db_path: db.to_path_buf(),
        eviction_policy: EvictionPolicy::Lru,
    };
    Ok(ContextEngine::new(
        Box::new(storage),
        Box::new(searcher),
        config,
    ))
}
