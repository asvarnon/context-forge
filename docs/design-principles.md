# Design Principles

## Project Overview

> **Context Forge** — compaction-aware persistent memory engine for AI coding agents. When VS Code compacts a conversation, accumulated context gets summarized or dropped. Context Forge hooks into the compaction lifecycle to preserve and re-inject critical context automatically.
> Stack: Rust (tokio, rusqlite, napi-rs, clap), TypeScript (VS Code extension host)
> Entry points: `crates/cli/src/main.rs` (CLI binary), `extension/src/extension.ts` (VS Code activation)

---

## Architecture

### Layer Structure

```
extension/          ← TypeScript. Thin VS Code host — registers providers, calls napi bindings. No logic.
crates/napi/        ← Rust + napi-rs. FFI translation layer — converts napi types ↔ core types. No logic.
crates/cli/         ← Rust + clap. CLI binary for chatHooks PreCompact. Thin — delegates to core.
crates/core/        ← Rust. Pure business logic — search, assembly, token budgeting, scoring.
crates/storage/     ← Rust + rusqlite. SQLite persistence — FTS5, WAL mode, migrations.
```

**Layer rules:**
- `extension/` calls `crates/napi/` only. Never imports from `core` or `storage` directly.
- `crates/napi/` calls `crates/core/`. Translates napi types at the boundary. No business logic.
- `crates/cli/` calls `crates/core/`. Parses args, delegates, exits. No business logic.
- `crates/core/` depends on storage **traits** only — never on `rusqlite` or any concrete storage impl.
- `crates/storage/` implements traits defined in `core`. Owns all SQL.
- Nothing skips layers. `extension/` never touches `storage/`. `cli/` never touches `napi/`.

### Shared Database

- Both `napi` (reader) and `cli` (writer during PreCompact) access the same SQLite file.
- WAL mode enables concurrent reads + single writer without locking conflicts.
- The CLI binary writes context snapshots before compaction; the napi layer reads them for injection.

---

## Key Design Decisions

### Why Rust for the Core
- `PreCompact` hook has a timeout — need to snapshot and reassemble in milliseconds, not hundreds of milliseconds.
- Single static binary for CLI — no interpreter, no virtualenv, no deps for end users.
- C ABI via napi-rs means the same core serves VS Code (napi), future Python wrapper (PyO3), and future REST/gRPC server.
- `unsafe` is contained to the FFI boundary only — the core library is safe Rust.

### SQLite as Single Storage Backend
- Local-first: no network dependency, no server to run, no credentials.
- FTS5 for semantic-ish search with BM25 scoring — good enough without embedding vectors.
- WAL mode for concurrent reader/writer pattern (napi reads, CLI writes).
- Schema migrations tracked in code with a version table — forward-only.

### Trait-Based Extensibility
- Storage backend is a trait (`ContextStorage`). SQLite is the first (and likely only) implementation, but the core never depends on `rusqlite`.
- Search strategy is a trait (`Searcher`). BM25/FTS5 is the first implementation; vector search can be added without touching core assembly logic.
- Output format is handled by each wrapper crate — core returns structured data, wrappers serialize it.

### Config-Driven Behavior
- Token budgets, eviction policies, scoring weights, and database paths come from configuration — not hardcoded.
- Adding a new scoring heuristic requires a config change + a registry entry, not editing core logic.

### Secret Management
- Context Forge stores user context locally — no secrets management needed for the tool itself.
- The SQLite database file is stored in VS Code's `globalStorageUri` — OS-level file permissions apply.
- No API keys, no network calls, no auth. If REST/gRPC wrapper is added later, this section must be revisited.

---

## Patterns

### Error Handling
- `Result<T, E>` everywhere in Rust — no `.unwrap()` in production code.
- Custom error types via `thiserror` in library crates. `anyhow` acceptable in CLI binary only.
- Every I/O operation (SQLite query, file read, napi callback) has an explicit timeout or is non-blocking.
- Validate inputs at system boundaries: FFI boundary (napi), CLI arg parsing (clap), SQL parameter binding. Don't re-validate internally.

### Dynamic Dispatch
- Behavior that varies by type, strategy, or configuration uses trait objects or enum dispatch — not `if/else` chains.
- New search strategies, scoring algorithms, or output formats are addable by implementing a trait + registering — not by branching on a name.

### Testability
- Core business logic is pure — no I/O, no side effects. Test with in-memory fixtures.
- Storage tests use in-memory SQLite (`:memory:`) — no temp files needed for most tests.
- Integration tests (napi, CLI) use `tempfile` for database paths.
- Dependencies are injected via trait objects — never globally imported.

### Resource Management
- RAII for all resources. `Drop` impls for cleanup (database connections, temp files).
- No manual `close()` calls — the type system handles it.
- Connection pooling for SQLite — never hold a connection across await points.

---

## What "Extensible" Means Here

- Adding a new **storage backend** requires: implementing the `ContextStorage` trait in a new crate — no core changes.
- Adding a new **search strategy** requires: implementing the `Searcher` trait — no core changes.
- Adding a new **output wrapper** (MCP, REST, Python) requires: a new thin crate calling `core` — no core changes.
- Adding a new **scoring heuristic** requires: a config entry + a registry function — no core changes.
- Adding a new **context entry type** requires: extending the `EntryKind` enum in core + a migration in storage.

If any of the above requires editing core assembly or search logic, the abstraction is wrong.

---

## Anti-Patterns (never do these)

- **`.unwrap()` in library code** — always use `?` with proper error types or `.expect("reason")` for provably impossible states
- **Business logic in napi/cli/extension layers** — these are thin translation/delegation layers only
- **Raw SQL string interpolation** — parameterized queries exclusively, even for "safe" values
- **`unsafe` outside the FFI boundary** — if you think you need `unsafe` in core, the design is wrong
- **Hardcoded paths or timeouts** — all operational parameters come from config
- **Growing conditionals** — new behavior goes in dispatch tables/trait impls, not `match` arms in core
- **Blocking the Node.js event loop** — all napi functions that touch I/O must be async

---

## Changelog

| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-03-30 | POC validated with TypeScript-only extension | Confirmed chatContextProvider survives compaction, chatHooks is shell-only |
| 2026-03-31 | Rust core architecture selected | Performance requirements for PreCompact timeout, multi-wrapper via C ABI |
| 2026-03-31 | SQLite + FTS5 chosen over embedded KV stores | WAL mode concurrent access, BM25 scoring built-in, single-file deployment |
| 2026-03-31 | Workspace Cargo structure (crates/) | Clean layer separation, independent compilation, trait-based dependency inversion |
