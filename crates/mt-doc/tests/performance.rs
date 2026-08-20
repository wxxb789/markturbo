//! Performance characteristics of the document engine.
//!
//! The goal's acceptance target is that the app stays usable at ~10K and ~100K
//! lines. The UI's responsiveness is bounded by how long a reparse takes, since
//! that is the work an edit schedules — so these assert the parse itself, which
//! is the thing that would make the UI unusable if it regressed.
//!
//! Thresholds are deliberately loose. They exist to catch an architectural
//! regression (accidental O(n²), a re-parse per block), not to benchmark a
//! particular machine.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mt_doc::{DocType, Document};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures/perf")
        .join(name)
}

fn read(name: &str) -> String {
    let path = fixture(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {} — run the fixture generator: {e}", path.display()))
}

fn time<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

#[test]
fn parses_a_10k_line_document_quickly() {
    let source = read("large-10k.md");
    assert!(source.lines().count() >= 9_000, "fixture is too small");

    let (doc, elapsed) = time(|| Document::with_type(DocType::Markdown, source.clone()));
    assert!(!doc.blocks().is_empty());
    assert!(
        elapsed < Duration::from_secs(2),
        "10K-line parse took {elapsed:?}; the editor debounce is 180ms, so this \
         would make typing feel broken"
    );
}

#[test]
fn parses_a_100k_line_document_in_bounded_time() {
    let source = read("huge-100k.md");
    assert!(source.lines().count() >= 90_000, "fixture is too small");

    let (doc, elapsed) = time(|| Document::with_type(DocType::Markdown, source.clone()));
    assert!(doc.outline().headings.len() > 1_000);
    // Loose: this is dominated by markdown-rs, and the app never blocks on it
    // (see `schedule_reparse`). The bound exists so a 10x regression fails.
    assert!(
        elapsed < Duration::from_secs(45),
        "100K-line parse took {elapsed:?}"
    );
}

#[test]
fn the_engine_adds_no_superlinear_overhead_over_the_parser() {
    // markdown-rs itself is superlinear in the *number of blocks*: measured on
    // this fixture family, 10x the input costs roughly 70x the time, and that
    // holds with the parser's own default constructs, so it is upstream and not
    // something this crate introduces. (A single huge paragraph — same bytes,
    // one block — scales linearly, which is what localizes it.)
    //
    // What this guards is that the document engine adds no *further* growth on
    // top of the parser. The honest measure is the engine's overhead ratio at
    // each size: if block classification or outline building were quadratic,
    // the overhead at 100K would exceed the overhead at 10K. Comparing
    // growth-ratios instead divides two noisy timings and amplifies jitter into
    // false failures, so measure the overhead directly.
    let small = read("large-10k.md");
    let large = read("huge-100k.md");
    let options = mt_doc::doc::parse_options(DocType::Markdown);

    // Best-of-three: a scheduler hiccup inflates one sample, never all three.
    let best = |mut f: Box<dyn FnMut() -> Duration>| (0..3).map(|_| f()).min().unwrap();

    let parser_small = best(Box::new(|| {
        time(|| markdown::to_mdast(&small, &options).unwrap()).1
    }));
    let engine_small = best(Box::new(|| {
        time(|| Document::with_type(DocType::Markdown, small.clone())).1
    }));
    let parser_large = best(Box::new(|| {
        time(|| markdown::to_mdast(&large, &options).unwrap()).1
    }));
    let engine_large = best(Box::new(|| {
        time(|| Document::with_type(DocType::Markdown, large.clone())).1
    }));

    let overhead_small = engine_small.as_secs_f64() / parser_small.as_secs_f64();
    let overhead_large = engine_large.as_secs_f64() / parser_large.as_secs_f64();

    // Measured at ~1.0x for both, i.e. the engine costs about what its parser
    // costs. A quadratic addition would show up as the large-document overhead
    // climbing well above the small one.
    assert!(
        overhead_large < overhead_small * 2.0 + 0.5,
        "the engine's overhead grows with document size \
         (10K: {overhead_small:.2}x, 100K: {overhead_large:.2}x); \
         something added is superlinear"
    );
}

/// The measured cost of a 100K-line parse, which the UI must never do inline.
///
/// This is the number that justifies `DocumentView::schedule_reparse` running
/// on a background executor: at this scale an inline reparse would freeze the
/// window for seconds on every edit.
#[test]
fn a_huge_document_is_slow_enough_to_require_background_parsing() {
    let source = read("huge-100k.md");
    let (_, elapsed) = time(|| Document::with_type(DocType::Markdown, source));
    assert!(
        elapsed > Duration::from_millis(200),
        "parsing got fast enough ({elapsed:?}) that the background-parse \
         machinery may no longer be needed — re-measure before simplifying"
    );
}

#[test]
fn repeated_edits_do_not_accumulate_cost() {
    // Simulates typing: each keystroke replaces the source and reparses. The
    // Nth edit must not be slower than the first, or the editor degrades as a
    // session goes on.
    let mut source = read("large-10k.md");
    let mut doc = Document::with_type(DocType::Markdown, source.clone());

    let mut first = Duration::ZERO;
    let mut last = Duration::ZERO;
    for i in 0..20 {
        source.push('x');
        let (_, elapsed) = time(|| doc.set_source(source.clone()));
        if i == 0 {
            first = elapsed;
        }
        last = elapsed;
    }

    assert!(
        last < first * 4 + Duration::from_millis(50),
        "the 20th edit ({last:?}) is much slower than the first ({first:?}); \
         state is accumulating across reparses"
    );
}

#[test]
fn setting_identical_source_is_free() {
    // The editor emits Change events that may not alter text; reparsing then
    // is pure waste on a large document.
    let source = read("huge-100k.md");
    let mut doc = Document::with_type(DocType::Markdown, source.clone());

    let (_, elapsed) = time(|| doc.set_source(source.clone()));
    assert!(
        elapsed < Duration::from_millis(100),
        "a no-op set_source took {elapsed:?}; it must short-circuit"
    );
}

#[test]
fn diagram_heavy_document_parses_without_rendering() {
    // Parsing must classify diagram blocks without invoking any renderer:
    // rendering is the view layer's job, on a background task. If parsing
    // rendered, opening this file would take seconds.
    let source = read("diagram-heavy.md");
    let (doc, elapsed) = time(|| Document::with_type(DocType::Markdown, source));

    assert!(
        doc.renderable_blocks().count() >= 150,
        "expected many diagram/math blocks, got {}",
        doc.renderable_blocks().count()
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "parsing a diagram-heavy document took {elapsed:?}; it must not render"
    );
}

#[test]
fn outline_and_block_lookup_are_cheap_on_a_huge_document() {
    let source = read("huge-100k.md");
    let doc = Document::with_type(DocType::Markdown, source);

    // Outline is precomputed, so reading it is O(1).
    let (headings, elapsed) = time(|| doc.outline().headings.len());
    assert!(headings > 1_000);
    assert!(elapsed < Duration::from_millis(10), "outline read {elapsed:?}");

    // Block lookup backs "translate the block at the cursor"; it runs on user
    // interaction, so it must not be a full scan of a huge document per call.
    let (_, elapsed) = time(|| {
        for offset in (0..doc.source().len()).step_by(doc.source().len() / 100) {
            let _ = doc.block_at(offset);
        }
    });
    assert!(
        elapsed < Duration::from_secs(1),
        "100 block lookups took {elapsed:?}"
    );
}
