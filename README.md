# Context Forge

Compaction-aware persistent memory engine for AI coding agents.

## What Is This?

When AI coding agents compact long conversations, accumulated context — architecture decisions, session learnings, user preferences — gets summarized or silently dropped. The agent loses hard-won knowledge and starts asking questions you already answered.

**Context Forge** hooks into the compaction lifecycle to solve this:

1. **PreCompact hook** — Before compaction, snapshots the full conversation transcript into a local SQLite database (FTS5, WAL mode).
2. **PostCompact hook** — After compaction, stores the compact summary for future sessions.
3. **SessionStart hook** — When a new session starts, assembles relevant context (BM25 + recency scoring) within a token budget and injects it automatically.

**Primary integration: [Claude Code](https://code.claude.com/)** via its hooks system — no VS Code required. Also ships as a VS Code extension (requires VS Code Insiders for proposed APIs).

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
                                      ▲
                                      │ (implements traits)
                                crates/storage/
```

**Layer rules:**
- `extension/` calls `crates/napi/` only — thin VS Code host, no business logic
- `crates/napi/` and `crates/cli/` call `crates/core/` only
- `crates/core/` depends on storage **traits**, never on `rusqlite` directly
- `crates/storage/` implements traits defined in `core`, owns all SQL

Both `napi` (reader) and `cli` (writer) access the same SQLite file. WAL mode enables concurrent reads alongside a single writer without locking conflicts.

## Crates

**`crates/core`** — Pure business logic. Search, context assembly, token budgeting, and relevance scoring. No I/O, no side effects. Defines the `ContextStorage` and `Searcher` traits that downstream crates implement.

**`crates/storage`** — SQLite persistence layer. Implements `core` traits using rusqlite with bundled-full feature, FTS5 for full-text search, WAL mode for concurrency, and forward-only schema migrations.

**`crates/napi`** — napi-rs FFI bindings. Translates between napi types and core types at the boundary. Called by the VS Code extension to read and inject context. No business logic.

**`crates/cli`** — clap-based CLI binary (`cf`). Invoked by Claude Code hooks or directly from the terminal. Subcommands: `pre-compact`, `save`, `query`, `clear`, `info`. Delegates entirely to `core`.

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

## Related Repos

| Repo | Purpose |
|------|---------|
| [context-forge-hub](https://github.com/asvarnon/context-forge-hub) | Documentation hub — architecture decisions, research, agent context |
| [context-forge-poc](https://github.com/asvarnon/context-forge-poc) | TypeScript proof-of-concept (validated, complete) |

## Current Status

**v0.1.0** — all phases complete and [released on GitHub](https://github.com/asvarnon/context-forge/releases) with cross-platform binaries. See [open issues](https://github.com/asvarnon/context-forge/issues) for the backlog.
