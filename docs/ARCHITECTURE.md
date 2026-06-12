# Architecture

Internal developer reference for `context-forge`. The crate is a single
library (`context_forge`) — no CLI, no FFI bridge, no extension.

## Design principles

- **Local-first.** SQLite is the only storage backend; no network calls, no
  external services, no credentials required by the library itself.
- **Sync API.** The library performs blocking SQLite I/O and never spawns
  its own threads or async runtime. Async callers wrap calls with
  `tokio::task::spawn_blocking` (see the crate-level docs in `src/lib.rs`).
- **Trait-based extensibility.** `ContextStorage` and `Searcher`
  (`src/traits.rs`) are the seams for a future alternate storage backend or
  search strategy. `ContextEngine` depends on these traits as trait objects,
  never on `rusqlite` directly.
- **Error handling.** `thiserror` for the crate's single `Error` type
  (`src/error.rs`); `anyhow` is banned. No `.unwrap()`/`.expect()` outside
  `#[cfg(test)]`. Validate inputs at API boundaries (e.g. `save_snapshot`
  rejects empty content) rather than re-validating internally.
- **Config-driven behavior.** Token budgets, eviction policy, recency
  half-life, and secret-scrubbing are all `Config` fields, not hardcoded
  constants — see `src/config.rs`.

## Module map

| Module | Responsibility | I/O |
|---|---|---|
| `src/lib.rs` | Public API surface: the `ContextForge` facade (`open`, `save`, `query`, `delete`, `clear_scope`, `clear_all`, `count`) and crate-level docs (untrusted-memory doctrine, async pattern). Re-exports the rest of the public surface. | none directly (delegates) |
| `src/engine.rs` | `ContextEngine`: `assemble` (search → recency decay → bin-pack) and `save_snapshot`. `SaveOptions`, `MATCH_ALL_QUERY`, `estimate_tokens`. | none |
| `src/entry.rs` | `ContextEntry`, `ScoredEntry`, the `kind` constants module. | none |
| `src/config.rs` | `Config`, `EvictionPolicy`, recency-half-life constants. | none |
| `src/error.rs` | `Error` (`thiserror`), the crate's only error type. | none |
| `src/traits.rs` | `ContextStorage` and `Searcher` traits, the `Result` alias. | none |
| `src/session.rs` | `group_entries_by_session` and `SessionGroup` — groups entries by explicit `session_id` or timestamp proximity. | none |
| `src/scrub.rs` | `scrub_secrets`, `ScrubConfig`. Compiles a fixed set of regexes and redacts credential-shaped substrings. | none |
| `src/storage/mod.rs` | `SqliteStorage` (implements `ContextStorage`), `open_storage`, connection pool setup (r2d2 + rusqlite, WAL mode via `PragmaCustomizer`). | SQLite |
| `src/storage/schema.rs` | Forward-only migrations (`SCHEMA_V1`..`SCHEMA_V3`, `migrate`), `schema_version` table, `row_to_entry`. | SQLite |
| `src/storage/searcher.rs` | `SqliteSearcher` (implements `Searcher`): FTS5 `MATCH` queries with BM25 scoring, plus the `MATCH_ALL_QUERY` fast path. | SQLite |
| `src/analysis/*` (feature `analysis`) | Importance-detection pipeline: tokenizer, lexicon, n-grams, frequency, classification, scoring, recurrence, injection, prefilter, extraction. | none |

## Purity rules

- `engine.rs`, `entry.rs`, `session.rs`, and every module under `analysis/`
  perform **no I/O**. They operate on `ContextEntry`/`ScoredEntry` values and
  trait objects only.
- All SQL lives in `storage/`. `SqliteStorage` and `SqliteSearcher` are the
  only types that touch `rusqlite`.
- `scrub.rs` is pure: it compiles regexes once (via `OnceLock`) and returns a
  `Cow<str>`, allocating only when a pattern matches.
- `ContextEngine` depends on `ContextStorage` and `Searcher` as trait
  objects (`Box<dyn ...>`), never on `SqliteStorage`/`SqliteSearcher`
  directly.

## Data flow

### Save (`ContextForge::save`)

1. **Facade** (`lib.rs`) receives `content`, `kind`, `SaveOptions`.
2. **Scrub** (`scrub::scrub_secrets`) redacts credential-shaped substrings in
   `content` using the instance's `ScrubConfig`. `SaveOptions::metadata` is
   *not* passed through this step.
3. **Engine** (`ContextEngine::save_snapshot`) rejects empty content,
   generates a UUIDv7 id and a Unix timestamp, estimates `token_count`
   (`text.len().div_ceil(4)`), and builds a `ContextEntry`.
