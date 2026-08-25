//! Block renderer registry.
//!
//! Diagram and math blocks are rendered out-of-band into SVG, which the native
//! path displays via `gpui::Image::from_bytes(ImageFormat::Svg, …)` and the
//! WebView embeds directly. A renderer is looked up by the block's
//! `renderer_id()`, so adding one is a registration — the Markdown parser and
//! the view code do not change.
//!
//! Mermaid and D2 render in pure Rust with no external dependency, so they are
//! always available. Math renders in pure Rust too, but its glyph outlines come
//! from the KaTeX faces shipped beside the executable — this application embeds
//! no font it can ask the user to install instead. PlantUML has no usable
//! pure-Rust implementation and falls back to a locally installed binary. When
//! either dependency is absent the block gets an install hint, never a crash.
//!
//! Every renderer is fallible by design. A missing tool or a syntax error
//! produces a [`RenderOutcome::Failed`] carrying a diagnostic; nothing panics,
//! and the original source is always preserved for display.

use std::collections::HashMap;
use std::fmt::Write as _;
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
    /// diagrams. D2 costs ~7ms a diagram, so this cache is load-bearing rather
    /// than an optimization.
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

// ---------------------------------------------------------------------------
// Pure-Rust renderers
// ---------------------------------------------------------------------------

/// KaTeX faces RaTeX's layout can ask for, and the file each lives in.
///
/// Mirrors `ratex-font-loader`'s own `FONT_MAP`. These are not embedded: see
/// [`MATH_FONTS_HINT`] for where they come from.
const FONT_FILES: &[(ratex_font::FontId, &str)] = {
    use ratex_font::FontId as F;
    &[
        (F::MainRegular, "KaTeX_Main-Regular.ttf"),
        (F::MainBold, "KaTeX_Main-Bold.ttf"),
        (F::MainItalic, "KaTeX_Main-Italic.ttf"),
        (F::MainBoldItalic, "KaTeX_Main-BoldItalic.ttf"),
        (F::MathItalic, "KaTeX_Math-Italic.ttf"),
        (F::MathBoldItalic, "KaTeX_Math-BoldItalic.ttf"),
        (F::AmsRegular, "KaTeX_AMS-Regular.ttf"),
        (F::CaligraphicRegular, "KaTeX_Caligraphic-Regular.ttf"),
        (F::FrakturRegular, "KaTeX_Fraktur-Regular.ttf"),
        (F::FrakturBold, "KaTeX_Fraktur-Bold.ttf"),
        (F::SansSerifRegular, "KaTeX_SansSerif-Regular.ttf"),
        (F::SansSerifBold, "KaTeX_SansSerif-Bold.ttf"),
        (F::SansSerifItalic, "KaTeX_SansSerif-Italic.ttf"),
        (F::ScriptRegular, "KaTeX_Script-Regular.ttf"),
        (F::TypewriterRegular, "KaTeX_Typewriter-Regular.ttf"),
        (F::Size1Regular, "KaTeX_Size1-Regular.ttf"),
        (F::Size2Regular, "KaTeX_Size2-Regular.ttf"),
        (F::Size3Regular, "KaTeX_Size3-Regular.ttf"),
        (F::Size4Regular, "KaTeX_Size4-Regular.ttf"),
    ]
};

/// What to tell a user who has no math fonts.
///
/// This application embeds no fonts it can ask the user to install instead —
/// the two in `assets.rs` are there because gpui requests them by exact path
/// and diagram labels come out blank without them, which is a different case.
/// The release archive ships these beside the executable, so a packaged build
/// finds them without the user doing anything; this hint is for a build run
/// from the source tree.
pub const MATH_FONTS_HINT: &str = "install the KaTeX fonts: download \
     https://github.com/KaTeX/KaTeX/releases/latest and install the `KaTeX_*.ttf` \
     files under katex/fonts/, or set MT_MATH_FONT_DIR to the folder holding them";

