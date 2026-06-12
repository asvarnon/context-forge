//! 30-second tour of the `context-forge` API.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example basic
//! ```

use context_forge::{kind, Config, ContextForge, SaveOptions};
use std::path::PathBuf;

fn main() -> Result<(), context_forge::Error> {
    // `Config` is `#[non_exhaustive]` — start from `Default` and mutate.
    let mut config = Config::default();
    config.db_path = PathBuf::from("basic-example.db");

    let cf = ContextForge::open(config)?;

    // Save an entry into a named scope (namespace). `None` would mean "global".
    let opts = SaveOptions {
        scope: Some("project:demo".to_owned()),
        ..SaveOptions::default()
    };
    cf.save(
        "the deploy failure was caused by a missing env var",
        kind::SNAPSHOT,
        &opts,
    )?;

    // Query within that scope, capped to a token budget.
    let hits = cf.query("deploy failure", Some("project:demo"), 2048)?;
    for hit in &hits {
        println!("{}: {}", hit.id, hit.content);
    }

    Ok(())
}
