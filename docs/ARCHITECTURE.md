# Architecture

Internal developer reference for Context Forge. For setup instructions, see [claude-code-setup.md](claude-code-setup.md).

## Data Pipeline

Context Forge is a **passthrough storage engine** — it does not summarize, compress, or transform content. Understanding this is critical to understanding the system.

### `cf pre-compact` (PreCompact Hook)

Claude Code fires this **before** compaction, piping a JSON metadata payload to stdin containing a `transcript_path` field that points to the session's JSONL transcript file.

1. Read stdin, parse as JSON
2. Extract `transcript_path` from the metadata
3. Read the JSONL transcript file at that path
4. Convert JSONL turns into BM25-friendly plain text (see below)
5. Generate ID via FNV-1a hash of `content + timestamp`
6. Store formatted transcript with `kind=PreCompact`

**Backward compatibility:** If stdin is not JSON, or JSON without `transcript_path`, stores stdin verbatim (supports shell wrapper scripts that pipe transcript content directly).

**JSONL → Plain Text Conversion** (`crates/cli/src/transcript.rs`):
- Filters for `user` and `assistant` turn types only (drops `system`, `file-history-snapshot`, `queue-operation`, `last-prompt`)
- Formats content blocks: `text` → plain text, `thinking` → reasoning text (drops encrypted `signature`), `tool_use` → tool name + compact JSON input, `tool_result` → tool output text
- Tool results are preserved — they contain the most valuable recoverable context (file contents, command outputs, search results)
- Malformed JSONL lines are skipped with a stderr warning

**Code:** `cmd_pre_compact()` in `crates/cli/src/main.rs`, `transcript::read_transcript()` in `crates/cli/src/transcript.rs`

### `cf save --kind auto` (PostCompact Hook)

Claude Code fires this **after** compaction, piping a JSON payload to stdin that includes a `compact_summary` field.

1. Read stdin
2. Parse as JSON, extract the `compact_summary` string
3. Store the summary with `kind=Auto`
4. If JSON parse fails or the field is missing → store raw stdin as fallback

**The summarization is done by Claude Code, not by Context Forge.** The `compact_summary` is Claude's own summary of the compacted conversation. Context Forge just stores it.

**Code:** `cmd_save()` in `crates/cli/src/main.rs`

### `cf query --format text` (SessionStart Hook)

Fires when a new Claude Code session starts.

1. Load optional config from `~/.context-forge/config.toml` (see Config File below)
2. Preprocess query string: multi-word queries without FTS5 operators (AND, OR, NOT, NEAR, quotes) are auto-expanded with OR — e.g. `security hardening` → `security OR hardening`. Explicit FTS5 syntax passes through unchanged. This preprocessing happens in the CLI layer (`crates/cli/`).
3. Call `engine.assemble(query, token_budget)` (query defaults to `"*"` when `--query` is omitted)
   - FTS5 search
   - Recency decay weighting (exponential, configurable half-life — default 72 hours)
   - Sort by weighted score descending
   - Greedy token budget packing (skips oversized entries)
4. Output entry contents verbatim, joined with `\n---\n`

No summarization on output.

**Code:** `cmd_query()` in `crates/cli/src/main.rs`

### Why Output Appears "Already Summarized"

The PostCompact entry stores Claude's own `compact_summary` — which IS a summary. When that entry is assembled into a new session, it reads like summarized content. But Context Forge didn't summarize it — Claude Code did, and Context Forge just stored and retrieved it.

## Core Engine

`crates/core/src/engine.rs`

`ContextEngine` wires together a `ContextStorage` implementation and a `Searcher` implementation.

### Key Methods

**`save_snapshot(content, kind)`**
- Generates entry ID via FNV-1a hash of `content + timestamp`
- Estimates token count: `1 token ≈ 4 chars`
- Delegates to `ContextStorage::save()`

**`assemble(query, token_budget)`**
1. `Searcher::search(query, DEFAULT_SEARCH_LIMIT)` — returns scored results
2. Apply recency weighting to each result's score
3. Sort by weighted score descending
4. Greedy budget packing — iterate sorted results, accumulate token counts, skip any entry that would exceed the remaining budget

**`recency_decay(age_seconds, half_life)`**
- Exponential decay: `0.5^(age / half_life)`
- Default half-life: 72 hours (259200 seconds)
- Configurable via `CoreConfig.recency_half_life_secs` (set from config file)
- Recent entries score higher; old entries fade but never reach zero

### Constants

```rust
DEFAULT_SEARCH_LIMIT: usize = 50;
DEFAULT_RECENCY_HALF_LIFE_HOURS: f64 = 72.0;  // in config.rs, converted to seconds at call sites
DEFAULT_RECENCY_HALF_LIFE_SECS: f64 = 259200.0;  // computed from HOURS * 3600.0 in config.rs
// Per-frontend defaults for max entries:
// - CLI: 100 (see crates/cli/src/main.rs)
// - napi (Node bindings): 1000 (constructor default)
DEFAULT_TOKEN_BUDGET: usize = 16000;
```

## Storage Layer

`crates/storage/`

`SqliteStorage` implements `ContextStorage`. `SqliteSearcher` implements `Searcher`. Both share a connection pool via `Arc<r2d2::Pool<SqliteConnectionManager>>`.

### Connection Pool

- `:memory:` databases → `max_size(1)` (no concurrent connections)
- File databases → `max_size(4)`
- `PragmaCustomizer` sets WAL mode + `busy_timeout=5000` on each connection

### LRU Eviction