/// Directories that may hold the KaTeX `.ttf` files, most specific first.
///
/// No hardcoded distro paths beyond the conventional per-user and system font
/// folders: a fixed list is what makes a font loader work on the author's
/// machine and nowhere else.
fn font_dir_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(explicit) = std::env::var_os("MT_MATH_FONT_DIR") {
        dirs.push(PathBuf::from(explicit));
    }
    // Beside the executable: this is where `package-release.sh` puts them.
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("fonts"));
        // And where `cargo run` / `cargo test` leave it: the executable is in
        // `target/<profile>/` or `target/<profile>/deps/`, so the repository's
        // own `fonts/katex` is two or three levels up. Without this a build
        // from source finds no font and every formula is a diagnostic, which
        // reads as "math is broken" rather than "math is not packaged yet".
        for up in [2, 3] {
            if let Some(root) = parent.ancestors().nth(up) {
                dirs.push(root.join("fonts/katex"));
            }
        }
    }
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if cfg!(windows) {
        dirs.extend(env("LOCALAPPDATA").map(|p| p.join("Microsoft/Windows/Fonts")));
        dirs.extend(env("SYSTEMROOT").map(|p| p.join("Fonts")));
    } else if cfg!(target_os = "macos") {
        dirs.extend(env("HOME").map(|p| p.join("Library/Fonts")));
        dirs.push(PathBuf::from("/Library/Fonts"));
    } else {
        dirs.extend(env("HOME").map(|p| p.join(".local/share/fonts")));
        dirs.extend(env("HOME").map(|p| p.join(".fonts")));
        dirs.push(PathBuf::from("/usr/share/fonts/truetype/katex"));
        dirs.push(PathBuf::from("/usr/share/fonts"));
    }
    dirs
}

/// The first candidate directory holding all nineteen faces.
///
/// **Stat only.** `availability()` reaches this from `render_status_bar`, which
/// runs every frame, so reading a single byte here would put half a megabyte of
/// font on the first frame of a workspace that may never show a formula.
fn font_dir() -> Option<&'static PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        font_dir_candidates()
            .into_iter()
            .find(|dir| FONT_FILES.iter().all(|(_, f)| dir.join(f).is_file()))
    })
    .as_ref()
}

/// The KaTeX faces, read and parsed once.
///
/// Nothing here runs until the first math block is rendered — there is no
/// warm-up and an empty workspace pays nothing. Measured: 4.34MB at startup,
/// 4.55MB after a thousand formulas.
///
/// The bytes are leaked deliberately. They live for the process either way, and
/// `FontRef` borrows rather than copies, so the alternative is re-parsing every
/// face on every formula.
struct Fonts {
    faces: HashMap<ratex_font::FontId, ab_glyph::FontRef<'static>>,
}

fn fonts() -> Option<&'static Fonts> {
    static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let dir = font_dir()?;
            let faces: HashMap<_, _> = FONT_FILES
                .iter()
                .filter_map(|(id, f)| {
                    let bytes: &'static [u8] =
                        Box::leak(std::fs::read(dir.join(f)).ok()?.into_boxed_slice());
                    Some((*id, ab_glyph::FontRef::try_from_slice(bytes).ok()?))
                })
                .collect();
            // A directory that passed the stat check but holds a truncated or
            // corrupt face is not usable; reporting it as available would show
            // half a formula.
            (faces.len() == FONT_FILES.len()).then_some(Fonts { faces })
        })
        .as_ref()
}

/// The largest math source this will parse.
///
/// Every formula in the 90-case KaTeX corpus is under 1KB, and the largest math
/// block in this repository is 171 bytes. The inputs that make the parser and
/// the emitter do unbounded work — a 200,000-cell matrix is 424ms to parse,
/// 794ms to lay out, and a ~289MB SVG string — start around 300KB. The cap is
/// one comparison and turns "the background parse never returns" into a
/// diagnostic.
const MAX_MATH_BYTES: usize = 16 * 1024;

