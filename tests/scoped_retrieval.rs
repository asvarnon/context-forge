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
async fn scope_starvation_regression() {
    // 120 high-scoring scope-B entries must not crowd out 1 scope-A entry.
    // With the old global-TopDocs + Rust-side filter approach the overfetch ceiling
    // was (limit * 10).max(100) = 100, so the scope-A entry at global rank 121+ was
    // never returned. The BooleanQuery fix makes TopDocs scope-aware, so the scope-A
    // entry is the only candidate and surfaces immediately.
    let config = Config::default();
    let cf = ContextForge::open(config)
        .await
        .expect("open in-memory store");

    for i in 0..120_usize {
        cf.save(
            &format!(
                "starship propulsion research entry {i} covering interstellar starship design"
            ),
            kind::FACT,
            &scoped_options("guild:beta"),
        )
        .await
        .expect("save scope B entry");
    }

    cf.save(
        "starship landing sequence confirmed",
        kind::FACT,
        &scoped_options("guild:alpha"),
    )
    .await
    .expect("save scope A entry");

    let hits = cf
        .query("starship", Some("guild:alpha"), 10)
        .await
        .expect("scoped query");

    assert_eq!(hits.len(), 1, "scope-A entry crowded out by scope-B corpus");
    assert_eq!(hits[0].scope.as_deref(), Some("guild:alpha"));
    assert!(hits[0].content.contains("starship"));
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