On `save()`, if entry count exceeds `max_entries`:
- `BEGIN IMMEDIATE` transaction (write lock)
- Delete oldest entries by `timestamp` until under the limit
- Insert the new entry
- Commit

### FTS5 Sync

Three triggers keep the `entries_fts` virtual table in sync with `entries`:
- `entries_ai` (after insert)
- `entries_ad` (after delete)
- `entries_au` (after update)

## Schema (v1)

```sql
CREATE TABLE IF NOT EXISTS schema_version (
    id      INTEGER PRIMARY KEY CHECK(id = 1),
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS entries (
    id          TEXT PRIMARY KEY,
    content     TEXT NOT NULL,
    timestamp   INTEGER NOT NULL,
    kind        TEXT NOT NULL CHECK(kind IN ('Manual','PreCompact','Auto')),
    token_count INTEGER CHECK(token_count >= 0),
    created_at  INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp);

CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    content, content=entries, content_rowid=rowid
);

-- FTS sync triggers
CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES('delete', old.rowid, old.content);
END;

CREATE TRIGGER IF NOT EXISTS entries_au AFTER UPDATE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, content) VALUES('delete', old.rowid, old.content);
    INSERT INTO entries_fts(rowid, content) VALUES (new.rowid, new.content);
END;
```

STRICT mode enforces column types. CHECK constraints enforce valid `kind` values and non-negative `token_count`.

## CLI

`crates/cli/src/main.rs`

### Argument Parsing

clap derive-based. Subcommands: `pre-compact`, `save`, `query`, `clear`, `info`.

### Config File

`cf query` loads an optional TOML config from `~/.context-forge/config.toml`. Supported keys: `token_budget` (usize), `top_k` (usize), `recency_half_life_hours` (f64). CLI flags take precedence over config values, which take precedence over compile-time defaults.

### Query Preprocessing

Before passing a query to the engine, the CLI checks for FTS5 operators (AND, OR, NOT, NEAR, quoted phrases). If none are found and the query has multiple words, it inserts `OR` between each word. This happens in `crates/cli/` — the core engine receives a valid FTS5 query string.

### Database Path Resolution

`default_db_path()` tries in order:

1. `dirs::home_dir()` → `~/.context-forge/context.db`
2. `dirs::data_dir()` → `<data_dir>/context-forge/context.db`
3. `dirs::config_dir()` → `<config_dir>/context-forge/context.db`
4. `std::env::temp_dir()` → `<tmp>/context-forge/context.db`

`ensure_db_dir()` creates the parent directory if it doesn't exist.

### Engine Construction

`make_engine()` wires `SqliteStorage` + `SqliteSearcher` → `ContextEngine`, using a shared connection pool.

## napi Layer

`crates/napi/`

All async operations use `napi::Task` on the libuv worker pool — no tokio runtime.

### Async Methods

| Method | Description |
|--------|-------------|
| `open` | Open/create database, run migrations |
| `save` | Store a context entry |
| `query` | Free-text FTS5 search |
| `search` | Scored search with limit |
| `assemble` | Search + recency weight + budget pack |
| `clear` | Delete all entries |
| `count` | Return entry count |
| `close` | `PRAGMA wal_checkpoint(TRUNCATE)`, drop pool |

## Build and Release

### Local Build

```bash
cargo build --workspace
```

### Release Process

GitHub Actions `workflow_dispatch` with a version input. Runs a 3-platform matrix:

| Platform | CLI Binary | napi Binary | VSIX |
|----------|-----------|-------------|------|
| Linux x64 | `cf-linux-x64` | `cf_napi.linux-x64.node` | `context-forge-linux-x64.vsix` |
| macOS ARM64 | `cf-darwin-arm64` | `cf_napi.darwin-arm64.node` | `context-forge-darwin-arm64.vsix` |
| Windows x64 | `cf-windows-x64.exe` | `cf_napi.win32-x64.node` | `context-forge-win32-x64.vsix` |

### Release Profile

```toml
[profile.release]
lto = true
strip = true
codegen-units = 1
opt-level = "z"
```

Optimized for binary size — LTO + strip + single codegen unit + size optimization.

## Agent System

Custom VS Code agents in `.github/agents/` provide specialized review and research capabilities. Each agent has scoped tools and a defined role:

| Agent | File | Tools | Role |
|-------|------|-------|------|
| Claude | `claude.agent.md` | read, search, execute, edit, todo | Orchestrator — planning, architecture, coordination |
| Codex | `codex.agent.md` | read, search, execute, edit, todo | Implementation — writing code, tests, debugging |
| Review | `review.agent.md` | read, search | Engineering quality — design patterns, layer separation, scalability |
| Security | `security.agent.md` | read, search | Vulnerability auditing, threat modeling, access control review |
| Documentation | `docs.agent.md` | read, search, edit, todo | Non-code artifacts — README, guides, design docs |
| Clean Code | `clean-code.agent.md` | read, search | Readability — naming, decomposition, idiomatic patterns, module organization. **Performance Precedence Rule:** optimal data structures and processing efficiency take priority over readability in hot paths; flags missing performance documentation rather than the code structure |
| Research | `research.agent.md` | read, search, web | Build-vs-buy analysis, crate/library discovery, prior art. Enforces a 3-tier **Trusted Source Registry** (Tier 1: official registries, Tier 2: community vetted, Tier 3: use with caution) and a 7-point supply chain security checklist. Never recommends packages not on a Tier 1 registry |

**Complementary review roles:** Review Agent covers architecture and correctness; Clean Code Agent covers readability and standards. Both are read-only.
