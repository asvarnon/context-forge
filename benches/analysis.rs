//! Benchmarks for the `analysis` module's parallelizable hot paths.
//!
//! # Matrix
//!
//! This bench sweeps a matrix of {10 KB, 100 KB, 1 MB} synthetic transcript
//! sizes x {5, 50, 500} session counts, exercising the three functions that
//! gain a `rayon`-parallel code path under the `parallel` feature:
//!
//! - [`context_forge::analysis::build_session_term_maps`]
//! - [`context_forge::analysis::classify_passages`]
//! - [`context_forge::analysis::score_passages`]
//!
//! Run with and without the `parallel` feature to compare:
//!
//! ```sh
//! cargo bench --bench analysis
//! cargo bench --bench analysis --features parallel
//! ```
//!
//! # Recorded expectations (acceptance documentation, not a gate)
//!
//! - The single-snapshot path (one session, a handful of passages) stays
//!   low-millisecond either way; rayon's per-task overhead can make the
//!   `parallel` build marginally *slower* at this shape because thread
//!   handoff costs more than the work itself.
//! - `parallel` wins on recurrence/batch shapes: many sessions (50-500)
//!   and/or large transcripts (100 KB-1 MB), where `build_session_term_maps`
//!   and `classify_passages` have enough independent per-item work to
//!   amortize the thread-pool overhead.
//! - `score_passages` benefits least, since its per-passage work
//!   (arithmetic + small clones) is cheap relative to tokenization and
//!   classification; expect it to track close to 1x at small/medium sizes
//!   and only show a modest improvement at the largest shapes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use context_forge::analysis::{
    build_session_term_maps, classify_passages, compute_recurrence, score_passages,
    ClassificationConfig, Lexicons, PassageContext, PrefilterConfig, RecurrenceConfig,
    ScoringConfig, Tokenizer, TokenizerConfig,
};

/// Approximate transcript sizes in bytes.
const TRANSCRIPT_SIZES: &[(&str, usize)] = &[
    ("10kb", 10 * 1024),
    ("100kb", 100 * 1024),
    ("1mb", 1024 * 1024),
];

/// Session counts to sweep.
const SESSION_COUNTS: &[usize] = &[5, 50, 500];

/// A small, deterministic vocabulary of sentence templates. Cycling through
/// these (seeded only by index, no RNG) produces formulaic but varied text
/// that exercises tokenization, n-grams, and the classification lexicons
/// (negation, comparison/causal markers, state-change operators).
const SENTENCE_TEMPLATES: &[&str] = &[
    "We switched from Redis to Memcached because latency dropped significantly.",
    "Cache mode was set to writeback after the incident review.",
    "We should not enable verbose logging in production environments.",
    "The deploy pipeline timeout changed to 30 seconds yesterday.",
    "Primary database is now PostgreSQL 16 after the migration completed.",
    "Yes always run cargo test before committing any changes.",
    "General context discussion about implementation details and tradeoffs.",
    "Confirmed the worker pool causes memory growth under sustained load.",
    "The retry policy was updated to use exponential backoff with jitter.",
    "Server IP changed to 10.0.0.2 after the network reconfiguration.",
];

/// Build a single session's transcript content of approximately
/// `target_bytes`, by repeating sentence templates in a deterministic cycle.
fn build_session_content(target_bytes: usize, session_index: usize) -> String {
    let mut content = String::with_capacity(target_bytes + 256);
    let mut template_index = session_index % SENTENCE_TEMPLATES.len();

    while content.len() < target_bytes {
        content.push_str(SENTENCE_TEMPLATES[template_index]);
        content.push(' ');
        template_index = (template_index + 1) % SENTENCE_TEMPLATES.len();
    }

    content
}

/// Build `session_count` sessions, each holding one content string of
/// roughly `transcript_bytes / session_count` bytes (minimum 64 bytes),
/// so the *total* corpus stays close to `transcript_bytes` regardless of
/// session count.
fn build_corpus(transcript_bytes: usize, session_count: usize) -> Vec<String> {
    let per_session = (transcript_bytes / session_count).max(64);
    (0..session_count)
        .map(|index| build_session_content(per_session, index))
        .collect()
}

