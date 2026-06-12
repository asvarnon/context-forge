//! Integration test for Phase 3 secret scrubbing: proves that a secret
//! present in `save`-d content never reaches the on-disk `SQLite` database
//! (table or FTS index), and that the opt-out path stores it verbatim.

use context_forge::{kind, Config, ContextForge, SaveOptions};
use rusqlite::Connection;

/// A syntactically valid (fake) GitHub personal access token: `ghp_` + 36
/// alphanumeric characters.
fn fake_github_token() -> String {
    format!("ghp_{}", "a".repeat(36))
}

#[test]
fn github_token_is_scrubbed_before_persistence() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("scrub-test.db");

    let token = fake_github_token();
    let content = format!("here is my token: {token} please remember it");

    {
        let mut config = Config::default();
        config.db_path = db_path.clone();
        let cf = ContextForge::open(config).expect("open db");
        cf.save(&content, kind::MANUAL, &SaveOptions::default())
            .expect("save scrubbed entry");
    }

    // Re-open the raw SQLite file directly, bypassing the library entirely.
    let conn = Connection::open(&db_path).expect("open raw sqlite connection");

    // (a) No stored content matches the raw token string.
    let entries_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries WHERE content LIKE ?1",
            [format!("%{token}%")],
            |row| row.get(0),
        )
        .expect("query entries table");
    assert_eq!(
        entries_match, 0,
        "raw GitHub token must not appear in the entries table"
    );

    // (b) The stored content contains the redaction placeholder.
    let redacted_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries WHERE content LIKE '%[REDACTED:github-token]%'",
            [],
            |row| row.get(0),
        )
        .expect("query entries table for redaction marker");
    assert_eq!(
        redacted_match, 1,
        "entries table must contain the [REDACTED:github-token] placeholder"
    );

    // (c) The FTS index does not contain the raw token either.
    let fts_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries_fts WHERE content LIKE ?1",
            [format!("%{token}%")],
            |row| row.get(0),
        )
        .expect("query entries_fts table");
    assert_eq!(
        fts_match, 0,
        "raw GitHub token must not appear in the FTS index"
    );

    // An FTS MATCH on a fragment of the token must return nothing.
    let fts_match_query_result = conn
        .query_row(
            "SELECT COUNT(*) FROM entries_fts WHERE entries_fts MATCH ?1",
            [format!("{token}*")],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    assert_eq!(
        fts_match_query_result, 0,
        "FTS MATCH on the raw token must return no rows"
    );
}

#[test]
fn opt_out_stores_token_verbatim() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("scrub-opt-out.db");

    let token = fake_github_token();
    let content = format!("here is my token: {token} please remember it");

    {
        let mut config = Config::default();
        config.db_path = db_path.clone();
        config.scrub.enabled = false;
        let cf = ContextForge::open(config).expect("open db");
        cf.save(&content, kind::MANUAL, &SaveOptions::default())
            .expect("save unscrubbed entry");
    }

    let conn = Connection::open(&db_path).expect("open raw sqlite connection");

    let entries_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries WHERE content LIKE ?1",
            [format!("%{token}%")],
            |row| row.get(0),
        )
        .expect("query entries table");
    assert_eq!(
        entries_match, 1,
        "with scrubbing disabled, the raw token must be stored verbatim"
    );

    let redacted_match: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM entries WHERE content LIKE '%[REDACTED:github-token]%'",
            [],
            |row| row.get(0),
        )
        .expect("query entries table for redaction marker");
    assert_eq!(
        redacted_match, 0,
        "with scrubbing disabled, no redaction placeholder should be present"
    );
}
