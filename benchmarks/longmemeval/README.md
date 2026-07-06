# LongMemEval retrieval benchmark (Phase 0)

Dev-only harness measuring whether context-forge's `query` surfaces the gold
evidence sessions for each LongMemEval question. Not published; excluded from
the release gate.

## What it measures

**Track 1 — retrieval, fully deterministic (raw-turns ingest, no LLM):**
- **Recall@k / NDCG@k** (k = 1, 3, 5, 10) — literature-comparable, scored against
  LongMemEval's gold `answer_session_ids`.
- **Recall@budget** (500 / 1000 / 2000 / 4000 tokens) — CF's native axis: did the
  evidence survive token-budgeted assembly? This is the token-efficiency story.

Both map cleanly because CF entries carry a native `session_id`, set to the
haystack session id at ingest and returned on every `ContextEntry`.

## Get the dataset (one-time, not committed)

The data is **not in the GitHub repo** — it's on Hugging Face
(`xiaowu0162/longmemeval-cleaned`; the original `xiaowu0162/longmemeval` is
deprecated). Files are plain JSON, public, no auth. Drop them in `data/`:

| File | Size | Use |
|---|---|---|
| `longmemeval_oracle.json` | 14 MB | Evidence-only sanity check (no distractors — retrieval is near-trivial; validates the harness, not retrieval quality). |
| `longmemeval_s_cleaned.json` | 264 MB | **The real baseline** — ~40 sessions/instance with distractors, 500 Qs. |
| `longmemeval_m_cleaned.json` | 2.6 GB | ~500 sessions/instance. Skip unless stress-testing. |

```bash
cd benchmarks/longmemeval/data
base=https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main
curl -L -O "$base/longmemeval_oracle.json"       # 14 MB
curl -L -O "$base/longmemeval_s_cleaned.json"    # 264 MB
```

> **Schema verified 2026-07-06** against the real oracle file: instance and turn
> keys match `src/dataset.rs` exactly. One quirk handled — `answer` is sometimes
> an integer/list, not a string, so it is typed as `serde_json::Value`.

## Run

```bash
# Deterministic baseline (build/validate this first):
cargo run -p longmemeval-bench --release -- benchmarks/longmemeval/data/longmemeval_s_cleaned.json

# Quick smoke test on 10 instances:
cargo run -p longmemeval-bench --release -- .../longmemeval_s_cleaned.json --limit 10

# Pipeline matrix (isolates each layer's contribution):
#   bm25    = ContextForge::open        (BM25 + recency only)
#   lexicon = builder().build()         (+ DefaultEnglishScorer)
#   full    = builder + embedding model (+ semantic; needs --embed-dir)
cargo run -p longmemeval-bench --release -- .../longmemeval_s.json --pipeline lexicon
cargo run -p longmemeval-bench --release -- .../longmemeval_s.json --pipeline full --embed-dir ./models

# Distilled ingest (needs an OpenAI-compatible endpoint; start on _oracle):
LLM_BASE_URL=http://localhost:11434/v1 LLM_MODEL=llama3.1 \
  cargo run -p longmemeval-bench --release -- .../longmemeval_oracle.json --ingest distill
```

## Known caveats (read before interpreting numbers)

- **Recency is neutralized.** CF stamps entries at ingest wall-clock, not the
  message's real date (`save` takes no timestamp). All of an instance's turns are
  saved near-simultaneously, so recency decay is ~uniform and contributes no
  signal here. Read `temporal-reasoning` results with this in mind. (A possible
  API note: allow a caller-supplied timestamp on save.)
- **Semantic runs are slow.** A shared loaded embedder can't be injected across
  the fresh-per-instance engines yet (needs the public `Embedder` trait — roadmap
  item 8), so `--pipeline full` reloads the model per instance. Correct, just slow;
  strengthens the case for item 8.
- **`full` also carries lexicon.** The builder always seeds `DefaultEnglishScorer`,
  so there is no public "BM25 + semantic, no lexicon" configuration. The isolable
  deltas are bm25→lexicon (lexicon's contribution) and lexicon→full (semantic's).
  Another data point for the Phase A API audit (let the builder opt out of the
  default scorer).
