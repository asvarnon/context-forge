# Architecture

Internal developer reference for `context-forge`. The crate is a single
library (`context_forge`) — no CLI, no FFI bridge, no extension.

## Design principles

- **Local-first.** turso (async SQLite) is the persistence backend; standalone
  Tantivy provides the in-memory BM25 index. No cloud calls, no external
  services, no credentials required by the library itself.
- **Async API.** All public `ContextForge` methods are `async` and require a
  tokio runtime. The `distill-http` feature requires the multi-thread flavor
  due to internal `block_in_place` usage.
- **Trait-based extensibility.** `ContextStorage` and `Searcher`
  (`src/traits.rs`) are the seams for a future alternate storage backend or
  search strategy. `ContextEngine` depends on these traits as trait objects,
  never on `turso` or `tantivy` directly.
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
| `src/engine.rs` | `ContextEngine`: `assemble` (BM25 + semantic search → RRF fusion → recency decay → lexicon scoring → bin-pack) and `save_snapshot`. `SaveOptions`, `MATCH_ALL_QUERY`, `estimate_tokens`. | none |
| `src/entry.rs` | `ContextEntry`, `ScoredEntry`, the `kind` constants module. | none |
| `src/config.rs` | `Config`, `EvictionPolicy`, recency-half-life constants. | none |
| `src/error.rs` | `Error` (`thiserror`), the crate's only error type. | none |
| `src/traits.rs` | `ContextStorage` and `Searcher` traits, the `Result` alias. | none |
| `src/session.rs` | `group_entries_by_session` and `SessionGroup` — groups entries by explicit `session_id` or timestamp proximity. | none |
| `src/scrub.rs` | `scrub_secrets`, `ScrubConfig`. Compiles a fixed set of regexes and redacts credential-shaped substrings. | none |
| `src/storage/mod.rs` | `open_storage` async constructor — builds `Arc<FtsIndex>`, opens turso, returns paired `(TursoStorage, TursoSearcher)`. | none directly |
| `src/storage/fts_index.rs` | `FtsIndex` — shared in-memory Tantivy index (`Arc<FtsIndex>` held by both storage and searcher). Exposes `add`, `remove`, `clear`, `commit`. Tantivy's in-memory directory; rebuilt from turso on every `open_storage`. | none (in-memory) |
| `src/storage/turso_storage.rs` | `TursoStorage` (implements `ContextStorage`). Holds `Arc<turso::Database>` + `Arc<FtsIndex>`. Every write dual-commits: turso first, then `fts.add() + fts.commit()`. LRU eviction, `busy_timeout` per connection. | turso + Tantivy |
| `src/storage/turso_searcher.rs` | `TursoSearcher` (implements `Searcher`). BM25 path: Tantivy query → scored UUIDs → turso fetch → zip scores. Semantic path: `vector_distance_cos` over the `embedding` column, converted to cosine similarity. `MATCH_ALL_QUERY` fast path: turso `ORDER BY timestamp DESC`, score `1.0`. | turso + Tantivy |
| `src/lexicon/` (feature `analysis`) | `LexiconScorer` trait, `ConfigLexiconScorer` (TOML-driven, f64 weights, case-insensitive, 3-token negation window), `DefaultEnglishScorer`, `CompositeLexiconScorer`, `LexiconAppender` (atomic temp-rename writes), `LexiconProposal`, `bootstrap_prompt`. | disk (appender only) |
| `src/semantic/` (feature `semantic`) | `Embedder` trait and `FasEmbedder` (fastembed all-MiniLM-L6-v2, ONNX Runtime, 384 dim). Inference runs inside `spawn_blocking`; guards the inner model with a `Mutex` to satisfy `Send + Sync`. | disk (model weights cache) |
| `src/analysis/*` (feature `analysis`) | Importance-detection pipeline: tokenizer, lexicon, n-grams, frequency, classification, scoring, recurrence, injection, prefilter, extraction. | none |

## Purity rules

- `engine.rs`, `entry.rs`, `session.rs`, and every module under `analysis/`
  perform **no I/O**. They operate on `ContextEntry`/`ScoredEntry` values and
  trait objects only.
- Core I/O lives in `storage/`. `TursoStorage` is the only type that touches
  `turso`; `FtsIndex` is the only type that touches `tantivy`.
- `scrub.rs` is pure: it compiles regexes once (via `OnceLock`) and returns a
  `Cow<str>`, allocating only when a pattern matches.
- `lexicon/` is mostly pure — `ConfigLexiconScorer` and scorers do no I/O.
  The exception is `LexiconAppender`, which reads and writes the lexicon file
  via a temp-rename pattern.
- `semantic/` (`FasEmbedder`) reads model weights from disk at init time and
  performs blocking CPU inference inside `spawn_blocking`.
- `ContextEngine` depends on `ContextStorage` and `Searcher` as trait
  objects (`Box<dyn ...>`), never on `TursoStorage`/`TursoSearcher` directly.

## Data flow

### Save (`ContextForge::save`)

1. **Facade** (`lib.rs`) receives `content`, `kind`, `SaveOptions`.
2. **Scrub** (`scrub::scrub_secrets`) redacts credential-shaped substrings in
   `content` using the instance's `ScrubConfig`. `SaveOptions::metadata` is
   *not* passed through this step.
