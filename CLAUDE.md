# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Context Forge is a compaction-aware persistent memory engine for AI coding agents. It snapshots conversations before Claude Code compacts them, stores BM25-indexed summaries in local SQLite, and re-injects relevant context at session start — all offline, no network calls.

## Commands

### Rust (core + CLI + napi)
```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all

# Single test
cargo test -p context-forge-core <test_name>
cargo test -p context-forge-storage <test_name>
```

### VS Code Extension (TypeScript)
```bash
cd extension/
npm install
npm run build
npm run lint
```

## Architecture

Strict layered dependency graph — no upward or lateral imports:

```
extension/   (TypeScript VS Code host)
    ↓
crates/napi/ (napi-rs FFI bridge → JS)
    ↓
crates/core/ (pure business logic — no I/O)
    ↑
crates/cli/  (clap binary; invoked by Claude Code hooks)
    ↑
crates/storage/ (SQLite + FTS5; implements core traits)
```

**`core/`** is pure — zero I/O, depends only on traits it defines (`ContextStorage`, `Searcher` in `traits.rs`). All SQL lives in `storage/`. CLI and napi are thin translation layers with no business logic.

### Core Engine (`crates/core/src/engine.rs`)

Assemble algorithm:
1. FTS5 BM25 search against query string
2. Apply recency decay: `score * 0.5^(age_seconds / 86400)` (24-hour half-life)
3. Sort by weighted score descending
4. Greedy bin-pack into token budget (skip oversized entries, don't stop)

Token estimate: `text.len() / 4`

### CLI Subcommands (`crates/cli/src/main.rs`)

| Command | Hook | Purpose |
|---|---|---|
| `cf pre-compact` | PreCompact | Snapshot current conversation to DB |
| `cf save [--kind auto]` | PostCompact | Store compaction summary |
| `cf query [--format text] [--top-k N]` | SessionStart | Assemble + emit context |
| `cf clear` | — | Delete all entries |
| `cf info` | — | Print DB diagnostics |

### Storage (`crates/storage/`)

- SQLite in WAL mode, default path `~/.context-forge/context.db`
- FTS5 virtual table for BM25 full-text search
- Schema version 1, forward-only migrations in `schema.rs`
- Eviction: when over `max_entries`, delete oldest by timestamp in a single transaction
- Tests use in-memory SQLite (`:memory:`) or `tempfile`

## Code Rules

- **No `.unwrap()` in library crates** (`core/`, `storage/`, `napi/`). CLI (`anyhow`) and tests are exempt.
- **`thiserror` for library errors**, `anyhow` only in the CLI binary.
- **No business logic in `extension/`, `napi/`, or `cli/`** — they are translation layers.
- All Rust must pass `clippy::pedantic` and `cargo fmt`.
