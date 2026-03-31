# Contributing to Context Forge

## Development Setup

1. **Rust toolchain** — Install via [rustup](https://rustup.rs/). Stable channel is sufficient.
2. **Node.js 20+** — Required for the VS Code extension and napi-rs build.
3. **VS Code Insiders** — The extension uses proposed APIs (`chatHooks`, `chatContextProvider`, `chatSessionCustomizationProvider`) that are only available in Insiders builds.

## Build Commands

```bash
# Build all crates
cargo build --workspace

# Run all tests
cargo test --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format check
cargo fmt --all -- --check

# Format fix
cargo fmt --all
```

For the extension:

```bash
cd extension/
npm install
npm run build
npm run lint
```

## Code Style

- **`cargo fmt`** on all Rust code before committing.
- **`clippy::pedantic`** is the lint baseline. All warnings are errors in CI.
- **No `.unwrap()`** in library crates (`core`, `storage`, `napi`). Use `Result<T, E>` with proper error propagation. `.unwrap()` is acceptable in tests and in the `cli` crate's `main()` only.
- Custom error types via `thiserror` in library crates. `anyhow` is acceptable in `cli` only.
- Validate inputs at system boundaries (FFI, CLI args, SQL bindings) — don't re-validate internally.

## Commit Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add BM25 scoring to search results
fix: handle empty query in context assembly
chore: update rusqlite to 0.32
docs: add storage crate architecture notes
test: add integration tests for WAL concurrency
refactor: extract token budgeting into separate module
```

The scope is optional but encouraged for multi-crate changes: `feat(storage): add FTS5 migration`.

## Branch Strategy

- **`main`** — stable, release-ready code
- **Feature branches** — branch from `main`, PR back to `main`
- Branch naming: `feature/short-description`, `fix/short-description`

## Testing Expectations

- **Unit tests** for all public functions in library crates.
- **Storage tests** use in-memory SQLite (`:memory:`) — no temp files for unit tests.
- **Integration tests** use `tempfile` for database paths when testing real file I/O.
- **Core logic** is pure (no I/O, no side effects) — test with in-memory fixtures.
- Dependencies are injected via trait objects — never globally imported.

## Agent-Assisted Development

This repo uses AI agent definitions in `.github/agents/` for agent-assisted development workflows. These agents have specific roles (code, review, security, docs) and are part of the normal development process. See the agent files for their scopes and responsibilities.

## Architecture Rules

Before contributing, read [docs/design-principles.md](docs/design-principles.md). The key constraints:

- **Layer rules are strict** — `extension/` → `napi/` → `core/` ← `cli/`. Nothing skips layers.
- **`core` never imports `rusqlite`** — it depends on storage traits only.
- **No business logic in `napi` or `cli`** — they are thin translation/delegation layers.
