//! Block renderer registry.
//!
//! Diagram and math blocks are rendered out-of-band into SVG, which the native
//! path displays via `gpui::Image::from_bytes(ImageFormat::Svg, …)` and the
//! WebView embeds directly. A renderer is looked up by the block's
//! `renderer_id()`, so adding one is a registration — the Markdown parser and
//! the view code do not change.
//!
//! Three of the four required technologies (Mermaid, D2, LaTeX) render in pure
//! Rust with no external dependency, so they are always available. PlantUML has
//! no usable pure-Rust implementation and falls back to a locally installed
//! binary; when it is absent the block gets an install hint, never a crash.
//!
//! Every renderer is fallible by design. A missing tool or a syntax error
//! produces a [`RenderOutcome::Failed`] carrying a diagnostic; nothing panics,
//! and the original source is always preserved for display.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

use mt_doc::Diagnostic;

/// What a renderer produced.
#[derive(Debug, Clone)]
pub enum RenderOutcome {
    /// SVG markup, displayable by both the native and WebView paths.
    Svg(String),
    /// The renderer could not run or the source was invalid. The block falls
    /// back to showing its source with this diagnostic attached.
    Failed(Diagnostic),
}

impl RenderOutcome {
    pub fn svg(&self) -> Option<&str> {
        match self {
            RenderOutcome::Svg(s) => Some(s),
            RenderOutcome::Failed(_) => None,
        }
    }

    pub fn diagnostic(&self) -> Option<&Diagnostic> {
        match self {
            RenderOutcome::Failed(d) => Some(d),
            RenderOutcome::Svg(_) => None,
        }
    }
}

/// Renders one kind of fenced block to SVG.
///
/// Implementations run on a background task — they must not touch the UI.
pub trait BlockRenderer: Send + Sync {
    /// Registry key; matches `Block::renderer_id()`.
    fn id(&self) -> &'static str;

    /// Human-facing name, used in diagnostics.
    fn display_name(&self) -> &'static str;

    /// Whether this renderer can run right now. A renderer backed by an
    /// external binary reports the absence, which becomes an actionable
    /// diagnostic instead of a confusing failure.
    fn availability(&self) -> Availability;

    /// Render `source` to SVG. Must never panic: invalid input becomes
    /// [`RenderOutcome::Failed`].
    fn render(&self, source: &str) -> RenderOutcome;
}

/// Whether a renderer's dependencies are satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Pure Rust, no external dependency: always works.
    Builtin,
    /// An external tool was found at this path.
    External(PathBuf),
    /// Not usable; the string explains what to install.
    Missing(String),
}

impl Availability {
    pub fn is_available(&self) -> bool {
        !matches!(self, Availability::Missing(_))
    }

    pub fn summary(&self) -> String {
        match self {
            Availability::Builtin => "built in".to_string(),
            Availability::External(p) => format!("using {}", p.display()),
            Availability::Missing(hint) => format!("unavailable — {hint}"),
        }
    }
}

/// Registry of block renderers, keyed by `renderer_id`.
#[derive(Default)]
pub struct RendererRegistry {
    renderers: HashMap<&'static str, Arc<dyn BlockRenderer>>,
}