4. **Storage** (`SqliteStorage::save`) opens an `IMMEDIATE` transaction,
   evicts the oldest entry by `timestamp` if at `max_entries` capacity (LRU),
   then `INSERT OR REPLACE`s the row and commits.
5. **FTS triggers** (`entries_ai`/`entries_au`/`entries_ad`, defined in
   `schema.rs`) keep the `entries_fts` virtual table in sync with `entries`
   on insert/update/delete — no application-level FTS maintenance is needed.

Calling `ContextEngine::save_snapshot` or `ContextStorage::save` directly
skips step 2 — those are documented as low-level paths where the caller is
responsible for scrubbing.

### Query (`ContextForge::query`)

1. **Facade** delegates to `ContextEngine::assemble(query, scope,
   token_budget)`.
2. **Searcher** (`SqliteSearcher::search`): if `query == MATCH_ALL_QUERY`
   (`"*"`), returns all entries (optionally filtered by `scope`) ordered by
   `timestamp DESC` with a fixed score of `1.0`. Otherwise runs an FTS5
   `MATCH` query joined against `entries`, scored by `bm25()` (negated so
   higher is better), optionally filtered by `scope`, up to
   `DEFAULT_SEARCH_LIMIT` (50) candidates.
3. **Recency decay** (`engine::recency_decay`): each candidate's score is
   multiplied by `0.5^(age_seconds / half_life)`, where `half_life` is
   `Config::recency_half_life_secs` (default 259,200s / 72h). Non-finite or
   non-positive configured half-lives are clamped to the default in
   `ContextEngine::new`.
4. **Sort**: candidates are sorted by weighted score descending
   (`f64::total_cmp`, so `NaN` sorts consistently).
5. **Bin-pack**: entries are added greedily until `token_budget` would be
   exceeded. Each entry's token cost is its stored `token_count` if present,
   else `estimate_tokens(content)`. An oversized entry is skipped, not
   aborting — smaller, lower-ranked entries can still be packed.

`scope = None` searches/returns everything regardless of scope (global
recall); `scope = Some(s)` restricts to entries whose `scope` column equals
`s`.

## Schema history

Migrations are forward-only and run automatically on `SqliteStorage::open`
via `storage::schema::migrate`, which is idempotent (`schema_version` table
tracks the current version).

- **v1** (legacy): `entries(id, content, timestamp, kind CHECK(...), token_count,
  created_at)` with a `kind` CHECK constraint limited to
  `'Manual'|'PreCompact'|'Auto'`, plus the original `entries_fts` virtual
  table and sync triggers.
- **v2** (legacy): added runtime/agent metadata columns
  (`session_id`, `runtime`, `model`, `cwd`, `git_branch`, `git_sha`,
  `compaction_trigger`, `turn_id`, `agent_type`, `agent_id`, `embedding`,
  `compaction_count`) plus `runtime_configs`, `runtime_field_mappings`,
  `entry_metadata_raw`, `tags`, `entry_tags` tables — artifacts of the
  multi-runtime hook integration.
- **v3** (current): rebuilds `entries` into the current general-purpose
  shape — `id, content, timestamp, kind (free-text), scope, session_id,
  token_count, metadata (JSON), created_at`. The v1 `kind` CHECK constraint
  is dropped (kinds are now caller-defined free text; see `entry::kind` for
  well-known values). Legacy `kind` values are remapped
  (`Manual→manual`, `PreCompact→snapshot`, `Auto→summary`). The v2
  runtime/agent columns are folded into the new `metadata` JSON column, and
  the v2-only runtime tables (`runtime_configs`, `runtime_field_mappings`,
  `entry_metadata_raw`) are dropped. `entries_fts` and its sync triggers are
  rebuilt against the new table. New indexes: `idx_entries_timestamp`,
  `idx_entries_scope`, `idx_entries_session_id`.

A fresh database created today runs straight to v3 (`migrate` applies V1,
V2, V3 in sequence when `version < 1`). Existing v1 or v2 databases are
migrated forward on next open.

## Trait seams

- **`ContextStorage`** (`traits.rs`): `save`, `get_top_k`, `get_all`,
  `delete`, `clear`, `clear_scope`, `count`. Implemented by `SqliteStorage`.
  This is the seam a future non-SQLite backend would implement; `engine.rs`
  never depends on `rusqlite`.
- **`Searcher`** (`traits.rs`): `search(query, scope, limit) ->
  Vec<ScoredEntry>`. Implemented by `SqliteSearcher`. This is the seam a
  future search strategy (e.g. embeddings-based) would implement without
  touching `ContextEngine::assemble`'s decay/bin-pack logic.

Both traits require `Send + Sync` so implementations can be shared across
threads (verified by `context_forge::tests::trait_objects_are_object_safe`
and `context_forge_is_send_sync`).