/// Type size for rendered math, and the padding around it.
const FONT_SIZE: f64 = 20.0;
const PADDING: f64 = 2.0;
/// Stroke width for unfilled paths, matching RaTeX's own default.
const STROKE_WIDTH: f64 = 1.5;

/// LaTeX math via RaTeX: parse, lay out, then emit the SVG here.
///
/// A ZST. RaTeX keeps what state it has behind its own `OnceLock`s, so the
/// renderer holds none and needs no initialization — which is why there is no
/// warm-up to spawn.
///
/// **`ratex-svg` is deliberately not a dependency.** The only way to get
/// `<path>` rather than `<text>` out of it is its `standalone` feature, and
/// `standalone` reaches `ratex-unicode-font` through two independent edges. That
/// crate prints past the `log` crate, hardcodes five distro font paths, and
/// reads a system CJK font it never frees — measured at 4.5MB to 52.0MB
/// resident on the first CJK glyph. Emitting the SVG here costs ~250 lines and
/// removes all three.
struct MathRenderer;

impl BlockRenderer for MathRenderer {
    fn id(&self) -> &'static str {
        "math"
    }
    fn display_name(&self) -> &'static str {
        "LaTeX"
    }
    fn availability(&self) -> Availability {
        match font_dir() {
            Some(dir) => Availability::External(dir.clone()),
            None => Availability::Missing(MATH_FONTS_HINT.to_string()),
        }
    }
    fn render(&self, source: &str) -> RenderOutcome {
        let source = source.trim();
        if source.len() > MAX_MATH_BYTES {
            return RenderOutcome::Failed(Diagnostic::error(
                "math",
                format!(
                    "LaTeX rendering failed\n\nformula is {} bytes; the limit is \
                     {MAX_MATH_BYTES}",
                    source.len()
                ),
            ));
        }
        // Not reached through `availability()`: that one only stats, and a
        // directory can pass the stat check and still hold a corrupt face.
        let Some(fonts) = fonts() else {
            return RenderOutcome::Failed(Diagnostic::warning(
                "math",
                format!("LaTeX fonts not found: {MATH_FONTS_HINT}"),
            ));
        };
        let nodes = match ratex_parser::parse(source) {
            Ok(nodes) => nodes,
            Err(err) => return RenderOutcome::Failed(diagnose_math(source, &err)),
        };
        let list = ratex_layout::to_display_list(&ratex_layout::layout(
            &nodes,
            &ratex_layout::LayoutOptions::default(),
        ));
        RenderOutcome::Svg(emit_svg(&list, fonts, FONT_SIZE, PADDING))
    }
}

/// A RaTeX `ParseError` carries a byte range; turn it into a 1-based line.
///
/// Separate from [`diagnose`], which reads a line number out of a renderer's
/// error *text* — RaTeX reports position structurally, so guessing is not
/// needed here.
fn diagnose_math(source: &str, err: &ratex_parser::ParseError) -> Diagnostic {
    let diag = Diagnostic::error("math", format!("LaTeX rendering failed\n\n{}", err.message));
    match &err.loc {
        Some(loc) => diag.at_line(source[..loc.start.min(source.len())].lines().count().max(1)),
        None => diag,
    }
}

/// The paint for one display item.
///
/// Black — RaTeX's default, and the overwhelming majority — becomes
/// `currentColor` so a formula inherits the surrounding text colour. That is
/// what lets one rendered SVG serve twelve themes and follow the OS light/dark
/// switch: the web pane sets `color` on the body and the native path sets it on
/// the root `<svg>`. Anything else came from `\textcolor` or `\color` and is
/// emitted literally.
fn paint(c: &ratex_types::color::Color) -> String {
    if *c == ratex_types::color::Color::BLACK {
        "currentColor".into()
    } else {
        c.to_string()
    }
}