impl RendererRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry with every renderer this build ships.
    pub fn with_defaults() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(MathRenderer));
        registry.register(Arc::new(MermaidRenderer));
        registry.register(Arc::new(D2Renderer));
        registry.register(Arc::new(PlantUmlRenderer));
        registry
    }

    pub fn register(&mut self, renderer: Arc<dyn BlockRenderer>) {
        self.renderers.insert(renderer.id(), renderer);
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn BlockRenderer>> {
        self.renderers.get(id)
    }

    /// Render a block, or produce a diagnostic explaining why not.
    ///
    /// Results are cached by `(id, source)`. Scrolling a document must not
    /// re-render every frame, and a large document may hold many repeated
    /// diagrams. MathJax in particular costs seconds to warm up and ~90ms per
    /// formula, so this cache is load-bearing, not an optimization.
    pub fn render(&self, id: &str, source: &str) -> RenderOutcome {
        let Some(renderer) = self.get(id) else {
            return RenderOutcome::Failed(Diagnostic::warning(
                id.to_string(),
                format!("no renderer registered for `{id}`"),
            ));
        };

        let key = format!("{id}\0{source}");
        if let Some(hit) = cache().lock().ok().and_then(|c| c.get(&key).cloned()) {
            return hit;
        }

        let outcome = match renderer.availability() {
            Availability::Missing(hint) => RenderOutcome::Failed(Diagnostic::warning(
                renderer.id().to_string(),
                format!("{} renderer unavailable: {hint}", renderer.display_name()),
            )),
            // A third-party renderer that panics must degrade to a diagnostic,
            // not take the application down with it.
            _ => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                renderer.render(source)
            })) {
                Ok(outcome) => outcome,
                Err(_) => RenderOutcome::Failed(Diagnostic::error(
                    renderer.id().to_string(),
                    format!(
                        "{} renderer panicked on this input",
                        renderer.display_name()
                    ),
                )),
            },
        };

        if let Ok(mut cache) = cache().lock() {
            // Bound the cache: a long editing session on a diagram-heavy file
            // would otherwise grow it without limit.
            // ponytail: clear-on-full instead of LRU; revisit if diagram-heavy
            // editing shows repeated re-renders.
            if cache.len() >= CACHE_CAPACITY {
                cache.clear();
            }
            cache.insert(key, outcome.clone());
        }
        outcome
    }

    /// Availability of every registered renderer, for a diagnostics view.
    pub fn availability_report(&self) -> Vec<(&'static str, Availability)> {
        let mut report: Vec<_> = self
            .renderers
            .values()
            .map(|r| (r.display_name(), r.availability()))
            .collect();
        report.sort_by_key(|(name, _)| *name);
        report
    }
}

const CACHE_CAPACITY: usize = 512;

fn cache() -> &'static Mutex<HashMap<String, RenderOutcome>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RenderOutcome>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pay MathJax's start-up cost now, on a thread nobody is waiting on.
///
/// `mathjax-svg-rs` builds its JS engine lazily behind a `OnceLock` and parses
/// the whole MathJax bundle the first time anything asks it to render. Measured
/// on this machine at **~870ms**, against ~50ms for every formula after it —
/// and that first bill lands on whoever opens the first document containing a
/// formula, which reads as "clicking a file in the tree takes a second".
///
/// Spawned rather than awaited: nothing here needs the result, and the render
/// path is unchanged either way. It just finds the engine already warm.
///
/// One thread rather than a background task, because the point is to run before
/// the executor has anything queued on it — and `OnceLock` makes a concurrent
/// real render wait for this one rather than duplicating it.
pub fn warm_up() {
    std::thread::Builder::new()
        .name("mt-renderer-warmup".into())
        .spawn(|| {
            // A trivially small formula: the cost is parsing the bundle, not
            // typesetting, so anything valid warms the same engine.
            let _ = mathjax_svg_rs::render_tex("x", &Default::default());
        })
        // A failure to spawn only means the cost is paid later, in line.
        .map(|_| ())
        .unwrap_or_else(|err| log::debug!("renderer warm-up not started: {err}"));
}

// ---------------------------------------------------------------------------
// Pure-Rust renderers
// ---------------------------------------------------------------------------

/// LaTeX math via MathJax on an embedded JS engine.
///
/// Emits glyph outlines (`<path>`) rather than `<text>`, so it renders
/// correctly under resvg with no fonts installed — which is exactly what the
/// native path needs.
struct MathRenderer;

impl BlockRenderer for MathRenderer {
    fn id(&self) -> &'static str {
        "math"
    }
    fn display_name(&self) -> &'static str {
        "LaTeX"
    }
    fn availability(&self) -> Availability {
        Availability::Builtin
    }
    fn render(&self, source: &str) -> RenderOutcome {
        match mathjax_svg_rs::render_tex(source.trim(), &Default::default()) {
            Ok(svg) => RenderOutcome::Svg(svg),
            Err(err) => RenderOutcome::Failed(diagnose("math", "LaTeX", &err)),
        }
    }
}

