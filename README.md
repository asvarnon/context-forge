# Context Forge

[![crates.io](https://img.shields.io/crates/v/context-forge.svg)](https://crates.io/crates/context-forge)

A local-first persistent memory library for LLM applications. turso (async
SQLite) + standalone Tantivy BM25 retrieval + fastembed semantic search,
recency-decay scoring, and token-budget-aware context assembly — no cloud
dependency, fully async.

Embed it in a bot, agent runtime, or MCP server that needs durable,
searchable memory across sessions.

## What this is

Context Forge is a **deterministic, algorithmic memory layer** — not a language
model, and not a wrapper around one. The query and assembly pipeline runs with no
AI calls:

```
query → BM25 candidate set        (Tantivy, classical information retrieval)
      + semantic candidate set     (fastembed all-MiniLM-L6-v2, cosine similarity)
      → RRF score fusion           (Reciprocal Rank Fusion, k=60, full union)
      → recency decay score        (exponential formula, configurable half-life)
      → lexicon importance         (config-driven heuristics, CPU-only)
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

**Semantic search** (opt-in via the `semantic` feature) adds embedding cosine
similarity as a ranking signal, catching entries that share meaning even when
they share no words. Both BM25 and semantic candidates feed into Reciprocal Rank
Fusion before recency and lexicon scoring — the full union of both result sets is
considered, not just a re-ranking of BM25 results. BM25, recency, and the lexicon
handle explicit memory-intent signals (decisions, commitments, corrections, domain
terms) that semantic similarity is not specifically designed to detect. The layers
are additive.

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

| Feature | Default | Pulls in | Notes |
|---|---|---|---|
| `analysis` | yes | `stop-words` | Importance-detection pipeline — tokenizer, lexicon, n-grams, recurrence, classification, scoring. |
| `parallel` | no | `rayon` | Opt-in rayon parallelism for the `analysis` pipeline. The library never configures the global rayon pool. |
| `distill-http` | no | `reqwest` | OpenAI-compatible local-LLM distillation (Ollama/llama-server). |
| `semantic` | no | `fastembed` | Hybrid BM25 + semantic search via fastembed (all-MiniLM-L6-v2, ONNX Runtime). Downloads ~22 MB model weights on first use. |

## Semantic search

Enable the `semantic` feature to add vector similarity as a ranking signal
alongside BM25. Uses `all-MiniLM-L6-v2` (384-dim, ~22 MB ONNX weights) via
fastembed. Model weights are downloaded automatically on first use to a
configurable cache directory; subsequent startups load from cache.

### Wiring it in

Use `ContextForge::builder` and call `with_embedding_model`:

```rust
use context_forge::{Config, ContextForge};
use std::path::PathBuf;

let mut config = Config::default();
config.db_path = PathBuf::from("memory.db");

// Model weights are cached in the `models/` directory alongside the DB.
let cf = ContextForge::builder(config)
    .with_embedding_model("models/")
    .build()
    .await?;
```

New entries are embedded automatically at save time. `query()` runs both BM25
and semantic search in parallel, fusing results via RRF — no API change needed.

### Backfilling existing entries

If you enable semantic search on a database that already has entries, call
`backfill_embeddings` once at startup to index the pre-existing content:

```rust
let embedded = cf.backfill_embeddings(32, |done, total| {
    eprintln!("backfill: {done}/{total}");
}).await?;
```

`batch_size` controls how many entries are sent to the ONNX model per inference
call. 32 is a good default. The callback receives `(done, total)` after each
batch. Returns the number of entries successfully embedded.

### Runtime note

ONNX inference is CPU-bound and blocking. The library wraps all embedding calls
in `tokio::task::spawn_blocking` — the async runtime is never blocked. The
multi-thread tokio flavor is required when using `semantic` alongside
`distill-http` (both features use blocking tasks internally).

## Lexicon scoring

**Lexicon scoring is opt-in.** By default the engine ranks on relevance only
(BM25, plus semantic when an embedding model is set) — no lexicon layer. Lexicon
scoring applies a *query-independent* importance boost, which is the right signal
for surfacing what matters to a persona but is the wrong signal for pure relevance
retrieval (see [When to enable it](#when-to-enable-it)). You opt in with one or both
of two independent scorers:

- **`with_default_english_scorer()`** — the built-in `DefaultEnglishScorer`, which
  recognizes common English importance signals: confirmations (`"confirmed"`,
  `"that's right"`), importance flags (`"remember this"`, `"key point"`,
  `"deadline"`), decisions (`"we decided"`), commissives (`"i'll fix it"`),
  dismissals (`"never mind"`, `"nvm"`), and self-corrections (`"my mistake"`).
- **`with_persona_scorer(...)`** — a domain **persona lexicon**: a TOML file with
  domain-specific terms, affirmations, and negations for your use case.

The two are **independent** — calling one does not imply the other. Compose whichever
you want:

| Builder calls | Active scorer |
|---|---|
| _(neither)_ | none — relevance only (BM25 + semantic) |
| `.with_default_english_scorer()` | English importance markers only |
| `.with_persona_scorer(p)` | **your persona lexicon only** (no English) |
| both | both, composed additively via `CompositeLexiconScorer` |

A persona lexicon TOML looks like:

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

**Weight semantics.** Inside a scorer, term weights are additive boosts (each
affirmation match adds `+0.5`; each negation subtracts `0.3`; term weights must be
in `(0.0, 1.5]`). The combined `boost` is then applied by the engine as a bounded
multiplier:

```text
final_score = base × (1.0 + boost.clamp(-c, c))
```

where `c` is `Config::lexicon_boost_clamp` — **the relevance ↔ importance dial**,
defaulting to a conservative `0.05`. With the default, even a large raw boost is
capped to a ±5% nudge, so the lexicon acts as a gentle tiebreaker rather than
overriding relevance. `0.0` disables lexicon influence entirely; larger values (e.g.
`0.15`–`0.5`) trade relevance precision for stronger importance weighting.

### When to enable it

Lexicon scoring boosts entries by importance markers **regardless of the query**. On
pure-relevance retrieval benchmarks (see [`benchmarks/`](benchmarks/)), forcing it on
*lowered* recall — importance-*sounding* distractors outranked the actual answer,
because fused relevance scores are tightly compressed and a broad importance
multiplier reorders the top results. That is why it is off by default and bounded by
a conservative clamp.

Enable it when you want ranking to reflect **importance / persona salience**, not just
query relevance (e.g. a persona-driven assistant surfacing what a user cares about).
Keep it off — or the clamp low — for factual recall / question-answering, where
relevance is all that matters. When in doubt, start without it and add it as a
measured change.

### Wiring it in via the builder

```rust
use context_forge::{Config, ConfigLexiconScorer, ContextForge};

let persona: ConfigLexiconScorer = std::fs::read_to_string("lexicon.toml")?
    .parse()?;

let cf = ContextForge::builder(config)
    .with_default_english_scorer()   // optional: English importance markers
    .with_persona_scorer(persona)    // optional: your domain lexicon
    .build()
    .await?;
```

Omit either call to leave that layer out; omit both for relevance-only ranking.
`ContextForge::open` (the lower-level path) wires no scorer at all.

**Toggling at runtime is the caller's job.** context-forge never reads environment
variables — whether to enable each scorer is your application's decision. A common
pattern is to gate the builder calls behind your own config/env flags:

```rust
let mut builder = ContextForge::builder(config);
if std::env::var("CF_ENGLISH_LEXICON").as_deref() == Ok("1") {
    builder = builder.with_default_english_scorer();
}
if let Ok(path) = std::env::var("CF_PERSONA_LEXICON") {
    let persona: ConfigLexiconScorer = std::fs::read_to_string(path)?.parse()?;
    builder = builder.with_persona_scorer(persona);
}
let cf = builder.build().await?;
```

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

**Model quality matters.** The bootstrap prompt requires genuine domain knowledge and
calibrated reasoning about which terms signal memory-worthy content. A small local
model may produce a sparse or poorly-weighted lexicon. If your wired model is weak,
skip the automatic path entirely: copy the prompt template below, substitute your
persona, paste it into Claude / ChatGPT / any capable model in a browser, and save
the TOML response directly to your lexicon file.

<details>
<summary>Bootstrap prompt template (copy, substitute your persona, paste into any LLM)</summary>

```
You are generating a lexicon configuration for a memory importance scoring system.

The AI assistant using this lexicon has the following persona:
<persona>
YOUR PERSONA DESCRIPTION HERE
</persona>

## What this lexicon does

This lexicon teaches a deterministic scoring system which domain-specific terms and phrases
signal "this conversation entry is worth remembering." Entries that score higher survive a
token budget cut and are surfaced in future conversations.

Each entry accumulates a boost:
  - Each matched [terms] entry adds its weight directly to boost
  - Each matched [affirmations] pattern adds +0.5 to boost
  - Each matched [negations] pattern subtracts 0.3 from boost

The engine applies boost as a bounded multiplier on the relevance score
(final_score = base_score × (1.0 + boost.clamp(-c, c))), where c is a caller-configured
strength that is small by default. What matters for this file is therefore the RELATIVE
weight of terms — how important each term is compared to the others — not any absolute
multiplier. Calibrate weights on the scale below.

## Weight calibration

| Range     | Use for                                                                          |
|-----------|----------------------------------------------------------------------------------|
| 0.1–0.4   | Mildly domain-specific. Appears in casual and important content alike.           |
| 0.5–0.8   | Strongly domain-specific. More often in important entries than not.              |
| 0.9–1.5   | Critical term or proper noun. Almost always marks high-value content.            |

Weights must be in (0.0, 1.5]. Never assign a weight above 1.5; the library will
reject any config that does.

## Inclusion rules for [terms]

1. Minimum 4 characters, unless the term is a well-known domain acronym.
2. Prefer precise multi-word phrases over short, ambiguous single words.
3. Memory-value test: include a term ONLY if its presence in an entry makes that entry
   meaningfully more likely to be worth recalling later. Do not include terms merely
   because they sound authentic or in-character for the persona.

## What NOT to include

The system already handles generic English signals ("confirmed", "agreed", "remember this",
"never mind", "my mistake", "incorrect", and similar). Do not repeat them. Only
domain-specific vocabulary and dialect belong in this lexicon.

## [affirmations] — speech act rules

Affirmation patterns must map to one of these speech acts in this persona's dialect:
  - Agreement or confirmation
  - Future commitment or obligation
  - Success or resolution
  - Flagging something as important or worth noting

Aim for 6–12 patterns. Domain-specific dialect only — no generic English.

## [negations] — speech act rules

Negation patterns must map to one of these speech acts in this persona's dialect:
  - Dismissal or disregard
  - Disagreement or correction
  - Failure or rejection

Aim for 4–8 patterns. Domain-specific dialect only — no generic English.

## Output instructions

Think through the calibration internally before writing any output. Reason about which
terms are genuinely high-signal vs. merely in-character, and what speech acts this
persona's dialect uses to express agreement, commitment, dismissal, and failure.

Then output ONLY a single fenced TOML block. No markdown, no prose before or after
the block. Put short rationale as valid TOML inline comments.

\`\`\`toml
# Persona lexicon — generated for context-forge
# Persona: YOUR PERSONA DESCRIPTION HERE

[terms]
"term" = 0.4   # rationale: why this term signals important content

[affirmations]
patterns = [
    "phrase",   # speech act: confirmation
]

[negations]
patterns = [
    "phrase",   # speech act: dismissal
]
\`\`\`
```

</details>

### Growing the lexicon at runtime

The lexicon is a living document. Use `LexiconAppender` to atomically append or
remove entries without corrupting the existing file. All writes use a
write-to-temp-then-rename pattern, so a crash mid-write leaves the original
intact.

```rust
use context_forge::{LexiconAppender, LexiconProposal};
use std::path::PathBuf;

let appender = LexiconAppender::new(PathBuf::from("lexicon.toml"));

// Add or overwrite a term. Rationale is written as a TOML inline comment.
appender.append(&LexiconProposal {
    term: "Battle-Sister".to_owned(),
    weight: 0.7,
    rationale: Some("confirmed important in 7 entries".to_owned()),
    source_ids: vec![],
})?;

// Add affirmation/negation patterns. Both deduplicate case-insensitively.
appender.append_affirmation("it shall be done")?;
appender.append_negation("cogitator returns null")?;

// Remove entries. Terms are case-sensitive identifiers; patterns are not.
appender.remove_term("Battle-Sister")?;
appender.remove_affirmation("IT SHALL BE DONE")?;    // matches regardless of case
appender.remove_negation("Cogitator Returns Null")?; // same
```

All `remove_*` methods are no-ops if the entry is not present.

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

- `engine` — `ContextEngine::assemble`: runs BM25 and semantic search (when
  enabled), fuses candidates via Reciprocal Rank Fusion (k=60, full union),
  then applies recency decay (`0.5^(age_seconds / half_life)`, default
  half-life 259,200s / 72h), then lexicon boost, then greedy bin-pack into the
  token budget. Oversized entries are skipped, not aborting. Also owns
  `save_snapshot`, which triggers embedding generation non-fatally after each
  write. No I/O.
- `storage` — turso (async SQLite) for persistence, standalone Tantivy for
  in-memory BM25 indexing. Dual-write on save: turso commits to disk, tantivy
  updates the in-memory index. On open, the tantivy index is rebuilt from
  turso (linear startup cost, negligible for small corpora). turso is the
  source of truth; tantivy is a derived index. When `semantic` is enabled,
  entries also carry a `vector32` embedding column queried via
  `vector_distance_cos` — no separate vector store needed.
- `semantic` (feature `semantic`) — `Embedder` trait and `FasEmbedder`
  (fastembed + ONNX Runtime). Embedding calls run inside `spawn_blocking`.
- `analysis` (feature `analysis`) — importance-detection pipeline
  (tokenizer, lexicon, n-grams, scoring). Pure computation, no I/O.
- `scrub` — secret-scrubbing patterns and `scrub_secrets`. Pure, no I/O.

Entries carry a `scope` field (e.g. `"discord:thread:42"`,
`"project:homelab-rs"`) for namespace partitioning; `scope = None` is global.
`ContextForge::query(query, scope, token_budget)` restricts the search to
`scope` when given, or searches everything when `scope` is `None`.

## Benchmarks

Retrieval quality is measured, not asserted. [`benchmarks/`](benchmarks/) contains
dev-only harnesses that score whether `query` surfaces the known-relevant evidence,
fully deterministically (no reader/judge LLM):

- **`longmemeval`** — retrieval recall (Recall@k, Recall@budget) on the
  [LongMemEval](https://github.com/xiaowu0162/LongMemEval) long-conversation dataset.
- **`personamem`** — preference-retrieval recall on
  [PersonaMem](https://huggingface.co/datasets/bowen-upenn/PersonaMem-v2).

They isolate each pipeline layer (BM25 · +semantic · +lexicon) against the same gold
labels, and report tokens alongside recall — the token-efficiency axis this library is
built around. These measurements are what motivated lexicon scoring being opt-in with a
conservative default clamp (see [When to enable it](#when-to-enable-it)). Datasets are
fetched separately; see each crate's README.

## Status

All features implemented and tested: single-crate layout, scoped data model,
the `ContextForge` async public API facade, real BM25 scoring via standalone
Tantivy, save-time secret scrubbing, optional rayon parallelism (`parallel`),
local-LLM distillation via an OpenAI-compatible endpoint (`distill-http`), and
hybrid BM25 + semantic search with RRF fusion (`semantic`).

Live-validated against a Discord bot (Husk) across save/recall, BM25 ranking,
restart persistence, scope isolation, secret-scrubbing, and semantic vocabulary-gap
retrieval (queries with zero BM25 term overlap returning the correct entry).

Storage is turso (async SQLite) + standalone Tantivy. All public methods are
`async` — a tokio runtime is required.
