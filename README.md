# Context Forge

Compaction-aware persistent memory engine for AI coding agents in VS Code.

<!-- CI badges will go here once GitHub Actions is configured -->

## What Is This?

When VS Code compacts a long conversation, accumulated context — architecture decisions, session learnings, user preferences — gets summarized or silently dropped. The agent loses hard-won knowledge and starts asking questions you already answered.

**Context Forge** hooks into the compaction lifecycle to solve this:

1. **PreCompact hook** — Before compaction runs, the CLI binary snapshots critical context into a local SQLite database (FTS5, WAL mode).
2. **Context provider** — When a new or compacted session starts, the napi layer reads from that database and re-injects relevant context into the conversation automatically.
3. **Scoring and budgeting** — Context entries are ranked by relevance (BM25) and recency, then assembled within a configurable token budget.

No network calls. No API keys. Everything stays local.

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

**`crates/cli`** — clap-based CLI binary. Invoked by the VS Code `chatHooks` PreCompact trigger to snapshot context before compaction. Delegates entirely to `core`.

## Prerequisites

- **Rust** stable toolchain (rustup)
- **Node.js** 20+
- **VS Code Insiders** (required for proposed API access)

## Quick Start

```bash
git clone https://github.com/asvarnon/context-forge.git
cd context-forge

# Build all Rust crates
cargo build --workspace

# Build the VS Code extension
cd extension/
npm install
npm run build
cd ..

# Run tests
cargo test --workspace
```

To launch the extension, open VS Code Insiders with the workspace and press `F5` (requires the extension dev host configuration).

## Project Structure

```
context-forge/
├── crates/
│   ├── core/          # Business logic, traits, scoring
│   ├── storage/       # rusqlite, FTS5, migrations
│   ├── napi/          # napi-rs bindings for VS Code
│   └── cli/           # clap binary for PreCompact hook
├── extension/         # TypeScript VS Code extension host
├── docs/
│   └── design-principles.md
├── .github/
│   └── agents/        # AI agent definitions
├── Cargo.toml         # Workspace manifest
└── README.md
```

## Related Repos

| Repo | Purpose |
|------|---------|
| [context-forge-hub](https://github.com/asvarnon/context-forge-hub) | Documentation hub — architecture decisions, research, agent context |
| [context-forge-poc](https://github.com/asvarnon/context-forge-poc) | TypeScript proof-of-concept (validated, complete) |

## Current Status

**Pre-development** — version 0.0.0. The TypeScript POC has been validated (all 6 E2E tests passed). Design principles and implementation plan are finalized. No Rust code has been written yet — Phase 0 (workspace scaffolding) has not started.

The implementation plan covers 8 phases (0–7), progressing from workspace setup through storage, core logic, napi bindings, CLI, extension integration, and packaging.