/// Mermaid, rendered by a pure-Rust implementation.
struct MermaidRenderer;

impl BlockRenderer for MermaidRenderer {
    fn id(&self) -> &'static str {
        "mermaid"
    }
    fn display_name(&self) -> &'static str {
        "Mermaid"
    }
    fn availability(&self) -> Availability {
        Availability::Builtin
    }
    fn render(&self, source: &str) -> RenderOutcome {
        match mermaid_svg::render(source) {
            Ok(svg) => RenderOutcome::Svg(svg),
            Err(err) => RenderOutcome::Failed(diagnose("mermaid", "Mermaid", &err.to_string())),
        }
    }
}

/// D2, rendered by a pure-Rust port including its own layout engine.
struct D2Renderer;

impl BlockRenderer for D2Renderer {
    fn id(&self) -> &'static str {
        "d2"
    }
    fn display_name(&self) -> &'static str {
        "D2"
    }
    fn availability(&self) -> Availability {
        Availability::Builtin
    }
    fn render(&self, source: &str) -> RenderOutcome {
        match d2_little::d2_to_svg(source) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(svg) => RenderOutcome::Svg(svg),
                Err(err) => RenderOutcome::Failed(Diagnostic::error("d2", err.to_string())),
            },
            Err(err) => RenderOutcome::Failed(diagnose("d2", "D2", &err)),
        }
    }
}

// ---------------------------------------------------------------------------
// External-tool renderers
// ---------------------------------------------------------------------------

/// PlantUML, via a locally installed CLI.
///
/// The one technology here with no usable pure-Rust implementation: the
/// available crate needs a native Graphviz library and fails at build time.
/// Isolating that behind this renderer is exactly the point — the document
/// architecture is unaffected, and an absent binary produces an install hint.
struct PlantUmlRenderer;

const PLANTUML_HINT: &str =
    "install PlantUML (requires Java) from https://plantuml.com, or put `plantuml` on PATH";

impl PlantUmlRenderer {
    fn resolve(&self) -> Option<PathBuf> {
        which("plantuml")
    }
}

impl BlockRenderer for PlantUmlRenderer {
    fn id(&self) -> &'static str {
        "plantuml"
    }
    fn display_name(&self) -> &'static str {
        "PlantUML"
    }

    fn availability(&self) -> Availability {
        match self.resolve() {
            Some(path) => Availability::External(path),
            None => Availability::Missing(PLANTUML_HINT.to_string()),
        }
    }

    fn render(&self, source: &str) -> RenderOutcome {
        let Some(program) = self.resolve() else {
            return RenderOutcome::Failed(Diagnostic::warning(
                "plantuml",
                format!("PlantUML not found: {PLANTUML_HINT}"),
            ));
        };

        match run_piped(&program, &["-tsvg", "-pipe"], &wrap_plantuml(source)) {
            Ok(svg) => interpret_plantuml(&svg),
            Err(err) => RenderOutcome::Failed(diagnose("plantuml", "PlantUML", &err)),
        }
    }
}

/// Wrap a fence body in `@startuml`/`@enduml` when it lacks them.
///
/// A fence usually omits them, and PlantUML then renders nothing useful. Adding
/// them makes the common case work without the user thinking about it.
fn wrap_plantuml(source: &str) -> String {
    if source.contains("@start") {
        source.to_string()
    } else {
        format!("@startuml\n{}\n@enduml\n", source.trim())
    }
}

/// Decide what PlantUML's output means.
///
/// Separated from the subprocess call so it is testable on a machine without
/// Java — which is most CI machines, and this one.
fn interpret_plantuml(svg: &str) -> RenderOutcome {
    if !svg.contains("<svg") {
        return RenderOutcome::Failed(Diagnostic::error(
            "plantuml",
            "PlantUML produced no SVG output",
        ));
    }
    // PlantUML exits 0 on syntax errors and draws the message into the image.
    // Surface it as a diagnostic rather than showing a picture of an error.
    match plantuml_error(svg) {
        Some(message) => RenderOutcome::Failed(diagnose("plantuml", "PlantUML", &message)),
        None => RenderOutcome::Svg(svg.to_string()),
    }
}