/// Display list to SVG.
///
/// `<path>` for every glyph a KaTeX face covers, which is all of maths;
/// `<text>` for the rest — CJK inside `\text`, emoji — which resvg resolves
/// against the font database gpui already populates from the system.
fn emit_svg(
    list: &ratex_types::display_item::DisplayList,
    fonts: &Fonts,
    em: f64,
    pad: f64,
) -> String {
    use ratex_types::display_item::DisplayItem;

    let mut body = String::new();
    let tx = |v: f64| num(v * em + pad);

    for item in &list.items {
        let (x, y, scale, font, char_code, color) = match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font,
                char_code,
                color,
            } => (x, y, scale, font, char_code, color),
            // Fraction bars, `\overline`, `\hline`. A zero-thickness rect draws
            // nothing at all, so the floor is load-bearing rather than tidy.
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                dashed,
                color,
            } => {
                let t = (thickness * em).max(1e-6);
                let _ = if *dashed {
                    write!(
                        body,
                        r#"<line x1="{}" y1="{y1}" x2="{}" y2="{y1}" stroke="{c}" stroke-width="{}" stroke-dasharray="{d} {d}"/>"#,
                        tx(*x),
                        tx(x + width),
                        num(t),
                        c = paint(color),
                        y1 = tx(*y),
                        d = num(t * 3.0)
                    )
                } else {
                    write!(
                        body,
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                        tx(*x),
                        num(y * em + pad - t / 2.0),
                        num(width * em),
                        num(t),
                        paint(color)
                    )
                };
                continue;
            }
            // `\colorbox` and `\fbox` backgrounds.
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                color,
            } => {
                let _ = write!(
                    body,
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
                    tx(*x),
                    tx(*y),
                    num(width * em),
                    num(height * em),
                    paint(color)
                );
                continue;
            }
            // Radical signs and large delimiters, which arrive already in path
            // form rather than as glyphs.
            DisplayItem::Path {
                x,
                y,
                commands,
                fill,
                color,
            } => {
                let d = path_d(x * em + pad, y * em + pad, em, commands);
                if !d.is_empty() {
                    let _ = if *fill {
                        write!(
                            body,
                            r#"<path d="{d}" fill="{}" fill-rule="nonzero" stroke="none"/>"#,
                            paint(color)
                        )
                    } else {
                        write!(
                            body,
                            r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                            paint(color),
                            num(STROKE_WIDTH)
                        )
                    };
                }
                continue;
            }
        };

        let id = ratex_font::FontId::parse(font).unwrap_or(ratex_font::FontId::MainRegular);
        let ch = ratex_font::katex_ttf_glyph_char(id, *char_code);
        let outline = fonts
            .faces
            .get(&id)
            .or_else(|| fonts.faces.get(&ratex_font::FontId::MainRegular))
            .and_then(|f| {
                use ab_glyph::Font as _;
                let g = f.glyph_id(ch);
                // Glyph 0 is `.notdef` — a box, which is worse than falling
                // through to the SVG renderer's own font stack.
                (g.0 != 0).then(|| {
                    outline_d(
                        (*x * em + pad) as f32,
                        (*y * em + pad) as f32,
                        (*scale * em) as f32,
                        f,
                        g,
                    )
                })?
            });

        let _ = match outline {
            Some(d) => write!(
                body,
                r#"<path d="{d}" fill="{}" stroke="none"/>"#,
                paint(color)
            ),
            None => write!(
                body,
                r#"<text x="{}" y="{}" font-family="sans-serif" font-size="{}" fill="{}">{}</text>"#,
                tx(*x),
                tx(*y),
                num(*scale * em),
                paint(color),
                escape_xml(ch)
            ),
        };
    }

    let w = list.width * em + 2.0 * pad;
    let h = (list.height + list.depth) * em + 2.0 * pad;
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}pt" height="{h}pt" fill="currentColor">{body}</svg>"#,
        w = num(w),
        h = num(h),
    )
}

