//! Integration test for Phase 3 secret scrubbing: proves that a secret
//! present in `save`-d content never reaches the on-disk database (entries
//! table or FTS index), and that the opt-out path stores it verbatim.
//!
//! Previously this test used a raw rusqlite connection to bypass the library.
//! With the turso backend the on-disk file contains a `USING fts` index that
//! is incompatible with rusqlite's schema parser, so we now verify via
//! ContextForge's storage APIs, which read the raw stored content directly
//! (no read-time scrubbing is applied).

use context_forge::{kind, Config, ContextForge, SaveOptions, MATCH_ALL_QUERY};

/// A syntactically valid (fake) GitHub personal access token: `ghp_` + 36
/// alphanumeric characters.
fn fake_github_token() -> String {
    format!("ghp_{}", "a".repeat(36))
}

#[tokio::test]
async fn github_token_is_scrubbed_before_persistence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("scrub-test.db");

    let token = fake_github_token();
    let content = format!("here is my token: {token} please remember it");

    let mut config = Config::default();
    config.db_path = db_path.clone();
    let cf = ContextForge::open(config).await.expect("open db");
    cf.save(&content, kind::MANUAL, &SaveOptions::default())
        .await
        .expect("save scrubbed entry");

    // (a) The stored content does not contain the raw token.
    // get_all() reads directly from the storage layer without any read-time
    // scrubbing, so this is equivalent to inspecting the raw database rows.
    let all: Vec<_> = cf
        .query(MATCH_ALL_QUERY, None, 1000)
        .await
        .expect("get all entries");
    assert_eq!(all.len(), 1, "should have exactly one entry");
    let stored = &all[0].content;
    assert!(
        !stored.contains(token.as_str()),
        "raw GitHub token must not appear in stored content: {stored:?}"
    );

    // (b) The stored content contains the redaction placeholder.
    assert!(
        stored.contains("[REDACTED:github-token]"),
        "stored content must contain the [REDACTED:github-token] placeholder: {stored:?}"
    );

    // (c) FTS search must not surface the raw token.
    let fts_hits = cf
        .query(&token[..8], None, 100)
        .await
        .expect("fts query for token fragment");
    assert!(
        fts_hits.is_empty(),
        "FTS search must not find the raw GitHub token"
    );
}

#[tokio::test]
async fn opt_out_stores_token_verbatim() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("scrub-opt-out.db");

    let token = fake_github_token();
    let content = format!("here is my token: {token} please remember it");

    let mut config = Config::default();
    config.db_path = db_path.clone();
    config.scrub.enabled = false;
    let cf = ContextForge::open(config).await.expect("open db");
    cf.save(&content, kind::MANUAL, &SaveOptions::default())
        .await
        .expect("save unscrubbed entry");

    let all: Vec<_> = cf
        .query(MATCH_ALL_QUERY, None, 1000)
        .await
        .expect("get all entries");
    assert_eq!(all.len(), 1, "should have exactly one entry");
    let stored = &all[0].content;

    assert!(
        stored.contains(token.as_str()),
        "with scrubbing disabled, the raw token must be stored verbatim: {stored:?}"
    );
    assert!(
        !stored.contains("[REDACTED:github-token]"),
        "with scrubbing disabled, no redaction placeholder should be present: {stored:?}"
    );
}
