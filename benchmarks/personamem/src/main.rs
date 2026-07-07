//! PersonaMem Track 1 — deterministic retrieval eval for the lexicon-BENEFIT case.
//!
//! Task: surface the user's *current* preference. Gold = the evidence turn(s)
//! (`related_conversation_snippet`); the 4-way-MC distractors (not used here) are
//! outdated/irrelevant preferences. So higher recall of the gold turn = better
//! importance/salience ranking — exactly what the lexicon is meant to provide,
//! and the complement to LongMemEval (which measures the lexicon's *cost*).
//!
//! Fully deterministic (no reader/judge LLM). Ingests each persona's chat history
//! once, runs all that persona's queries, scores by content match.
//!
//! Usage:
//!   cargo run -p personamem-bench --release -- <benchmark.csv> [flags]
//! Flags:
//!   --data-root PATH    dataset root holding data/chat_history_32k/... (default: CSV's dir)
//!   --pipeline bm25|lexicon|semantic|full   (default bm25)
//!   --clamp F           override Config.lexicon_boost_clamp (lexicon/full only)
//!   --limit N           only the first N personas
//!   --embed-dir PATH    model cache dir for semantic/full (or CF_EMBED_MODEL_DIR)

mod dataset;
mod metrics;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use context_forge::{Config, ContextForge, Embedder, FasEmbedder, SaveOptions};

use dataset::{load_chat_history, load_grouped, Persona};
use metrics::Mean;

const K_VALUES: &[usize] = &[1, 3, 5, 10];
const BUDGETS: &[usize] = &[500, 1000, 2000];
const UNBOUNDED_BUDGET: usize = 100_000_000;
const MSG_KIND: &str = "message";

#[derive(Clone, Copy)]
enum Pipeline {
    Bm25,
    Lexicon,
    Semantic,
    Full,
}

impl Pipeline {
    fn parse(s: &str) -> anyhow::Result<Self> {
        match s {
            "bm25" => Ok(Pipeline::Bm25),
            "lexicon" => Ok(Pipeline::Lexicon),
            "semantic" => Ok(Pipeline::Semantic),
            "full" => Ok(Pipeline::Full),
            other => Err(anyhow::anyhow!("unknown pipeline {other:?}")),
        }
    }
    fn label(self) -> &'static str {
        match self {
            Pipeline::Bm25 => "bm25",
            Pipeline::Lexicon => "lexicon",
            Pipeline::Semantic => "semantic",
            Pipeline::Full => "full",
        }
    }
    fn wants_embedder(self) -> bool {
        matches!(self, Pipeline::Semantic | Pipeline::Full)
    }
    fn wants_lexicon(self) -> bool {
        matches!(self, Pipeline::Lexicon | Pipeline::Full)
    }
}

struct Args {
    csv: PathBuf,
    data_root: PathBuf,
    pipeline: Pipeline,
    clamp: Option<f64>,
    limit: Option<usize>,
    embed_dir: Option<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut csv = None;
    let mut data_root = None;
    let mut pipeline = Pipeline::Bm25;
    let mut clamp = None;
    let mut limit = None;
    let mut embed_dir = std::env::var("CF_EMBED_MODEL_DIR").ok().map(PathBuf::from);

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-root" => data_root = Some(PathBuf::from(next(&mut it, "--data-root")?)),
            "--pipeline" => pipeline = Pipeline::parse(&next(&mut it, "--pipeline")?)?,
            "--clamp" => clamp = Some(next(&mut it, "--clamp")?.parse()?),
            "--limit" => limit = Some(next(&mut it, "--limit")?.parse()?),
            "--embed-dir" => embed_dir = Some(PathBuf::from(next(&mut it, "--embed-dir")?)),
            other if other.starts_with("--") => {
                return Err(anyhow::anyhow!("unknown flag {other:?}"))
            }
            positional => csv = Some(PathBuf::from(positional)),
        }
    }

    let csv = csv.ok_or_else(|| anyhow::anyhow!("missing benchmark.csv path argument"))?;
    // Default data-root = the CSV's parent dir (dataset links are relative to it).
    let data_root = data_root
        .or_else(|| csv.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(Args {
        csv,
        data_root,
        pipeline,
        clamp,
        limit,
        embed_dir,
    })
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
    it.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
}

/// Build a fresh engine for one persona (fresh in-memory DB → per-persona BM25
/// stats + isolation), injecting the shared embedder and configured clamp.
async fn build_forge(
    args: &Args,
    shared_embedder: &Option<Arc<dyn Embedder>>,
) -> anyhow::Result<ContextForge> {
    let mut config = Config::default();
    config.db_path = PathBuf::from(":memory:");
    config.max_entries = 100_000_000;
    if let Some(c) = args.clamp {
        config.lexicon_boost_clamp = c;
    }

    let mut builder = ContextForge::builder(config);
    if args.pipeline.wants_lexicon() {
        builder = builder.with_default_english_scorer();
    }
    if args.pipeline.wants_embedder() {
        let emb = shared_embedder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("this pipeline requires an embedder (--embed-dir)"))?;
        builder = builder.with_embedder(emb);
    }
    Ok(builder.build().await?)
}

#[derive(Default, Clone, Copy)]
struct Timing {
    build: Duration,
    ingest: Duration,
    query: Duration,
}

/// Aggregated recall for a slice of rows.
#[derive(Default)]
struct Agg {
    recall_at: BTreeMap<usize, Mean>,
    recall_budget: BTreeMap<usize, Mean>,
    scored: usize,
    skipped: usize,
}

