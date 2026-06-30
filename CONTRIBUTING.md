# Contributing to Context Forge

## Development Setup

1. **Rust toolchain** — Install via [rustup](https://rustup.rs/). Stable
   channel is sufficient.

This is a single Rust library crate (`context-forge`, lib name
`context_forge`). No other tooling is required.

## Build Commands

```bash
# Build
cargo build
cargo build --all-features
cargo build --no-default-features

# Test
cargo test
cargo test --all-features

# Lint (must stay clean — this is the CI gate)
cargo clippy --all-features -- -D warnings

# Format
cargo fmt --all
cargo fmt --all -- --check

# Single test
cargo test <test_name>
```

## Code Style

- **`cargo fmt --all`** on all Rust code before committing.
- **`cargo clippy --all-features -- -D warnings`** must be clean.
  `clippy::pedantic` is not yet crate-wide (deferred), but `#![warn(clippy::pedantic)]`
  is set at the crate root in `src/lib.rs` and any warnings it produces are
  part of the `-D warnings` gate.
- **No `.unwrap()` / `.expect()`** outside `#[cfg(test)]`. Use `Result<T, E>`
  with proper error propagation.
- **`thiserror`** for all errors (`src/error.rs`). `anyhow` is banned
  everywhere in this crate.
- **Library is async** — all public `ContextForge` methods are `async` and
  require a tokio runtime. The `distill-http` feature requires the
  multi-thread flavor due to internal `block_in_place` usage.
- **Module purity** — `engine.rs`, `entry.rs`, `session.rs`, and
  `analysis/*` do no I/O. All SQL stays in `storage/`.
- Idiomatic Rust per the Rust API Guidelines: borrow at API boundaries,
  minimize `pub` surface, apply `#[must_use]` / `#[non_exhaustive]`
  consistently on new public items.

## Commit Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add scoped namespace filtering to query
fix: handle empty query in context assembly
chore: update turso to 0.7
docs: update architecture notes for v3 schema
test: add migration tests for v2 to v3
refactor: extract token estimation into engine module
```

## Branch Strategy

- **`main`** — stable, release-ready code.
- **Feature branches** — branch from `main`, PR back to `main`.
- Branch naming: `feature/short-description`, `fix/short-description`.

## Testing Expectations

- **Unit tests** for all public functions.
- **Storage tests** use in-memory turso (`:memory:`) where possible;
  a real file path only when the test specifically exercises on-disk behavior.
- **Engine/analysis tests** use mock `ContextStorage`/`Searcher`
  implementations — no real database needed.
- Dependencies are injected via trait objects (`ContextStorage`, `Searcher`)
  — never globally imported.

## Architecture Rules

Before contributing, read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The
key constraints:

- All I/O (turso + Tantivy) lives in `src/storage/`. `engine.rs`, `entry.rs`,
  `session.rs`, and `analysis/*` do no I/O.
- `ContextEngine` depends on the `ContextStorage` and `Searcher` traits, never
  on `turso` or `TursoStorage`/`TursoSearcher` directly.
- Schema setup is a single idempotent `CREATE TABLE IF NOT EXISTS` — there is
  no migration system or `schema_version` table.
