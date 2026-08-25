//! Where does clicking a file in the tree spend its time?
//!
//! Ignored by default: it measures real documents, so it is a diagnostic to run
//! by hand rather than a gate. Run it with:
//!
//! ```sh
//! cargo test -p mt-app --test open_document_cost -- --ignored --nocapture
//! MARKTURBO_BENCH_FILE=Q:/some/big.md \
//!   cargo test -p mt-app --test open_document_cost -- --ignored --nocapture
//! ```
//!
//! The point is to attribute the click-to-preview delay to one phase. Opening a
//! document does four separable things: reads the file, parses it into the
//! document model, hands the text to the editor, and renders every diagram
//! fence. Only the last is unbounded, and only measuring says which dominates.

use std::path::{Path, PathBuf};
use std::time::Instant;

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("sample")
}

fn documents() -> Vec<PathBuf> {
    if let Ok(value) = std::env::var("MARKTURBO_BENCH_FILE") {
        let path = PathBuf::from(value.trim());
        if path.is_file() {
            return vec![path];
        }
    }
    let mut found = Vec::new();
    collect(&sample_dir(), &mut found);
    found.sort();
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if mt_doc::DocType::of(&path).is_document() {
            out.push(path);
        }
    }
}

/// What the first formula in a process costs, against every one after it.
///
/// This used to measure MathJax's JS engine starting up — 792ms cold against
/// ~146ms warm, which is why `main.rs` warmed it on a thread before the window
/// opened. RaTeX has no engine, so the only first-call cost left is reading and
/// parsing the nineteen KaTeX faces, and the warm-up is gone with it.
///
/// Still worth measuring, because it is what justifies *not* having a warm-up:
/// if this ever grows back to hundreds of milliseconds, deferring it becomes a
/// question again.
///
/// Run it alone — anything else in the same process that renders math loads the
/// faces first:
///
/// ```sh
/// cargo test --release -p mt-app --test open_document_cost \
///   first_formula -- --ignored --nocapture
/// ```
#[test]
#[ignore = "must run in a process where nothing has rendered math yet"]
fn first_formula_costs_little_more_than_the_rest() {
    let registry = mt_app::renderer::RendererRegistry::with_defaults();

    let start = Instant::now();
    let first = registry.render("math", "a^2 + b^2 = c^2");
    let cold = start.elapsed();

    // A different formula, so the result cache cannot serve it.
    let start = Instant::now();
    let _ = registry.render("math", "\\int_0^1 x\\,dx");
    let warm = start.elapsed();

    println!("first {cold:.1?}  subsequent {warm:.1?}");
    println!(
        "loading the faces costs {:.1?} once",
        cold.saturating_sub(warm)
    );
    if first.diagnostic().is_some() {
        println!(
            "NOTE: math is unavailable here, so this measured the diagnostic \
             path rather than a render — {:?}",
            first.diagnostic().map(|d| &d.message)
        );
    }
}

#[test]
#[ignore = "a diagnostic over real documents, not a gate"]
fn attribute_the_cost_of_opening_a_document() {
    let registry = mt_app::renderer::RendererRegistry::with_defaults();

    println!("    load    parse   render    total  document");

    for path in documents() {
        let start = Instant::now();
        let Ok(file) = mt_app::fs::load(&path) else {
            continue;
        };
        let load = start.elapsed();

        let start = Instant::now();
        let document = mt_doc::Document::new(Some(path.clone()), file.text.clone());
        let parse = start.elapsed();

        // What the native preview's background parse does per diagram fence.
        // Cached after the first call, so this measures a cold open — which is
        // the one the user waits through.
        let start = Instant::now();
        let mut fences = 0usize;
        let mut slowest = ("", std::time::Duration::ZERO);
        for block in document.renderable_blocks() {
            let Some(id) = block.renderer_id() else {
                continue;
            };
            let one = Instant::now();
            let _ = registry.render(id, &block.content);
            let one = one.elapsed();
            if one > slowest.1 {
                slowest = (id, one);
            }
            fences += 1;
        }
        let render = start.elapsed();

        let total = load + parse + render;
        println!(
            "{:>8.1?} {:>8.1?} {:>8.1?} {:>8.1?}  {} ({} bytes, {fences} fences, \
             slowest {} {:.1?})",
            load,
            parse,
            render,
            total,
            path.file_name().unwrap_or_default().to_string_lossy(),
            file.text.len(),
            slowest.0,
            slowest.1,
        );
    }
}