3. **Engine** (`ContextEngine::save_snapshot`) rejects empty content,
   generates a UUIDv7 id and a Unix timestamp, estimates `token_count`
   (`text.len().div_ceil(4)`), and builds a `ContextEntry`.
4. **Storage** (`TursoStorage::save`) opens a fresh turso connection, evicts
   the oldest entry by `timestamp` if at `max_entries` capacity (LRU), then
   `INSERT OR REPLACE`s the row and commits.
5. **FTS dual-write**: after turso commits, `fts.add(id, content)` updates the
   Tantivy in-memory index and `fts.commit()` flushes it. Both writes succeed
   or the save is considered failed.
6. **Embedding** (feature `semantic`, non-fatal): if an `Embedder` is wired,
   `embed_and_store` is called in a `spawn_blocking` task after the save
   commits. It embeds the content and issues an `UPDATE entries SET embedding =
   ?` for the new row's UUID. Failure is logged and swallowed — a missing
   embedding degrades search quality but does not fail the save.

Calling `ContextEngine::save_snapshot` or `ContextStorage::save` directly
skips step 2 — those are documented as low-level paths where the caller is
responsible for scrubbing.

### Query (`ContextForge::query`)

1. **Facade** delegates to `ContextEngine::assemble(query, scope,
   token_budget)`.
2. **BM25 search** (`TursoSearcher::search`): if `query == MATCH_ALL_QUERY`
   (`"*"`), queries turso directly (ordered by `timestamp DESC`, score `1.0`),
   optionally filtered by `scope`. Otherwise: Tantivy `QueryParser` (OR-joined
   terms, lenient parse) produces scored UUIDs; those IDs are fetched from
   turso via `WHERE id IN (...)`; scores are zipped back onto the entries.
   Scope filtering is pushed into the Tantivy `BooleanQuery` so no over-fetch
   or Rust-side filter is needed.
3. **Semantic search** (feature `semantic`): if an `Embedder` is wired, the
   query is embedded and `TursoSearcher::search_semantic` issues a
   `vector_distance_cos(embedding, vector32(?))` query against the turso
   `embedding` column, returning cosine similarity scores (1 − distance). BM25
   and semantic searches run concurrently (`tokio::join!`).
4. **RRF fusion**: the full union of BM25 and semantic candidate sets is merged
   via Reciprocal Rank Fusion (k = 60). Entries that appear in both sets get
   additive rank contributions: `score = Σ 1/(k + rank_i)`. This ensures
   entries with no BM25 term overlap can still rank highly via semantic
   similarity, and vice versa.
5. **Recency decay** (`engine::recency_decay`): each fused candidate's score is
   multiplied by `0.5^(age_seconds / half_life)`, where `half_life` is
   `Config::recency_half_life_secs` (default 259,200s / 72h). Non-finite or
   non-positive configured half-lives are clamped to the default in
   `ContextEngine::new`.
6. **Lexicon scoring**: if a `LexiconScorer` is wired, each candidate's score
   is multiplied by `(1.0 + boost.clamp(-1.0, 2.0))` where `boost` is the sum
   of matched term weights, affirmation hits (+0.5 each), and negation hits
   (−0.3 each). Runs in `spawn_blocking` alongside the recency step.
7. **Sort**: candidates are sorted by weighted score descending
   (`f64::total_cmp`, so `NaN` sorts consistently).
8. **Bin-pack**: entries are added greedily until `token_budget` would be
   exceeded. Each entry's token cost is its stored `token_count` if present,
   else `estimate_tokens(content)`. An oversized entry is skipped, not
   aborting — smaller, lower-ranked entries can still be packed.

`scope = None` searches/returns everything regardless of scope (global
recall); `scope = Some(s)` restricts to entries whose `scope` column equals
`s`.

## Schema

There is no migration system. Schema setup is a single idempotent batch run
on every `open_storage`:

```sql
CREATE TABLE IF NOT EXISTS entries (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    kind TEXT NOT NULL,
    scope TEXT,
    session_id TEXT,
    token_count INTEGER,
    metadata TEXT,
    embedding F32_BLOB(384),              -- nullable; populated by embed_and_store
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_entries_timestamp ON entries(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_entries_scope      ON entries(scope);
CREATE INDEX IF NOT EXISTS idx_entries_session_id ON entries(session_id);
```

A turso FTS index (`USING fts`) is also created on the `content` column.
The `embedding` column uses turso's `F32_BLOB(384)` type and is queried via
`vector_distance_cos` — no separate vector store is needed.
No `schema_version` table exists; the `IF NOT EXISTS` guards make setup
safe to re-run on an existing database.

## Trait seams

- **`ContextStorage`** (`traits.rs`): `save`, `get_top_k`, `get_all`,
  `delete`, `clear`, `clear_scope`, `count`. Implemented by `TursoStorage`.
  This is the seam a future alternate backend would implement; `engine.rs`
  never depends on `turso` directly.
- **`Searcher`** (`traits.rs`): `search(query, scope, limit) -> Vec<ScoredEntry>` for BM25 and `search_semantic(embedding, scope, limit) -> Vec<ScoredEntry>` for vector search. Implemented by `TursoSearcher`. An alternate backend would implement this trait without touching `ContextEngine::assemble`'s decay/bin-pack logic.

Both traits require `Send + Sync` so implementations can be shared across
tokio tasks (verified by `context_forge::tests::trait_objects_are_object_safe`
and `context_forge_is_send_sync`).
