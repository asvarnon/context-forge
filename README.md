# Context Forge

[![crates.io](https://img.shields.io/crates/v/context-forge.svg)](https://crates.io/crates/context-forge)

A local-first persistent memory library for LLM applications. turso (async
SQLite) + standalone Tantivy BM25 retrieval, recency-decay scoring, and
token-budget-aware context assembly — no cloud dependency, fully async.

Embed it in a bot, agent runtime, or MCP server that needs durable,
searchable memory across sessions.

## What this is

Context Forge is a **deterministic, algorithmic memory layer** — not a language
model, and not a wrapper around one. The query and assembly pipeline runs with no
AI calls:

```
query → BM25 candidate set   (Tantivy, classical information retrieval)
      → recency decay score   (exponential formula, configurable half-life)
      → lexicon importance    (config-driven heuristics, CPU-only)
      → [future] semantic similarity  (embedding cosine, CPU-only)
      → token budget cut
      → minimal high-signal context block
```

Every step is deterministic and fast. No randomness, no model inference, no
network calls on the hot path. The goal is to be as **consistent and predictable
as possible without AI input at query time** — a memory layer that sits between
LLM calls rather than depending on them.

The LLM is only involved at `distill_and_save` time: an explicit, amortized call
you opt into when you want to compress a transcript into durable facts. One
distillation produces structured memory retrieved cheaply on every future query.
That asymmetry is intentional — many fast algorithmic retrievals per one
deliberate LLM call.

**Semantic search** (planned) will add embedding cosine similarity as a fourth
ranking signal, catching entries that share meaning even when they share no words.
It complements the pipeline; it does not replace the algorithmic layers. BM25,
recency, and the lexicon handle explicit memory-intent signals (decisions,
commitments, corrections, domain terms) that semantic similarity is not
specifically designed to detect. The layers are additive.

## Installation

Defaults to the latest published version:

```sh
cargo add context-forge
```

To pin an exact version (recommended for production — see the badge above for
the current release):

```sh
cargo add context-forge@=x.y.z
```

## Quick start

```rust
use context_forge::{kind, Config, ContextForge, SaveOptions};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), context_forge::Error> {
    // `Config` is `#[non_exhaustive]` — start from `Default` and mutate.
    let mut config = Config::default();
    config.db_path = PathBuf::from("memory.db");

    let cf = ContextForge::open(config).await?;

    // Save an entry into a named scope (namespace). `None` means global scope.
    let opts = SaveOptions {
        scope: Some("project:demo".to_owned()),
        ..SaveOptions::default()
    };
    cf.save(
        "the deploy failure was caused by a missing env var",
        kind::SNAPSHOT,
        &opts,
    )
    .await?;

    // Query within that scope, capped to a token budget.
    let hits = cf.query("deploy failure", Some("project:demo"), 2048).await?;
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
| `analysis` | yes | `stop-words` | Importance-detection pipeline — tokenizer, lexicon, n-grams, recurrence, classification, scoring. |
| `parallel` | no | `rayon` | Opt-in rayon parallelism for the `analysis` pipeline (per-session term maps, classification, scoring). The library never configures the global rayon pool. |
| `distill-http` | no | `reqwest` | OpenAI-compatible local-LLM distillation (Ollama/llama-server). |

## Lexicon scoring

The library ships an always-on `DefaultEnglishScorer` that recognizes common
English importance signals — confirmations (`"confirmed"`, `"that's right"`),
importance flags (`"remember this"`, `"key point"`, `"deadline"`), decisions
(`"we decided"`, `"final decision"`), commissives (`"i'll fix it"`, `"we committed
to"`), dismissals (`"never mind"`, `"nevermind"`, `"nvm"`), and self-corrections
(`"my mistake"`, `"scratch that"`).

On top of that baseline, callers can inject a **persona lexicon** — a TOML file
with domain-specific terms, affirmations, and negations for their use case:

```toml
# lexicon.toml
[terms]
"Omnissiah" = 0.9   # critical domain proper noun — nearly always high-value content
"Astartes"  = 0.6   # strong domain noun — more often in important entries than not
"bolter"    = 0.3   # mild domain term — appears in casual and important content alike

[affirmations]
patterns = ["for the emperor", "it shall be done", "affirmative, brother"]

[negations]
patterns = ["the emperor frowns upon this", "negative, battle-brother"]
```

**Weight semantics:** term weights are additive boosts. The engine formula is
`final_score = base × (1.0 + boost.clamp(-1.0, 2.0))`, so a weight of `0.3` adds
30% (1.3×); `1.0` doubles the score (2.0×). The engine caps total boost at `2.0`
(3.0× maximum). Weights must be in `(0.0, 1.5]` — the library rejects configs that
exceed this range. Each affirmation match adds a fixed `+0.5`; each negation match
subtracts `0.3`.

### Wiring it in via the builder

Use `ContextForge::builder` to compose the English baseline with your persona lexicon:

```rust
use context_forge::{Config, ConfigLexiconScorer, ContextForge};

let persona: ConfigLexiconScorer = std::fs::read_to_string("lexicon.toml")?
    .parse()?;

let cf = ContextForge::builder(config)
    .with_persona_scorer(persona)
    .build()
    .await?;
```

Without `with_persona_scorer`, the builder still pre-seeds `DefaultEnglishScorer` —
plain-English importance signals are always active. `ContextForge::open` (the
lower-level path) wires no scorer at all.

### Bootstrapping a persona lexicon with an LLM

Writing a well-calibrated lexicon from scratch requires knowing what weight values
mean in practice. The library provides `bootstrap_prompt` to generate a structured
calibration prompt you can pass to any LLM:

```rust
use context_forge::bootstrap_prompt;

let prompt = bootstrap_prompt("A Space Marine Chaplain from Warhammer 40k");
// pass `prompt` to your LLM — the response is a fenced TOML block
// extract the TOML, parse it, and save it to disk
```

The prompt instructs the model on the weight scale, which term lengths and speech
acts are valid, what generic English signals to omit (already covered by the English
baseline), and that rationale should appear as TOML inline comments rather than prose.
The result is a `lexicon.toml` you can load with `ConfigLexiconScorer::from_file`.

This generation happens once at setup time — no LLM call on the query path.

### Growing the lexicon at runtime

The lexicon is a living document. Use `LexiconAppender` to atomically append new
terms discovered at runtime without corrupting the existing file:

```rust
use context_forge::{LexiconAppender, LexiconProposal};

let appender = LexiconAppender::new("lexicon.toml");
appender.append(&LexiconProposal {
    term: "Battle-Sister".to_owned(),
    weight: 0.7,
    rationale: Some("confirmed important in 7 entries".to_owned()),
    source_ids: vec![],
})?;
```

**Platform-specific shorthands** (chat abbreviations like `smh`, `imo`, `mb`) are
intentionally excluded from the English defaults — they are context-specific, not
universal. Add them to your own lexicon file if your user base uses them:

```toml
# abbreviations.toml — load alongside your persona lexicon
[affirmations]
patterns = ["imo", "imho", "ngl", "tbh", "fr"]

[negations]
patterns = ["smh", "mb", "lol no"]
```

## Chunked distillation

`ChunkingDistiller` wraps any `Distiller` and bounds the size of the prompt
sent to the model on each call. A long transcript is split into
budget-sized pieces, each piece is distilled independently, and the partial
results are merged into one `DistilledMemory`:

```rust
use context_forge::{ChunkingDistiller, ReduceStrategy};

let distiller = ChunkingDistiller::new(inner_distiller, max_chunk_chars)
    .with_reduce_strategy(ReduceStrategy::Structural); // the default
```

`max_chunk_chars` is **caller policy** — this crate has no opinion on what a
safe prompt size is for any particular model or host; it only knows how to
split, map, and reduce once given a budget. `ChunkingDistiller` is
model-agnostic (it wraps any `Distiller`, including a hand-rolled one) and
needs no feature flags — it works the same with or without `distill-http`.

`merge_distilled` and `split_on_budget`, the pieces `ChunkingDistiller` is
built from, are also exported directly for callers who want custom
split/merge logic.

See [`examples/chunked_distill.rs`](examples/chunked_distill.rs) for a
runnable, no-network example.

## Runtime requirement

This crate is fully async — all public methods on `ContextForge` return
futures and must be `.await`ed. A **tokio** runtime is required. The
`distill-http` feature additionally requires the multi-thread flavor
(`#[tokio::main]` or `tokio::runtime::Builder::new_multi_thread()`) because
`distill_and_save` uses `tokio::task::block_in_place` internally.

`ContextForge` is `Send + Sync` and can be shared across tasks directly:

```rust,ignore
use std::sync::Arc;

let cf = Arc::new(ContextForge::open(config).await?);

// share across tokio tasks — no spawn_blocking needed
let hits = cf.query("deploy failure", Some("discord:thread:42"), 2048).await?;
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
- `storage` — turso (async SQLite) for persistence, standalone Tantivy for
  in-memory BM25 indexing. Dual-write on save: turso commits to disk, tantivy
  updates the in-memory index. On open, the tantivy index is rebuilt from
  turso (linear startup cost, negligible for small corpora). turso is the
  source of truth; tantivy is a derived index.
- `analysis` (feature `analysis`) — importance-detection pipeline
  (tokenizer, lexicon, n-grams, scoring). Pure computation, no I/O.
- `scrub` — secret-scrubbing patterns and `scrub_secrets`. Pure, no I/O.

Entries carry a `scope` field (e.g. `"discord:thread:42"`,
`"project:homelab-rs"`) for namespace partitioning; `scope = None` is global.
`ContextForge::query(query, scope, token_budget)` restricts the search to
`scope` when given, or searches everything when `scope` is `None`.

## Status

All features implemented and tested: single-crate layout, scoped data model,
the `ContextForge` async public API facade, real BM25 scoring via standalone
Tantivy, save-time secret scrubbing, optional rayon parallelism (`parallel`),
and local-LLM distillation via an OpenAI-compatible endpoint (`distill-http`).

Live-validated against a Discord bot (Husk) across save/recall, BM25 ranking,
restart persistence, scope isolation, and secret-scrubbing test scenarios.

Storage is turso (async SQLite) + standalone Tantivy. All public methods are
`async` — a tokio runtime is required.
