//! Where does opening a large folder spend its time?
//!
//! Ignored by default: this measures against whatever real directory the
//! `MARKTURBO_BENCH_DIR` variable names, so it is a diagnostic to run by hand
//! rather than a gate. Run it with:
//!
//! ```sh
//! MARKTURBO_BENCH_DIR=Q:/repos/thoughtscape/ob-flow \
//!   cargo test -p mt-app --test open_folder_cost -- --ignored --nocapture
//! ```
//!
//! The point is to attribute the stall to one phase rather than guessing, since
//! opening a folder does four separable things: reads the tree, starts a
//! recursive filesystem watch, discovers skills, and discovers instruction
//! files.

use std::path::PathBuf;
use std::time::Instant;

fn bench_dir() -> Option<PathBuf> {
    let value = std::env::var("MARKTURBO_BENCH_DIR").ok()?;
    let path = PathBuf::from(value.trim());
    path.is_dir().then_some(path)
}

#[test]
#[ignore = "measures a real directory named by MARKTURBO_BENCH_DIR"]
fn attribute_the_cost_of_opening_a_folder() {
    let Some(root) = bench_dir() else {
        eprintln!("set MARKTURBO_BENCH_DIR to a directory to measure");
        return;
    };
    eprintln!("measuring {}", root.display());

    let time = |label: &str, f: &mut dyn FnMut() -> String| {
        let start = Instant::now();
        let detail = f();
        eprintln!("{:>10.0?}  {label}  {detail}", start.elapsed());
    };

    time("read_dir (depth 0)", &mut || {
        let nodes = mt_app::workspace::read_dir(&root).unwrap_or_default();
        format!("{} entries", nodes.len())
    });

    time("read_dir (depth 1)", &mut || {
        let nodes = mt_app::workspace::read_dir_deep(&root, 1).unwrap_or_default();
        let children: usize = nodes.iter().map(|n| n.children.len()).sum();
        format!("{} entries, {children} children", nodes.len())
    });

    time("Watcher::new (recursive)", &mut || {
        match mt_app::watcher::Watcher::new(&root) {
            Ok(w) => {
                // Hold it until the timer stops: dropping early would measure
                // setup without the registration it performs.
                let _ = w.poll();
                "ok".to_string()
            }
            Err(err) => format!("failed: {err}"),
        }
    });

    time("skill::discover (workspace)", &mut || {
        format!("{} skills", mt_doc::skill::discover(&root).len())
    });

    time("skill::discover_with (global)", &mut || {
        let found = mt_doc::skill::discover_with(&root, mt_doc::Discovery::everything());
        format!("{} skills", found.len())
    });

    time("instruction::discover", &mut || {
        format!("{} files", mt_doc::instruction::discover(&root).len())
    });

    time("instruction::discover_with (global)", &mut || {
        format!(
            "{} files",
            mt_doc::instruction::discover_with(&root, true).len()
        )
    });
}