fn bench_build_session_term_maps(c: &mut Criterion) {
    let tokenizer = Tokenizer::new(&TokenizerConfig::default());
    let prefilter_config = PrefilterConfig::default();

    let mut group = c.benchmark_group("build_session_term_maps");
    for &(size_label, size_bytes) in TRANSCRIPT_SIZES {
        for &session_count in SESSION_COUNTS {
            let corpus = build_corpus(size_bytes, session_count);
            let session_contents: Vec<Vec<&str>> = corpus
                .iter()
                .map(|content| vec![content.as_str()])
                .collect();

            let id = BenchmarkId::from_parameter(format!("{size_label}/{session_count}sess"));
            group.bench_with_input(id, &session_contents, |b, session_contents| {
                b.iter(|| build_session_term_maps(session_contents, &tokenizer, &prefilter_config));
            });
        }
    }
    group.finish();
}

/// Build `PassageContext` values for the classification/scoring benches by
/// reusing the sentence templates directly as passages (one passage per
/// template repetition up to the target byte budget).
fn build_passages(transcript_bytes: usize, session_count: usize) -> Vec<PassageContext> {
    let per_session = (transcript_bytes / session_count).max(64);
    let mut passages = Vec::new();

    for session_index in 0..session_count {
        let mut written = 0usize;
        let mut template_index = session_index % SENTENCE_TEMPLATES.len();
        let mut local_timestamp = 1_700_000_000_i64 + session_index as i64;

        while written < per_session {
            let text = SENTENCE_TEMPLATES[template_index];
            written += text.len();

            passages.push(PassageContext {
                passage_text: text.to_string(),
                triggering_terms: vec!["cache".to_string(), "redis".to_string()],
                session_id: format!("session-{session_index}"),
                timestamp: local_timestamp,
            });

            template_index = (template_index + 1) % SENTENCE_TEMPLATES.len();
            local_timestamp += 1;
        }
    }

    passages
}

fn bench_classify_passages(c: &mut Criterion) {
    let lexicons = Lexicons::default();
    let config = ClassificationConfig::default();

    let mut group = c.benchmark_group("classify_passages");
    for &(size_label, size_bytes) in TRANSCRIPT_SIZES {
        for &session_count in SESSION_COUNTS {
            let passages = build_passages(size_bytes, session_count);

            let id = BenchmarkId::from_parameter(format!("{size_label}/{session_count}sess"));
            group.bench_with_input(id, &passages, |b, passages| {
                b.iter(|| classify_passages(passages, &lexicons, &config));
            });
        }
    }
    group.finish();
}

fn bench_score_passages(c: &mut Criterion) {
    let lexicons = Lexicons::default();
    let classify_config = ClassificationConfig::default();
    let scoring_config = ScoringConfig::default();
    let recurrence_config = RecurrenceConfig::default();
    let tokenizer = Tokenizer::new(&TokenizerConfig::default());
    let prefilter_config = PrefilterConfig::default();
    let now_timestamp = 1_700_100_000_i64;

    let mut group = c.benchmark_group("score_passages");
    for &(size_label, size_bytes) in TRANSCRIPT_SIZES {
        for &session_count in SESSION_COUNTS {
            let passages = build_passages(size_bytes, session_count);
            let classified = classify_passages(&passages, &lexicons, &classify_config);

            // Build a recurrence map from the same corpus so triggering
            // terms resolve to non-zero scores.
            let corpus = build_corpus(size_bytes, session_count);
            let session_contents: Vec<Vec<&str>> = corpus
                .iter()
                .map(|content| vec![content.as_str()])
                .collect();
            let session_term_maps =
                build_session_term_maps(&session_contents, &tokenizer, &prefilter_config);
            let recurrence = compute_recurrence(&session_term_maps, &recurrence_config);
            let recurrence_map = recurrence
                .into_iter()
                .map(|result| (result.term.clone(), result))
                .collect();

            let id = BenchmarkId::from_parameter(format!("{size_label}/{session_count}sess"));
            group.bench_with_input(id, &classified, |b, classified| {
                b.iter(|| {
                    score_passages(classified, &recurrence_map, &scoring_config, now_timestamp)
                });
            });
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_build_session_term_maps,
    bench_classify_passages,
    bench_score_passages
);
criterion_main!(benches);