/// A `DisplayItem::Path`'s commands as an SVG path `d`.
///
/// Geometry ported from `ratex-svg` 0.1.14 (MIT), which this crate deliberately
/// does not depend on — see [`MathRenderer`].
fn path_d(
    ox: f64,
    oy: f64,
    em: f64,
    commands: &[ratex_types::path_command::PathCommand],
) -> String {
    use ratex_types::path_command::PathCommand as P;

    let mut d = String::new();
    for cmd in commands {
        let _ = match cmd {
            P::MoveTo { x, y } => write!(d, "M{} {} ", num(ox + x * em), num(oy + y * em)),
            P::LineTo { x, y } => write!(d, "L{} {} ", num(ox + x * em), num(oy + y * em)),
            P::QuadTo { x1, y1, x, y } => write!(
                d,
                "Q{} {} {} {} ",
                num(ox + x1 * em),
                num(oy + y1 * em),
                num(ox + x * em),
                num(oy + y * em)
            ),
            P::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => write!(
                d,
                "C{} {} {} {} {} {} ",
                num(ox + x1 * em),
                num(oy + y1 * em),
                num(ox + x2 * em),
                num(oy + y2 * em),
                num(ox + x * em),
                num(oy + y * em)
            ),
            P::Close => write!(d, "Z "),
        };
    }
    d.trim_end().to_string()
}

/// One glyph's outline as an SVG path `d`, y-flipped and scaled to `em`.
///
/// Geometry ported from `ratex-svg` 0.1.14 (MIT). The 0.01 tolerance is theirs:
/// `ab_glyph` hands back a flat list of curves, and a start point that does not
/// continue the previous end point begins a new closed subpath.
fn outline_d(
    px: f32,
    py: f32,
    em: f32,
    font: &ab_glyph::FontRef<'_>,
    gid: ab_glyph::GlyphId,
) -> Option<String> {
    use ab_glyph::{Font as _, OutlineCurve};

    let outline = font.outline(gid)?;
    let s = em / font.units_per_em().unwrap_or(1000.0);
    let at = |p: &ab_glyph::Point| (px + p.x * s, py - p.y * s);

    let mut d = String::new();
    let mut last: Option<(f32, f32)> = None;
    for curve in &outline.curves {
        let (start, end) = match curve {
            OutlineCurve::Line(a, b) => (at(a), at(b)),
            OutlineCurve::Quad(a, _, b) => (at(a), at(b)),
            OutlineCurve::Cubic(a, _, _, b) => (at(a), at(b)),
        };
        if last.is_none_or(|(lx, ly)| (lx - start.0).abs() > 0.01 || (ly - start.1).abs() > 0.01) {
            if last.is_some() {
                d.push_str("Z ");
            }
            let _ = write!(d, "M{} {} ", num(start.0 as f64), num(start.1 as f64));
        }
        let p = |q: &ab_glyph::Point| {
            let (x, y) = at(q);
            format!("{} {}", num(x as f64), num(y as f64))
        };
        let _ = match curve {
            OutlineCurve::Line(_, b) => write!(d, "L{} ", p(b)),
            OutlineCurve::Quad(_, c, b) => write!(d, "Q{} {} ", p(c), p(b)),
            OutlineCurve::Cubic(_, c1, c2, b) => write!(d, "C{} {} {} ", p(c1), p(c2), p(b)),
        };
        last = Some(end);
    }
    if last.is_some() {
        d.push('Z');
    }
    (!d.is_empty()).then(|| d.trim().to_string())
}

/// Shortest decimal that still places a glyph correctly at display sizes.
fn num(n: f64) -> String {
    let s = format!("{n:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.into()
    }
}

