//! LongMemEval retrieval benchmark for context-forge (Phase 0, Track 1).
//!
//! Measures whether CF's `query` surfaces the gold evidence sessions, using
//! LongMemEval's turn-level annotations. Fully deterministic for the raw-turns
//! ingest path (no reader or judge LLM). Reports Recall@k / NDCG@k (literature-
//! comparable) and Recall@budget (CF's native token-efficiency axis).
//!
//! Usage:
//!   cargo run -p longmemeval-bench --release -- <dataset.json> [flags]
//! Flags:
//!   --pipeline bm25|lexicon|semantic|full   (default bm25)
//!   --ingest   raw|distill                  (default raw)
//!   --limit    N                            (only the first N instances)
//!   --embed-dir PATH                        (model cache dir for semantic/full; or CF_EMBED_MODEL_DIR)

mod dataset;
mod ingest;
mod metrics;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use context_forge::{Config, ContextForge};

use dataset::Instance;
use ingest::Ingest;
use metrics::Mean;

/// Recall/NDCG cutoffs.
const K_VALUES: &[usize] = &[1, 3, 5, 10];
/// Token budgets for the Recall@budget sweep.
const BUDGETS: &[usize] = &[500, 1000, 2000, 4000];
/// Budget used to approximate "return everything ranked" for Recall@k.
const UNBOUNDED_BUDGET: usize = 100_000_000;

#[derive(Clone, Copy)]
enum Pipeline {
    /// `open`: BM25 + recency, no lexicon, no embedder.
    Bm25,
    /// `builder().with_default_english_scorer().build()`: BM25 + lexicon.
    Lexicon,
    /// `builder().with_embedding_model().build()`: BM25 + semantic, no lexicon.
    /// (Expressible now that the English scorer is opt-in.)
    Semantic,
    /// `builder().with_default_english_scorer().with_embedding_model().build()`:
    /// BM25 + semantic + lexicon.
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
}

struct Args {
    dataset: PathBuf,
    pipeline: Pipeline,
    ingest: Ingest,
    limit: Option<usize>,
    embed_dir: Option<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut dataset = None;
    let mut pipeline = Pipeline::Bm25;
    let mut ingest = Ingest::RawTurns;
    let mut limit = None;
    let mut embed_dir = std::env::var("CF_EMBED_MODEL_DIR").ok().map(PathBuf::from);

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pipeline" => {
                pipeline = Pipeline::parse(&next(&mut it, "--pipeline")?)?;
            }
            "--ingest" => {
                ingest = match next(&mut it, "--ingest")?.as_str() {
                    "raw" => Ingest::RawTurns,
                    "distill" => Ingest::DistillPerSession,
                    other => return Err(anyhow::anyhow!("unknown ingest {other:?}")),
                };
            }
            "--limit" => limit = Some(next(&mut it, "--limit")?.parse()?),
            "--embed-dir" => embed_dir = Some(PathBuf::from(next(&mut it, "--embed-dir")?)),
            other if other.starts_with("--") => {
                return Err(anyhow::anyhow!("unknown flag {other:?}"))
            }
            positional => dataset = Some(PathBuf::from(positional)),
        }
    }

    Ok(Args {
        dataset: dataset.ok_or_else(|| anyhow::anyhow!("missing dataset path argument"))?,
        pipeline,
        ingest,
        limit,
        embed_dir,
    })
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> anyhow::Result<String> {
    it.next()
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))
}

/// Build a fresh, empty engine for one instance (fresh in-memory DB → per-
/// instance BM25 statistics and complete isolation).
async fn build_forge(
    pipeline: Pipeline,
    embed_dir: &Option<PathBuf>,
) -> anyhow::Result<ContextForge> {
    // Config is #[non_exhaustive] — build from Default, then set fields.
    let mut config = Config::default();
    config.db_path = PathBuf::from(":memory:");
    config.max_entries = 100_000_000; // never evict during a run
    let require_dir = |embed_dir: &Option<PathBuf>| {
        embed_dir.clone().ok_or_else(|| {
            anyhow::anyhow!("--embed-dir (or CF_EMBED_MODEL_DIR) required for this pipeline")
        })
    };
    let forge = match pipeline {
        Pipeline::Bm25 => ContextForge::open(config).await?,
        Pipeline::Lexicon => {
            ContextForge::builder(config)
                .with_default_english_scorer()
                .build()
                .await?
        }
        Pipeline::Semantic => {
            ContextForge::builder(config)
                .with_embedding_model(require_dir(embed_dir)?)
                .build()
                .await?
        }
        Pipeline::Full => {
            ContextForge::builder(config)
                .with_default_english_scorer()
                .with_embedding_model(require_dir(embed_dir)?)
                .build()
                .await?
        }
    };
    Ok(forge)
}

/// Aggregated metrics for one slice of the dataset (overall or one type).
#[derive(Default)]
struct Agg {
    recall_at: BTreeMap<usize, Mean>,
    ndcg_at: BTreeMap<usize, Mean>,
    recall_budget: BTreeMap<usize, Mean>,
    scored: usize,
    abstention: usize,
}