impl Agg {
    fn record(&mut self, ranked: &[String], budgets: &BTreeMap<usize, HashSet<String>>, gold: &HashSet<String>) {
        for &k in K_VALUES {
            self.recall_at
                .entry(k)
                .or_default()
                .push(metrics::recall_at_k(ranked, gold, k));
        }
        for (&b, set) in budgets {
            self.recall_budget
                .entry(b)
                .or_default()
                .push(metrics::recall_in_set(set, gold));
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let mut personas = load_grouped(&args.csv)?;
    if let Some(n) = args.limit {
        personas.truncate(n);
    }
    let total_rows: usize = personas.iter().map(|p| p.rows.len()).sum();

    eprintln!(
        "PersonaMem retrieval benchmark (Track 1)\n  csv: {}\n  personas: {}  rows: {}\n  pipeline: {}{}",
        args.csv.display(),
        personas.len(),
        total_rows,
        args.pipeline.label(),
        args.clamp.map_or(String::new(), |c| format!("  clamp: {c}")),
    );

    let shared_embedder: Option<Arc<dyn Embedder>> = if args.pipeline.wants_embedder() {
        let dir = args
            .embed_dir
            .clone()
            .ok_or_else(|| anyhow::anyhow!("--embed-dir (or CF_EMBED_MODEL_DIR) required"))?;
        eprintln!("  loading embedding model once from {}...", dir.display());
        Some(Arc::new(FasEmbedder::new(&dir)?))
    } else {
        None
    };

    let mut overall = Agg::default();
    let mut updated = Agg::default(); // preference-evolution slice (the key lexicon signal)
    let mut total = Timing::default();
    let mut errors = 0usize;

    for (i, persona) in personas.iter().enumerate() {
        match run_persona(&args, persona, &shared_embedder, &mut overall, &mut updated).await {
            Ok(t) => {
                total.build += t.build;
                total.ingest += t.ingest;
                total.query += t.query;
            }
            Err(e) => {
                errors += 1;
                eprintln!("  [!] persona {} ({}) failed: {e}", i, persona.persona_id);
            }
        }
        if (i + 1) % 50 == 0 {
            eprintln!("  ...{}/{} personas", i + 1, personas.len());
        }
    }

    print_agg("OVERALL", &overall);
    print_agg("UPDATED (preference-evolution)", &updated);
    println!(
        "\nphase totals: build={:.1}s  ingest={:.1}s  query={:.1}s  errors={errors}",
        total.build.as_secs_f64(),
        total.ingest.as_secs_f64(),
        total.query.as_secs_f64(),
    );
    Ok(())
}

/// Ingest one persona's chat history once, then run all its queries.
async fn run_persona(
    args: &Args,
    persona: &Persona,
    shared_embedder: &Option<Arc<dyn Embedder>>,
    overall: &mut Agg,
    updated: &mut Agg,
) -> anyhow::Result<Timing> {
    let scope = persona.persona_id.as_str();

    let t = Instant::now();
    let forge = build_forge(args, shared_embedder).await?;
    let build = t.elapsed();

    // Ingest every chat-history message as an entry (one batch → one index commit).
    let history = load_chat_history(&args.data_root, &persona.chat_history_link)?;
    let t = Instant::now();
    let items: Vec<(String, String, SaveOptions)> = history
        .chat_history
        .iter()
        .filter(|m| !m.content.trim().is_empty())
        .map(|m| {
            (
                m.content.clone(),
                MSG_KIND.to_owned(),
                SaveOptions {
                    scope: Some(scope.to_owned()),
                    ..Default::default()
                },
            )
        })
        .collect();
    forge.save_batch(&items).await?;
    let ingest = t.elapsed();

    // Run each row's query against this persona's store.
    let t = Instant::now();
    for row in &persona.rows {
        let gold: HashSet<String> = row.gold_contents().into_iter().collect();
        if gold.is_empty() {
            overall.skipped += 1;
            continue;
        }

        let ranked = forge
            .query(&row.user_query, Some(scope), UNBOUNDED_BUDGET)
            .await?;
        let ranked_contents: Vec<String> = ranked.into_iter().map(|e| e.content).collect();

        let mut budgets = BTreeMap::new();
        for &b in BUDGETS {
            let entries = forge.query(&row.user_query, Some(scope), b).await?;
            budgets.insert(b, entries.into_iter().map(|e| e.content).collect::<HashSet<_>>());
        }

        overall.scored += 1;
        overall.record(&ranked_contents, &budgets, &gold);
        if row.is_updated() {
            updated.scored += 1;
            updated.record(&ranked_contents, &budgets, &gold);
        }
    }
    let query = t.elapsed();

    Ok(Timing {
        build,
        ingest,
        query,
    })
}

fn print_agg(name: &str, agg: &Agg) {
    if agg.scored == 0 {
        println!("\n[{name}] no scored rows (skipped={})", agg.skipped);
        return;
    }
    let sampled = agg.recall_at.get(&K_VALUES[0]).map_or(0, Mean::count);
    println!(
        "\n[{name}]  (scored={}, skipped={}, sampled={sampled})",
        agg.scored, agg.skipped
    );
    let recalls: Vec<String> = K_VALUES
        .iter()
        .map(|k| format!("R@{k}={:.3}", agg.recall_at.get(k).map_or(0.0, Mean::get)))
        .collect();
    let budgets: Vec<String> = BUDGETS
        .iter()
        .map(|b| format!("R@{b}tok={:.3}", agg.recall_budget.get(b).map_or(0.0, Mean::get)))
        .collect();
    println!("  {}", recalls.join("  "));
    println!("  {}", budgets.join("  "));
}
