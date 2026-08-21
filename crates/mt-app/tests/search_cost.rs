//! What a workspace search actually costs, per scope.
//!
//! `#[ignore]` because it walks real directories and reports timings rather than
//! asserting a bound — a machine-dependent number in the gate would be a flaky
//! test. Run it when changing the search path:
//!
//! ```sh
//! cargo test --release -p mt-app --test search_cost -- --ignored --nocapture
//! ```
//!
//! Point it at a real vault with `MARKTURBO_BENCH_DIR`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mt_doc::search::{self, Query, Results};

fn bench_dir() -> PathBuf {
    std::env::var("MARKTURBO_BENCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sample"))
}

#[test]
#[ignore = "reports timings against a real directory; run explicitly"]
fn attribute_the_cost_of_a_folder_search() {
    let root = bench_dir();
    if !root.is_dir() {
        eprintln!("no such directory: {}", root.display());
        return;
    }

    let started = Instant::now();
    let paths = search::document_paths(&root);
    let walk = started.elapsed();
    println!("walk:  {:>8.1?}  ({} documents)", walk, paths.len());

    // Three needles with very different hit counts: the cap is what bounds the
    // worst case, so a common word and a rare one should not differ by orders
    // of magnitude in wall clock.
    for needle in ["the", "workspace", "zzzz-no-such-token"] {
        let mut out = Results::default();
        let started = Instant::now();
        search::search_files(&paths, &Query::new(needle), search::DEFAULT_LIMIT, &mut out);
        let elapsed = started.elapsed();
        println!(
            "query {needle:>20}: {:>8.1?}  {} match(es) in {} file(s){}",
            elapsed,
            out.matches.len(),
            out.files,
            if out.truncated { " [capped]" } else { "" }
        );
    }
}

#[test]
#[ignore = "reports timings against the real harness; run explicitly"]
fn attribute_the_cost_of_a_harness_search() {
    let started = Instant::now();
    let skills = mt_doc::skill::discover_with(Path::new("."), mt_doc::Discovery::everything());
    let discover = started.elapsed();

    let started = Instant::now();
    let mut paths: Vec<PathBuf> = Vec::new();
    for skill in &skills {
        paths.extend(search::document_paths(&skill.dir));
    }
    paths.sort();
    paths.dedup();
    let walk = started.elapsed();
    println!(
        "discover: {discover:>8.1?}  ({} skills)\nwalk:     {walk:>8.1?}  ({} documents)",
        skills.len(),
        paths.len()
    );

    let mut out = Results::default();
    let started = Instant::now();
    search::search_files(
        &paths,
        &Query::new("skill"),
        search::DEFAULT_LIMIT,
        &mut out,
    );
    println!(
        "query:    {:>8.1?}  {} match(es) in {} file(s){}",
        started.elapsed(),
        out.matches.len(),
        out.files,
        if out.truncated { " [capped]" } else { "" }
    );
}

/// What the cap actually saves on a real vault.
///
/// The interesting number is the *rare* query: it finds nothing, so nothing
/// stops it early, and it therefore pays the full walk plus a read of every
/// document. That is the worst case a user can produce by typing, and it is
/// the one worth watching — a common word exits at the cap almost immediately.
#[test]
#[ignore = "reports timings against a real directory; run explicitly"]
fn the_cap_bounds_the_filesystem_work_not_only_the_list() {
    let root = bench_dir();
    if !root.is_dir() {
        eprintln!("no such directory: {}", root.display());
        return;
    }

    for needle in ["the", "zzzz-no-such-token"] {
        let mut out = Results::default();
        let started = Instant::now();
        let walked = search::document_paths(&root);
        let walk = started.elapsed();
        search::search_files(
            &walked,
            &Query::new(needle),
            search::DEFAULT_LIMIT,
            &mut out,
        );
        println!(
            "{needle:>20}: walk {walk:>7.1?}  total {:>7.1?}  {} match(es){}",
            started.elapsed(),
            out.matches.len(),
            if out.truncated { " [capped]" } else { "" }
        );
    }
}