impl Agg {
    fn record_ranked(&mut self, ranked_sessions: &[String], gold: &HashSet<String>) {
        for &k in K_VALUES {
            self.recall_at
                .entry(k)
                .or_default()
                .push(metrics::recall_at_k(ranked_sessions, gold, k));
            self.ndcg_at
                .entry(k)
                .or_default()
                .push(metrics::ndcg_at_k(ranked_sessions, gold, k));
        }
    }
    fn record_budget(&mut self, budget: usize, sessions: &HashSet<String>, gold: &HashSet<String>) {
        self.recall_budget
            .entry(budget)
            .or_default()
            .push(metrics::recall_in_set(sessions, gold));
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let mut instances = dataset::load(&args.dataset)?;
    if let Some(n) = args.limit {
        instances.truncate(n);
    }

    eprintln!(
        "LongMemEval retrieval benchmark\n  dataset: {}\n  instances: {}\n  pipeline: {}\n  ingest: {}",
        args.dataset.display(),
        instances.len(),
        args.pipeline.label(),
        args.ingest.label(),
    );

    let mut overall = Agg::default();
    let mut by_type: BTreeMap<String, Agg> = BTreeMap::new();
    let mut errors = 0usize;

    for (i, inst) in instances.iter().enumerate() {
        match run_instance(&args, inst).await {
            Ok(result) => accumulate(&mut overall, &mut by_type, inst, result),
            Err(e) => {
                errors += 1;
                eprintln!("  [!] instance {i} ({}) failed: {e}", inst.question_id);
            }
        }
        if (i + 1) % 50 == 0 {
            eprintln!("  ...{}/{} instances", i + 1, instances.len());
        }
    }

    print_report(&overall, &by_type, errors);
    Ok(())
}

/// Per-instance outcome: ranked sessions from the unbounded query, plus the
/// session sets retrieved at each token budget.
struct InstanceResult {
    ranked_sessions: Vec<String>,
    budget_sessions: BTreeMap<usize, HashSet<String>>,
}

async fn run_instance(args: &Args, inst: &Instance) -> anyhow::Result<InstanceResult> {
    let forge = build_forge(args.pipeline, &args.embed_dir).await?;
    let scope = inst.question_id.as_str();
    args.ingest.run(&forge, inst, scope).await?;

    // Unbounded query → ranked session order for Recall@k / NDCG@k.
    let ranked = forge
        .query(&inst.question, Some(scope), UNBOUNDED_BUDGET)
        .await?;
    let ranked_sessions: Vec<String> = ranked.into_iter().filter_map(|e| e.session_id).collect();

    // Budgeted queries → which sessions survive assembly at each budget.
    let mut budget_sessions = BTreeMap::new();
    for &budget in BUDGETS {
        let entries = forge.query(&inst.question, Some(scope), budget).await?;
        let sessions: HashSet<String> = entries.into_iter().filter_map(|e| e.session_id).collect();
        budget_sessions.insert(budget, sessions);
    }

    Ok(InstanceResult {
        ranked_sessions,
        budget_sessions,
    })
}

fn accumulate(
    overall: &mut Agg,
    by_type: &mut BTreeMap<String, Agg>,
    inst: &Instance,
    result: InstanceResult,
) {
    if inst.is_abstention() {
        overall.abstention += 1;
        by_type
            .entry(inst.question_type.clone())
            .or_default()
            .abstention += 1;
        return;
    }
    let gold = inst.gold_sessions();
    let slot = by_type.entry(inst.question_type.clone()).or_default();
    for agg in [&mut *overall, slot] {
        agg.scored += 1;
        agg.record_ranked(&result.ranked_sessions, &gold);
        for (&budget, sessions) in &result.budget_sessions {
            agg.record_budget(budget, sessions, &gold);
        }
    }
}

fn print_report(overall: &Agg, by_type: &BTreeMap<String, Agg>, errors: usize) {
    println!("\n=== Retrieval results ===");
    print_agg("OVERALL", overall);
    for (ty, agg) in by_type {
        let name = if ty.is_empty() {
            "(untyped)"
        } else {
            ty.as_str()
        };
        print_agg(name, agg);
    }
    println!(
        "\nscored={} abstention-excluded={} errors={}",
        overall.scored, overall.abstention, errors
    );
}

fn print_agg(name: &str, agg: &Agg) {
    if agg.scored == 0 {
        println!(
            "\n[{name}] no scored instances (abstention={})",
            agg.abstention
        );
        return;
    }
    println!("\n[{name}]  (n={})", agg.scored);
    let recalls: Vec<String> = K_VALUES
        .iter()
        .map(|k| format!("R@{k}={:.3}", agg.recall_at.get(k).map_or(0.0, Mean::get)))
        .collect();
    let ndcgs: Vec<String> = K_VALUES
        .iter()
        .map(|k| format!("NDCG@{k}={:.3}", agg.ndcg_at.get(k).map_or(0.0, Mean::get)))
        .collect();
    let budgets: Vec<String> = BUDGETS
        .iter()
        .map(|b| {
            format!(
                "R@{b}tok={:.3}",
                agg.recall_budget.get(b).map_or(0.0, Mean::get)
            )
        })
        .collect();
    println!("  {}", recalls.join("  "));
    println!("  {}", ndcgs.join("  "));
    println!("  {}", budgets.join("  "));
}