/// Detect PlantUML's in-image error report and extract its text.
fn plantuml_error(svg: &str) -> Option<String> {
    if !svg.contains("Syntax Error") {
        return None;
    }
    let mut parts = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<text") {
        let after = &rest[start..];
        let Some(open) = after.find('>') else { break };
        let Some(close) = after.find("</text>") else {
            break;
        };
        if close > open {
            parts.push(after[open + 1..close].trim().to_string());
        }
        rest = &after[close + 7..];
    }
    let joined = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    Some(if joined.is_empty() {
        "Syntax Error".to_string()
    } else {
        joined
    })
}

/// Turn a renderer's error text into a diagnostic, anchored to a line when one
/// is reported.
fn diagnose(id: &str, display_name: &str, error: &str) -> Diagnostic {
    let message = error
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("rendering failed")
        .trim()
        .to_string();
    let diag = Diagnostic::error(
        id.to_string(),
        format!("{display_name} rendering failed\n\n{message}"),
    );
    match extract_line(error) {
        Some(line) => diag.at_line(line),
        None => diag,
    }
}

/// Find a `line N` or `N:M:` style line number in renderer output.
fn extract_line(text: &str) -> Option<usize> {
    // `12:3: message` — the shape d2-little uses.
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some((head, _)) = trimmed.split_once(':')
            && let Ok(n) = head.trim().parse::<usize>()
            && n > 0
        {
            return Some(n);
        }
    }
    // `line 7` / `line: 7`
    let lower = text.to_ascii_lowercase();
    let idx = lower.find("line")?;
    lower[idx + 4..]
        .trim_start_matches([' ', ':', '#'])
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
        .filter(|&n: &usize| n > 0)
}

