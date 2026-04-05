# Context Forge

Compaction-aware persistent memory engine for AI coding agents.

This project started as personal tinkering — an experiment in how far an AI coding agent can be pushed to manage its own memory and decide what context matters across sessions. It grew well beyond its original scope into a full pipeline covering conversation snapshots, BM25 retrieval, importance detection, and multi-runtime hook support.

## What Is This?

When AI coding agents compact long conversations, accumulated context — architecture decisions, session learnings, user preferences — gets summarized or silently dropped. The agent loses hard-won knowledge and starts asking questions you already answered.

**Context Forge** hooks into the compaction lifecycle to solve this:

1. **PreCompact hook** — Before compaction, snapshots the full conversation transcript into a local SQLite database (FTS5, WAL mode).
2. **PostCompact hook** — After compaction, stores the compact summary for future sessions.
3. **SessionStart hook** — When a new session starts, assembles relevant context (BM25 + recency scoring) within a token budget and injects it automatically.

**Primary integration: [Claude Code](https://code.claude.com/)** via its hooks system — no VS Code required. Also ships as a VS Code extension (requires VS Code Insiders for proposed APIs).

Optionally, Context Forge runs an importance detection pipeline that surfaces high-value passages — corrective instructions, design decisions, and recurring patterns — across sessions. These are injected as a dedicated block before BM25 results when `--source` is passed on the `SessionStart` hook.

Hook payloads are auto-detected across runtimes (Claude Code, Codex CLI, Gemini CLI, Cline, OpenClaw), normalizing runtime-specific fields into a unified schema. Pass `--runtime <name>` to override auto-detection.

No network calls. No API keys. Everything stays local.

## Install

### Install Scripts

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/asvarnon/context-forge/main/scripts/install.sh | bash

# Windows (PowerShell)
irm https://raw.githubusercontent.com/asvarnon/context-forge/main/scripts/install.ps1 | iex
```

### Manual Download

Download the `cf` binary for your platform from [GitHub Releases](https://github.com/asvarnon/context-forge/releases):

| Platform | Binary |
|----------|--------|
| Linux x64 | `cf-linux-x64` |
| macOS ARM64 | `cf-darwin-arm64` |
| Windows x64 | `cf-windows-x64.exe` |

Place it on your `PATH` and verify with `cf --version`.

## Claude Code Integration

Context Forge integrates with Claude Code via CLI hooks — `PreCompact`, `PostCompact`, and `SessionStart`. See [docs/claude-code-setup.md](docs/claude-code-setup.md) for full configuration.

## Architecture

```
extension/ ──→ crates/napi/ ──→ crates/core/ ←── crates/cli/
                                      ▲                ▲
                                      │         crates/analysis/
                               crates/storage/
```

**Layer rules:**
- `extension/` calls `crates/napi/` only — thin VS Code host, no business logic
- `crates/napi/` and `crates/cli/` call `crates/core/` only
- `crates/core/` depends on storage **traits**, never on `rusqlite` directly
- `crates/storage/` implements traits defined in `core`, owns all SQL
- `crates/analysis/` runs the importance pipeline; imports from `core/` only

Both `napi` (reader) and `cli` (writer) access the same SQLite file. WAL mode enables concurrent reads alongside a single writer without locking conflicts.

## Crates

**`crates/core`** — Pure business logic. Search, context assembly, token budgeting, and relevance scoring. No I/O, no side effects. Defines the `ContextStorage` and `Searcher` traits that downstream crates implement.

**`crates/storage`** — SQLite persistence layer. Implements `core` traits using rusqlite with bundled-full feature, FTS5 for full-text search, WAL mode for concurrency, and forward-only schema migrations.

**`crates/napi`** — napi-rs FFI bindings. Translates between napi types and core types at the boundary. Called by the VS Code extension to read and inject context. No business logic.

**`crates/analysis`** — Importance detection pipeline. Pre-filtering, tokenization, n-gram extraction, session-frequency scoring, context extraction, classification, and importance scoring. Imports from `core/` only — no I/O, no storage access.

**`crates/cli`** — clap-based CLI binary (`cf`). Invoked by Claude Code hooks or directly from the terminal. Subcommands: `pre-compact`, `save`, `query`, `clear`, `info`. Delegates entirely to `core`.

## Agent System

Custom VS Code agents in `.github/agents/` provide specialized capabilities:

| Agent | Role |
|-------|------|
| Claude | Orchestrator — planning, architecture, coordination |
| Codex | Implementation — code, tests, debugging |
| Review | Engineering quality — design patterns, scalability |
| Security | Vulnerability auditing, threat modeling |
| Documentation | Non-code artifacts — README, guides, design docs |
| Clean Code | Readability — naming, decomposition, idiomatic patterns. Performance takes precedence over readability in hot paths |
| Research | Build-vs-buy analysis, library discovery, prior art. Enforces trusted source registry and supply chain security checklist |

## Development Setup

```bash
git clone https://github.com/asvarnon/context-forge.git
cd context-forge

# Build all Rust crates
cargo build --workspace

# Run tests
cargo test --workspace

# Build the VS Code extension
cd extension/
npm install
npm run build
cd ..
```

**Prerequisites:**
- Rust stable toolchain (rustup)
- Node.js 20+
- VS Code Insiders (only needed for extension development — proposed API access)

To launch the extension, open VS Code Insiders with the workspace and press `F5`.

## Project Structure

```
context-forge/
├── crates/
│   ├── analysis/      # Importance detection pipeline
│   ├── core/          # Business logic, traits, scoring
│   ├── storage/       # rusqlite, FTS5, migrations
│   ├── napi/          # napi-rs bindings for VS Code
│   └── cli/           # clap binary for PreCompact hook
├── extension/         # TypeScript VS Code extension host
├── scripts/           # Install scripts (install.sh, install.ps1)
├── docs/
│   ├── claude-code-setup.md
│   ├── design-principles.md
│   └── ARCHITECTURE.md
├── Cargo.toml         # Workspace manifest
└── README.md
```

## CLI Reference

```
cf pre-compact          Snapshot conversation transcript (reads stdin)
cf save [--kind auto]   Store a context entry (reads stdin)
cf query                Assemble and output context
cf clear                Delete all entries
cf info                 Print database diagnostics
```

**Common flags for `cf query`:**

| Flag | Default | Description |
|------|---------|-------------|
| `--query` | *(none — returns all)* | FTS5 search query to filter entries (supports AND, OR, NOT, NEAR, quoted phrases) |
| `--token-budget` | 16000 | Max tokens to assemble. Increase for richer context |
| `--top-k` | 10 | Max entries to consider |
| `--format` | json | Output format: `json` or `text` |
| `--source` | *(none)* | Event source (`startup`, `resume`, `compact`, `clear`). When set, enables importance injection |
| `--importance-budget` | 512 | Token ceiling for the importance block (prepended before BM25 results) |
| `--db` | `~/.context-forge/context.db` | Database path |

All subcommands support `--help` for full usage.

## Configuration

Optional config file at `~/.context-forge/config.toml` sets defaults for `cf query`:

```toml
token_budget = 16000
top_k = 10
recency_half_life_hours = 72.0
```

For `token_budget` and `top_k`, CLI flags override config file values, which override compile-time defaults. `recency_half_life_hours` is read only from the config file (or falls back to the compile-time default if not set).

## Current Status

All phases complete and [released on GitHub](https://github.com/asvarnon/context-forge/releases) with cross-platform binaries. See [open issues](https://github.com/asvarnon/context-forge/issues) for the backlog.
