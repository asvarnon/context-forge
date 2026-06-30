//! Integration test for Phase 2 scoped retrieval: saving to distinct scopes,
//! querying within a scope, global (`None`) recall, and per-scope clearing.

use context_forge::{kind, Config, ContextForge, SaveOptions};

fn scoped_options(scope: &str) -> SaveOptions {
    SaveOptions {
        scope: Some(scope.to_owned()),
        ..SaveOptions::default()
    }
}

#[tokio::test]
async fn scoped_save_and_query_do_not_cross_contaminate() {
    let config = Config::default();
    let cf = ContextForge::open(config)
        .await
        .expect("open in-memory store");

    cf.save(
        "alpha project deploy notes",
        kind::SNAPSHOT,
        &scoped_options("project:alpha"),
    )
    .await
    .expect("save scope A entry");
    cf.save(
        "beta project deploy notes",
        kind::SNAPSHOT,
        &scoped_options("project:beta"),
    )
    .await
    .expect("save scope B entry");

    // Query scope A returns only A's entries.
    let alpha_hits = cf
        .query("deploy notes", Some("project:alpha"), 1000)
        .await
        .expect("query scope A");
    assert_eq!(alpha_hits.len(), 1);
    assert_eq!(alpha_hits[0].scope.as_deref(), Some("project:alpha"));
    assert!(alpha_hits[0].content.contains("alpha"));

    // Query scope B returns only B's entries.
    let beta_hits = cf
        .query("deploy notes", Some("project:beta"), 1000)
        .await
        .expect("query scope B");
    assert_eq!(beta_hits.len(), 1);
    assert_eq!(beta_hits[0].scope.as_deref(), Some("project:beta"));
    assert!(beta_hits[0].content.contains("beta"));

    // Query with no scope sees both.
    let global_hits = cf
        .query("deploy notes", None, 1000)
        .await
        .expect("global query");
    assert_eq!(global_hits.len(), 2);

    // clear_scope(A) removes only A's entries.
    let cleared = cf
        .clear_scope("project:alpha")
        .await
        .expect("clear scope A");
    assert_eq!(cleared, 1);

    let remaining = cf
        .query("deploy notes", None, 1000)
        .await
        .expect("global query after clear");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].scope.as_deref(), Some("project:beta"));

    assert_eq!(cf.count().await.expect("count"), 1);
}

#[tokio::test]
async fn match_all_query_respects_scope() {
    let config = Config::default();
    let cf = ContextForge::open(config)
        .await
        .expect("open in-memory store");

    cf.save("entry one", kind::MANUAL, &scoped_options("scope-a"))
        .await
        .expect("save A");
    cf.save("entry two", kind::MANUAL, &scoped_options("scope-b"))
        .await
        .expect("save B");
    cf.save("entry three", kind::MANUAL, &SaveOptions::default())
        .await
        .expect("save global");

    let scope_a = cf
        .query(context_forge::MATCH_ALL_QUERY, Some("scope-a"), 1000)
        .await
        .expect("query scope a match-all");
    assert_eq!(scope_a.len(), 1);
    assert_eq!(scope_a[0].content, "entry one");

    let everything = cf
        .query(context_forge::MATCH_ALL_QUERY, None, 1000)
        .await
        .expect("global match-all");
    assert_eq!(everything.len(), 3);
}