fn run_piped(program: &PathBuf, args: &[&str], source: &str) -> Result<String, String> {
    use std::io::Write;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    child
        .stdin
        .as_mut()
        .ok_or("cannot write to renderer stdin")?
        .write_all(source.as_bytes())
        .map_err(|e| e.to_string())?;
    // Close stdin so the tool knows input ended; otherwise `-pipe` hangs.
    drop(child.stdin.take());

    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() || stdout.trim().is_empty() {
        return Err(format!(
            "{}{stdout}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(stdout)
}

/// Locate `program` on PATH.
///
/// Cached: `availability()` runs on every render pass, and probing the
/// filesystem per frame is the kind of cost that erases the native performance
/// advantage.
fn which(program: &str) -> Option<PathBuf> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<PathBuf>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(cache) = cache.lock()
        && let Some(hit) = cache.get(program)
    {
        return hit.clone();
    }

    let found = search_path(program);
    if let Ok(mut cache) = cache.lock() {
        cache.insert(program.to_string(), found.clone());
    }
    found
}

fn search_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    // On Windows a CLI is usually a `.cmd`/`.exe` shim, so an extensionless
    // probe finds nothing even when the tool is installed.
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|e| e.to_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };

    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> RendererRegistry {
        RendererRegistry::with_defaults()
    }

    // --- Availability -----------------------------------------------------

    #[test]
    fn every_required_renderer_is_registered() {
        // The goal requires Mermaid, D2, PlantUML and LaTeX. All four must be
        // registered so a block gets a real diagnostic rather than "unknown
        // renderer", even where the dependency is absent.
        let registry = registry();
        for id in ["mermaid", "d2", "plantuml", "math"] {
            assert!(registry.get(id).is_some(), "{id} must be registered");
        }
    }

    #[test]
    fn three_of_four_renderers_are_always_available() {
        let report = registry().availability_report();
        for name in ["LaTeX", "Mermaid", "D2"] {
            let entry = report.iter().find(|(n, _)| *n == name).unwrap();
            assert_eq!(
                entry.1,
                Availability::Builtin,
                "{name} must need no external dependency"
            );
        }
    }

    // --- Valid input ------------------------------------------------------

    #[test]
    fn mermaid_renders_a_valid_diagram() {
        let out = registry().render("mermaid", "pie\n\"A\" : 1\n\"B\" : 2\n");
        let svg = out
            .svg()
            .unwrap_or_else(|| panic!("expected SVG, got {:?}", out.diagnostic()));
        assert!(svg.contains("<svg"), "got {}", &svg[..svg.len().min(120)]);
    }

    #[test]
    fn mermaid_renders_a_flowchart() {
        let out = registry().render("mermaid", "graph TD;\nA[Start]-->B[End];\n");
        assert!(out.svg().is_some(), "got {:?}", out.diagnostic());
    }

    #[test]
    fn d2_renders_a_valid_diagram() {
        let out = registry().render("d2", "a -> b\n");
        let svg = out
            .svg()
            .unwrap_or_else(|| panic!("expected SVG, got {:?}", out.diagnostic()));
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn math_renders_a_valid_formula() {
        let out = registry().render("math", r"\frac{a}{b}");
        let svg = out
            .svg()
            .unwrap_or_else(|| panic!("expected SVG, got {:?}", out.diagnostic()));
        assert!(svg.contains("<svg"));
        // Glyph outlines, not <text>: this is what makes it render under resvg
        // without system fonts.
        assert!(
            svg.contains("<path") || svg.contains("<use"),
            "expected glyph outlines"
        );
    }

    // --- Invalid input: must diagnose, never crash ------------------------

    #[test]
    fn invalid_mermaid_produces_a_diagnostic() {
        let out = registry().render("mermaid", "!!! definitely not a diagram !!!");
        let diag = out.diagnostic().expect("expected a diagnostic");
        assert_eq!(diag.source, "mermaid");
        assert!(diag.message.contains("Mermaid rendering failed"));
    }

    #[test]
    fn invalid_d2_produces_a_diagnostic() {
        let out = registry().render("d2", "a ->");
        let diag = out.diagnostic().expect("expected a diagnostic");
        assert_eq!(diag.source, "d2");
    }

    #[test]
    fn invalid_math_produces_a_diagnostic() {
        let out = registry().render("math", r"\frac{");
        let diag = out.diagnostic().expect("expected a diagnostic");
        assert_eq!(diag.source, "math");
    }

    #[test]
    fn invalid_plantuml_never_crashes() {
        // PlantUML may or may not be installed; either way this must return.
        let out = registry().render("plantuml", "!!! not plantuml !!!");
        assert!(
            out.svg().is_some() || out.diagnostic().is_some(),
            "must always produce something"
        );
    }

    #[test]
    fn pathological_input_does_not_hang_or_panic() {
        let registry = registry();
        let long = "x".repeat(10_000);
        for id in ["mermaid", "d2", "math"] {
            for source in ["", "\0\0\0", long.as_str(), "中文 🎉 \\("] {
                let out = registry.render(id, source);
                assert!(
                    out.svg().is_some() || out.diagnostic().is_some(),
                    "{id} produced nothing for {:?}",
                    &source[..source.len().min(20)]
                );
            }
        }
    }

    // --- Registry behavior ------------------------------------------------

    #[test]
    fn unknown_renderer_id_is_a_diagnostic() {
        let out = registry().render("nonexistent", "x");
        assert!(out.diagnostic().unwrap().message.contains("no renderer"));
    }

    #[test]
    fn missing_external_tool_yields_an_actionable_diagnostic() {
        struct AlwaysMissing;
        impl BlockRenderer for AlwaysMissing {
            fn id(&self) -> &'static str {
                "fake"
            }
            fn display_name(&self) -> &'static str {
                "Fake"
            }
            fn availability(&self) -> Availability {
                Availability::Missing("install fake-tool".into())
            }
            fn render(&self, _: &str) -> RenderOutcome {
                panic!("must not be called when unavailable");
            }
        }
        let mut registry = RendererRegistry::new();
        registry.register(Arc::new(AlwaysMissing));
        let diag = registry.render("fake", "x").diagnostic().unwrap().clone();
        assert!(
            diag.message.contains("install fake-tool"),
            "{}",
            diag.message
        );
    }

    #[test]
    fn a_panicking_renderer_becomes_a_diagnostic() {
        struct Exploding;
        impl BlockRenderer for Exploding {
            fn id(&self) -> &'static str {
                "boom"
            }
            fn display_name(&self) -> &'static str {
                "Boom"
            }
            fn availability(&self) -> Availability {
                Availability::Builtin
            }
            fn render(&self, _: &str) -> RenderOutcome {
                panic!("third-party renderer bug");
            }
        }
        let mut registry = RendererRegistry::new();
        registry.register(Arc::new(Exploding));
        // Silence the panic hook's stderr noise for this one call.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out = registry.render("boom", "unique-panic-input");
        std::panic::set_hook(previous);
        assert!(out.diagnostic().unwrap().message.contains("panicked"));
    }

    #[test]
    fn results_are_cached_per_source() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        struct Counting;
        impl BlockRenderer for Counting {
            fn id(&self) -> &'static str {
                "counting"
            }
            fn display_name(&self) -> &'static str {
                "Counting"
            }
            fn availability(&self) -> Availability {
                Availability::Builtin
            }
            fn render(&self, _: &str) -> RenderOutcome {
                CALLS.fetch_add(1, Ordering::SeqCst);
                RenderOutcome::Svg("<svg/>".into())
            }
        }
        let mut registry = RendererRegistry::new();
        registry.register(Arc::new(Counting));
        registry.render("counting", "unique-source-abc");
        registry.render("counting", "unique-source-abc");
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "second call must hit cache"
        );
        registry.render("counting", "unique-source-xyz");
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "new source must re-render");
    }

    #[test]
    fn a_new_renderer_needs_no_core_changes() {
        // The extensibility claim, asserted: registering a renderer is all it
        // takes for the whole pipeline to dispatch to it.
        struct Graphviz;
        impl BlockRenderer for Graphviz {
            fn id(&self) -> &'static str {
                "graphviz"
            }
            fn display_name(&self) -> &'static str {
                "Graphviz"
            }
            fn availability(&self) -> Availability {
                Availability::Builtin
            }
            fn render(&self, _: &str) -> RenderOutcome {
                RenderOutcome::Svg("<svg id=\"gv\"/>".into())
            }
        }
        let mut registry = RendererRegistry::with_defaults();
        registry.register(Arc::new(Graphviz));
        // `graphviz` is already a recognized fence language in mt-doc, so the
        // whole path works without touching the parser.
        assert_eq!(
            mt_doc::DiagramKind::from_lang("graphviz").unwrap().id(),
            "graphviz"
        );
        assert!(registry.render("graphviz", "digraph{}").svg().is_some());
    }

    // --- Helpers ----------------------------------------------------------

    #[test]
    fn line_numbers_are_extracted_from_renderer_output() {
        assert_eq!(extract_line("Parse error on line 7:"), Some(7));
        assert_eq!(
            extract_line("12:3: connection missing destination"),
            Some(12)
        );
        assert_eq!(extract_line("no numbers here"), None);
        assert_eq!(extract_line("line 0"), None, "0 is not a valid line");
    }

    #[test]
    fn diagnose_keeps_the_first_meaningful_error_line() {
        let d = diagnose(
            "mermaid",
            "Mermaid",
            "\n\nUnexpected token at line 7\ntrace…\n",
        );
        assert!(d.message.contains("Unexpected token at line 7"));
        assert_eq!(d.line, Some(7));
    }

    #[test]
    fn plantuml_error_is_detected_in_the_svg() {
        let svg = "<svg><text>Syntax Error?</text><text>at line 3</text></svg>";
        assert_eq!(
            plantuml_error(svg).as_deref(),
            Some("Syntax Error? at line 3")
        );
        assert_eq!(plantuml_error("<svg><rect/></svg>"), None);
    }

    // PlantUML needs Java, which is not present on every machine (including
    // this one). These cover the decisions the renderer makes about its output,
    // which is the part that can actually be wrong.

    #[test]
    fn plantuml_source_is_wrapped_only_when_needed() {
        assert_eq!(
            wrap_plantuml("Alice -> Bob: hi"),
            "@startuml\nAlice -> Bob: hi\n@enduml\n"
        );
        let already = "@startuml\nAlice -> Bob: hi\n@enduml\n";
        assert_eq!(wrap_plantuml(already), already, "must not double-wrap");
        // Other diagram families use their own @start directive.
        let mindmap = "@startmindmap\n* root\n@endmindmap";
        assert_eq!(wrap_plantuml(mindmap), mindmap);
    }

    #[test]
    fn plantuml_output_is_interpreted_correctly() {
        // A real diagram passes through.
        let good = "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>";
        assert_eq!(interpret_plantuml(good).svg(), Some(good));

        // A syntax error drawn into the image becomes a diagnostic, so the user
        // does not get a picture of an error message.
        let bad = "<svg><text>Syntax Error?</text><text>at line 2</text></svg>";
        let diag = interpret_plantuml(bad).diagnostic().unwrap().clone();
        assert_eq!(diag.source, "plantuml");
        assert!(diag.message.contains("Syntax Error"));
        assert_eq!(diag.line, Some(2), "the reported line is extracted");

        // Non-SVG output is a failure, not silently accepted.
        assert!(
            interpret_plantuml("Exception in thread \"main\"")
                .diagnostic()
                .is_some()
        );
        assert!(interpret_plantuml("").diagnostic().is_some());
    }

    #[test]
    fn plantuml_reports_its_absence_without_running_anything() {
        // On a machine with PlantUML installed this renders; without it, the
        // diagnostic must name the install step. Either is correct — what must
        // never happen is a panic or an empty result.
        let out = registry().render("plantuml", "Alice -> Bob: hi");
        match (out.svg(), out.diagnostic()) {
            (Some(svg), _) => assert!(svg.contains("<svg")),
            (_, Some(d)) => assert!(
                d.message.contains("plantuml.com") || d.message.contains("PlantUML"),
                "unhelpful diagnostic: {}",
                d.message
            ),
            _ => panic!("produced neither output nor a diagnostic"),
        }
    }

    #[test]
    fn which_finds_a_program_that_certainly_exists() {
        // cargo is running this test, so it is on PATH by definition.
        assert!(which("cargo").is_some());
        assert!(which("definitely-not-a-real-program-xyz").is_none());
    }

    #[test]
    fn availability_summary_is_human_readable() {
        assert_eq!(Availability::Builtin.summary(), "built in");
        assert!(
            Availability::Missing("install x".into())
                .summary()
                .contains("install x")
        );
    }

    /// Warming up must be non-blocking and must leave the engine usable.
    ///
    /// The cost it moves is real and measured: MathJax's engine start-up is
    /// ~870ms on this machine, against ~50ms per formula afterwards. What the
    /// test pins is the two properties that make moving it safe — the call
    /// returns immediately, and a render after it still produces SVG rather
    /// than tripping over a half-initialized engine.
    #[test]
    fn warming_up_returns_immediately_and_leaves_math_working() {
        let start = std::time::Instant::now();
        super::warm_up();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(100),
            "warm_up blocked for {elapsed:?}; it must not be on the caller's \
             thread, or it has simply moved the stall to start-up"
        );

        // Concurrent with the warm-up thread, which is the interesting case:
        // `OnceLock` must make one wait for the other rather than both
        // building an engine.
        let out = registry().render("math", "x^2");
        assert!(
            out.svg().is_some_and(|svg| svg.contains("<svg")),
            "math must still render after a warm-up: {:?}",
            out.diagnostic()
        );
    }
}