/// Escape a character for XML text content.
///
/// Only reached for the `<text>` fallback, and only for one character at a
/// time — but a document is untrusted input on both render paths, so a `<` that
/// survives is markup injection.
fn escape_xml(c: char) -> String {
    match c {
        '&' => "&amp;".into(),
        '<' => "&lt;".into(),
        '>' => "&gt;".into(),
        other => other.to_string(),
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

    /// The diagram backends need nothing installed; math needs its fonts.
    ///
    /// This used to assert all three of LaTeX, Mermaid and D2 were `Builtin`.
    /// LaTeX no longer is, and the change is deliberate rather than a
    /// regression: MathJax carried its own glyph outlines inside a 1.6MB
    /// JavaScript bundle compiled into the binary, and this application now
    /// embeds no font it could instead ship beside the executable. So math
    /// reports where its fonts came from, exactly as PlantUML reports where its
    /// binary came from.
    ///
    /// What must stay true is that math is never *silently* unavailable: either
    /// a directory it found, or a `Missing` carrying an install hint.
    #[test]
    fn the_diagram_backends_need_nothing_installed() {
        let report = registry().availability_report();
        for name in ["Mermaid", "D2"] {
            let entry = report.iter().find(|(n, _)| *n == name).unwrap();
            assert_eq!(
                entry.1,
                Availability::Builtin,
                "{name} must need no external dependency"
            );
        }

        let math = report.iter().find(|(n, _)| *n == "LaTeX").unwrap();
        match &math.1 {
            Availability::External(dir) => assert!(
                dir.join("KaTeX_Main-Regular.ttf").is_file(),
                "reported {dir:?} as the font directory, but the faces are not there"
            ),
            Availability::Missing(hint) => assert!(
                hint.contains("KaTeX") && hint.contains("MT_MATH_FONT_DIR"),
                "the hint must say what to install and how to point at it: {hint}"
            ),
            Availability::Builtin => panic!(
                "math no longer carries its own glyphs; reporting `Builtin` \
                 would hide a missing font until the first formula"
            ),
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

    /// Math must initialize nothing until a formula is actually rendered.
    ///
    /// This replaces `warming_up_returns_immediately_and_leaves_math_working`,
    /// whose whole premise was a JS engine that cost ~870ms to start and so had
    /// to be warmed on a thread before the window opened. RaTeX has no engine:
    /// the renderer is a ZST and the fonts load on first use, so the property
    /// worth pinning is the opposite one — that constructing the registry
    /// touches no font and reads no file.
    ///
    /// Measured on the release build: 4.34MB at startup, 4.55MB after a
    /// thousand formulas, against MathJax's 87.5MB paid unconditionally.
    #[test]
    fn constructing_the_registry_does_not_load_a_font() {
        let registry = RendererRegistry::with_defaults();

        // The property, observed rather than timed: `availability()` may stat —
        // it runs from `render_status_bar` on every frame — but must not read.
        // A wall-clock bound alone would pass just as well if a font *were*
        // loaded quickly, and would flake on a cold cache.
        //
        // `External(dir)` proves the directory was found by stat, and that no
        // face was parsed: parsing happens in `fonts()`, behind its own
        // `OnceLock`, which `availability()` never reaches.
        match registry
            .availability_report()
            .iter()
            .find(|(n, _)| *n == "LaTeX")
        {
            Some((_, Availability::External(dir))) => assert!(
                dir.is_dir(),
                "reported {dir:?} without stat-ing it into existence"
            ),
            Some((_, Availability::Missing(_))) => {}
            other => panic!("math reported {other:?}, which hides when a font loads"),
        }

        // Repeated reports must be free: the `OnceLock` means one directory
        // search per process, not one per frame. Nineteen `is_file()` calls per
        // candidate directory, sixty times a second, would be a real cost.
        let start = std::time::Instant::now();
        for _ in 0..1_000 {
            let _ = registry.availability_report();
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(200),
            "1000 availability reports took {elapsed:?}; the directory search \
             is meant to be behind a OnceLock, so this is re-stat-ing per call"
        );
    }

    /// The column-count bombs, which abort rather than panic.
    ///
    /// `\begin{alignat}{N}` allocates `N * 2` 64-byte values with no bound, so
    /// an unclamped `N` is an allocation failure — and that aborts, which the
    /// `catch_unwind` in [`RendererRegistry::render`] cannot contain. The clamp
    /// lives in `vendor/ratex-parser`; this is what proves the patch is applied.
    ///
    /// Every shape here defeats a guard written over the source text, which is
    /// why the clamp is in the parser instead: it sees the argument after macro
    /// expansion.
    #[test]
    fn column_count_bombs_are_rejected_rather_than_aborting() {
        let bombs = [
            r"\begin{alignat}{1000000000} a &= b \end{alignat}".to_string(),
            format!(
                "{B}begin {{alignat}}{{1000000000}} a &= b {B}end{{alignat}}",
                B = "\\"
            ),
            format!(
                "{B}begin\n{{alignat}}{{1000000000}} a &= b {B}end{{alignat}}",
                B = "\\"
            ),
            r"\begin{alignedat}{ 999999999 } a &= b \end{alignedat}".to_string(),
            r"\def\N{1000000000}\begin{alignat}{\N} a &= b \end{alignat}".to_string(),
            r"\def\EE{alignat}\begin{\EE}{300000000} a &= b \end{\EE}".to_string(),
        ];
        for bomb in &bombs {
            assert!(
                ratex_parser::parse(bomb).is_err(),
                "must be rejected before allocating: {bomb}"
            );
        }
        // Ordinary LaTeX a source-text guard would have false-positived on: the
        // brace after `\begin{cases}` is a cell, not a column count.
        for ok in [
            r"\begin{alignat}{2} a &= b & c &= d \end{alignat}",
            r"\begin{alignat}{256} a &= b \end{alignat}",
            r"\begin{cases}{-1} & x<0 \\ {1} & x\ge 0\end{cases}",
            r"\begin{pmatrix}{1000} & 0 \\ 0 & 1\end{pmatrix}",
        ] {
            assert!(ratex_parser::parse(ok).is_ok(), "must still parse: {ok}");
        }
    }

    /// An oversized source is capped before the parser sees it.
    ///
    /// A 200,000-cell matrix costs 424ms to parse, 794ms to lay out and yields
    /// a ~289MB string. The largest math block in this repository is 171 bytes.
    #[test]
    fn an_oversized_formula_is_capped() {
        let out = registry().render("math", &"x+".repeat(MAX_MATH_BYTES));
        assert!(
            out.diagnostic().is_some(),
            "an oversized source must diagnose rather than run"
        );
    }

    /// Math colour must follow the theme, and an explicit colour must survive.
    ///
    /// One rendered SVG serves twelve themes and the OS light/dark switch, so a
    /// baked-in black would make every formula invisible on a dark background.
    #[test]
    fn glyph_colour_defaults_to_currentcolor_and_textcolor_survives() {
        let registry = registry();
        let Some(plain) = registry.render("math", "x+y").svg().map(str::to_string) else {
            // No KaTeX fonts on this machine, so there is no colour to assert
            // about. Say so rather than passing silently: this repository has
            // already had thirty-seven source-scanning tests pass against
            // nothing, and a green test that asserted nothing is the same bug.
            eprintln!(
                "SKIPPED glyph_colour_defaults_to_currentcolor_and_textcolor_survives: \
                 no KaTeX fonts found. Set MT_MATH_FONT_DIR or run from the repository, \
                 which carries them in fonts/katex."
            );
            return;
        };
        assert!(
            plain.contains("currentColor"),
            "an uncoloured formula must inherit the theme"
        );
        assert!(!plain.contains("#000"), "no hardcoded black");
        let red = registry
            .render("math", r"\textcolor{red}{x}+y")
            .svg()
            .expect("fonts were present a moment ago")
            .to_string();
        assert!(red.contains("#ff0000"), "an explicit colour must survive");
        assert!(
            red.contains("currentColor"),
            "the uncoloured half must still follow the theme"
        );
    }
}
