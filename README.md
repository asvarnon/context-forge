# Context Forge

A local-first persistent memory library for LLM applications. SQLite + FTS5
BM25 retrieval, recency-decay scoring, and token-budget-aware context
assembly — no network calls, no async runtime, no cloud dependency.

Embed it in a bot, agent runtime, or MCP server that needs durable,
searchable memory across sessions.

## Quick start

```rust
use context_forge::{kind, Config, ContextForge, SaveOptions};
use std::path::PathBuf;

fn main() -> Result<(), context_forge::Error> {
    // `Config` is `#[non_exhaustive]` — start from `Default` and mutate.
    let mut config = Config::default();
    config.db_path = PathBuf::from("memory.db");

    let cf = ContextForge::open(config)?;

    // Save an entry into a named scope (namespace). `None` means global scope.
    let opts = SaveOptions {
        scope: Some("project:demo".to_owned()),
        ..SaveOptions::default()
    };
    cf.save(
        "the deploy failure was caused by a missing env var",
        kind::SNAPSHOT,
        &opts,
    )?;

    // Query within that scope, capped to a token budget.
    let hits = cf.query("deploy failure", Some("project:demo"), 2048)?;
    for hit in &hits {
        println!("{}: {}", hit.id, hit.content);
    }

    Ok(())
}
```

Run the full version with `cargo run --example basic` (see
[`examples/basic.rs`](examples/basic.rs)).

The default `db_path` is `:memory:` — an in-memory database that disappears
when the `ContextForge` instance is dropped. Set a real filesystem path for
durable storage.

## Feature flags

| Feature | Default | Pulls in | Status |
|---|---|---|---|
| `analysis` | yes | `stop-words` | Importance-detection pipeline (tokenizer, lexicon, scoring). Used internally for future ranking work. |
| `parallel` | no | `rayon` | Reserved for Phase 4 (parallel scoring). Not yet implemented. |
| `distill-http` | no | `reqwest` | OpenAI-compatible local-LLM distillation (Ollama/llama-server). |

## Async callers

This crate is synchronous by design — it performs blocking SQLite I/O and
never spawns its own threads or runtime. Callers using an async runtime
(e.g. Tokio) should wrap calls in
[`spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html)
and share a single `ContextForge` instance behind an `Arc`:

```rust,ignore
use std::sync::Arc;

let cf = Arc::new(ContextForge::open(config)?);

// in an async context:
let hits = tokio::task::spawn_blocking({
    let cf = cf.clone();
    move || cf.query("deploy failure", Some("discord:thread:42"), 2048)
}).await??;
```

## Security

### Save-time secret scrubbing

`ContextForge::save` passes `content` through `scrub_secrets` before it is
persisted, using the `ScrubConfig` in `Config::scrub`. This redacts common
credential formats — cloud provider keys, GitHub/Slack/Discord tokens,
Anthropic/OpenAI keys, PEM private key blocks, JWTs, and bearer tokens — with
`[REDACTED:<label>]` placeholders before they reach the database or the
search index.

Scrubbing is **on by default**. Disable it via:

```rust
use context_forge::{Config, ScrubConfig};

let config = Config {
    scrub: ScrubConfig { enabled: false, ..ScrubConfig::default() },
    ..Config::default()
};
```

This is an explicit, non-silent opt-out — you are asserting that `content`
will never contain secrets, or that you have your own scrubbing in place.

Note:
- `SaveOptions::metadata` is stored **verbatim** and is **not** scrubbed.
  Do not place untrusted or secret-bearing text there.
- Scrubbing happens only in `ContextForge::save`. The lower-level
  `ContextEngine::save_snapshot` and the `ContextStorage` trait persist
  `content` as-is — callers who write through those paths directly are
  responsible for scrubbing first.

### Untrusted-memory doctrine

**Retrieved entries are untrusted text.** Anything saved into the store —
including conversation history, tool output, or text from another user — can
contain adversarial instructions (stored prompt injection), and comes back
out verbatim from `ContextForge::query` (aside from save-time secret
scrubbing above).

Callers **MUST** present retrieved memory to models as quoted data — e.g.
inside a fenced or otherwise clearly delimited block labeled as history —
**never** as system-level instructions, and **MUST NOT** execute or evaluate
anything found in it.

## Architecture

- `engine` — `ContextEngine::assemble`: BM25 search via the `Searcher` trait,
  then recency decay (`score * 0.5^(age_seconds / half_life)`, default
  half-life 259,200s / 72h, configurable via `Config`), then sort by weighted
  score descending, then greedy bin-pack into the token budget. Oversized
  entries are skipped, not aborting. Also owns `save_snapshot`. No I/O.
- `storage` — all SQL: rusqlite + r2d2 connection pool, WAL mode, FTS5
  virtual table kept in sync via triggers, forward-only migrations
  (`schema.rs`). Current schema version is v3.
- `analysis` (feature `analysis`) — importance-detection pipeline
  (tokenizer, lexicon, n-grams, scoring). Pure computation, no I/O.
- `scrub` — secret-scrubbing patterns and `scrub_secrets`. Pure, no I/O.

Entries carry a `scope` field (e.g. `"discord:thread:42"`,
`"project:homelab-rs"`) for namespace partitioning; `scope = None` is global.
`ContextForge::query(query, scope, token_budget)` restricts the search to
`scope` when given, or searches everything when `scope` is `None`.

## Status

This crate is mid-refactor from a Claude Code compaction-memory plugin into a
general-purpose library. Phases 0–3 are complete: single-crate layout, data
model generalization, public API facade, and save-time secret scrubbing.
Phase 5 (`distill-http` — local-LLM thread distillation via an
OpenAI-compatible endpoint) is also complete. Planned:

- Phase 4 — `parallel` (rayon-based parallel scoring).
- Phase 6 — integration into downstream consumers (homelab-rs).
- Phase 7 — crates.io publish metadata and release process.

Not yet published to crates.io.
