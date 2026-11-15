//! Owner lane: document.
//!
//! The text column's content: the source editor, and the routing between
//! source, preview and the web compatibility pane.
//!
//! Contract: [`column`] is what the shell puts inside the column box — the
//! measure, the ground, the 40px insets, the column's scroll behaviour and the
//! half-viewport top inset all belong to it, not to the shell. `glow`'s
//! decorations are applied to the editor here ([`glow::document_decorations`]),
//! so a finding's light moves with the text.
//!
//! The document lane owns: Martian Mono 14/24 with Markdown punctuation at 34%
//! ink, `reveal_position(.., center)` for the review's camera, the preview and
//! its results, the web pane with its trust level and its `web_dirty`
//! deferral, and the Split routing that puts the preview (or the web pane) in
//! the field.
//!
//! The camera: a docked surface (the review's racked finding, a revision's
//! current hunk) asks for a line with
//! [`crate::state::AppState::focus_document_line`], and the source editor
//! levels it with the field's docked reading position. With a docked surface
//! open the editor's content carries half a viewport of room above its first
//! row, so a line near the top of the document can be levelled too (the
//! editor clamps its scroll at 0, so the room has to be in the content); the
//! bottom half is `scroll_beyond_last_line`.

use mt_core::settings::Language;
use nsh_base::{
    editor::{
        Editor, EditorApi, EditorDecoration, EditorPosition, EditorRange, EditorScrollBeyond,
        EditorStyles, editor,
    },
    text_view::PartStyle,
};
use nsh_core::{
    Color, PropertyValue, StyleKey,
    element::{Style, StyleRefinement},
    prelude::*,
};

use crate::{
    state::{AppState, Pane, Tab},
    ui::{glow, preview, revision, tokens},
};

/// What the source editor can be set in today: Martian Mono under the name the
/// editor's text engine can find it by.
///
/// `tokens::FONT_SOURCE` names the app's mono by its typographic family name
/// (`Martian Mono`), which is what `fonts::register` reports back; the
/// Windows text engine searches both the system collection and the registered
/// faces with DirectWrite's `FindFamilyName`, which matches the file's
/// *legacy* family name instead (nameID 1: `Martian Mono Cn Rg`). Measured,
/// 60 `M`s at 14pt: DengXian 727, the registered face 504 (its 0.6em
/// advance), Consolas 462 — see `the_editor_draws_the_registered_source_font`.
/// The legacy name comes first, the typographic one second, so the editor
/// follows a platform that resolves that one, and the spec's fallbacks last.
const FONT_SOURCE_EDITOR: &str =
    "Martian Mono Cn Rg, Martian Mono, DengXian, Microsoft YaHei UI, monospace";

/// The column's content for the active document, by pane. Empty while no
/// document is open (the shell shows [`crate::ui::welcome::welcome`] then).
pub fn column(state: AppState, language: Language) -> impl IntoElement {
    let state_for_tabs = state.clone();
    view()
        .test_id("document")
        .flex_1()
        .h(relative(1.0))
        // The document starts 40px below the column's top edge, as the
        // prototype's `.doc` does; the banners the shell puts above stay
        // outside it.
        .pt(px(tokens::SPACE_5))
        .child(each(
            move || state_for_tabs.active_tab().into_iter().collect::<Vec<_>>(),
            |tab: &Tab| tab.id,
            {
                let state = state.clone();
                move |tab: ReadSignal<Tab>, _| {
                    let tab = tab.get();
                    match tab.pane.get() {
                        Pane::Source | Pane::Split | Pane::SplitWeb => {
                            source(state.clone(), tab).into_element()
                        }
                        Pane::Preview => preview::column(state.clone(), language).into_element(),
                        Pane::Web => preview::web(state.clone(), language).into_element(),
                    }
                }
            },
        ))
}

/// The source editor: plain-file Markdown, bytes preserved, findings drawn as
/// decorations on the live text.
///
/// Afterglow's source mode is monochrome: your text sits in ambient ink,
/// Markdown punctuation steps down to 34%, and nothing else is coloured —
/// the light belongs to the model's findings, never to your own text.
fn source(state: AppState, tab: Tab) -> impl IntoElement {
    let change_state = state.clone();
    let edit_tab = tab.clone();
    let decorations_state = state.clone();
    let decorations_tab = tab.clone();
    let punctuation_tab = tab.clone();
    let camera_state = state.clone();
    let camera_tab = tab.clone();
    let styles_state = state.clone();
    let numbers_state = state.clone();
    let editor_ref = component_ref::<Editor>();
    let camera = editor_ref;

    // The column's camera: a docked surface asked to level a line, and the
    // editor's own `reveal_position(.., true)` centres it. Reading the signal
    // subscribes; a request for another tab is ignored.
    //
    // The reveal waits one frame: the half-viewport inset (the `styles` below,
    // which turns on with the same request) gives the editor its room to
    // scroll into, and the editor computes its scroll target from the rows as
    // laid out, so the inset must be a frame old by then.
    effect(move || {
        let focus = camera_state.document_focus.get();
        if focus.tab != camera_tab.id || !docked_surfaces(&camera_state) {
            return;
        }
        request_animation_frame(move |_| {
            if let Some(api) = camera.api::<EditorApi>() {
                api.reveal_position(EditorPosition::new(focus.line, 0), true);
            }
        });
    });

    // The half-viewport inset, in points: the visible height of the column's
    // text region, while a docked surface holds a camera request for this tab.
    let viewport = signal(0.0f32);
    let inset_tab = tab.id;
    let styles = move || {
        let mut styles = EditorStyles::default();
        styles.content = Some(PartStyle::from(Style::from(
            StyleRefinement::default()
                .pt(px(inset(&styles_state, inset_tab, viewport.get())))
                // The editor draws its text in the theme's `foreground`; the
                // source's ink is the room's, at the body level.
                .text_color(tokens::palette().ink),
        )));
        // The gutter: 11px line numbers in non-text ink, shown only while a
        // docked surface is reading the document (the prototype hides them
        // while you edit).
        styles.line_number = Some(PartStyle::from(Style::from(
            StyleRefinement::default()
                .text_size(px(11.0))
                .text_color(tokens::palette().ink_faint),
        )));
        styles
    };

    // The punctuation ink is built once per text (and appearance), not once
    // per binding read: a long document is scanned on a change, not a frame.
    let punctuation =
        computed(move || punctuation_decorations(&punctuation_tab.text.get(), tokens::palette()));

    view()
        .test_id("source")
        .flex_1()
        .min_h(px(0.0))
        .h(relative(1.0))
        .on_layout(move |frame: &Bounds<Points>| viewport.set(frame.size.height.0))
        .child(
            // The editor's element carries no `test_id` of its own; the box
            // around it does, and it is the editor's own box.
            view()
                .test_id("source-editor")
                .flex_1()
                .min_h(px(0.0))
                .h(relative(1.0))
                .child(
                    editor()
                        .component_ref(&editor_ref)
                        .value(move || Some(edit_tab.text.get()))
                        // `language` would turn syntect's reading palette on; Afterglow
                        // colours nothing of yours, so the editor stays plain and the
                        // decorations carry the punctuation ink.
                        .language(None)
                        .accessibility_label(Some(SharedString::from("Markdown source")))
                        .font_family(FONT_SOURCE_EDITOR)
                        .font_size(px(tokens::TYPE_SOURCE))
                        .line_height(px(tokens::TYPE_SOURCE_LINE))
                        // The measure is the window's, not a horizontal scroll: as the
                        // prototype wraps, so does the editor.
                        .soft_wrap(true)
                        .line_numbers(move || docked_surfaces(&numbers_state))
                        .active_line(false)
                        .indent_guides(false)
                        // No fold markers: Afterglow's gutter shows the line numbers
                        // of a reading pass and nothing else, and a Markdown file's
                        // folds are its code fences, not structure to collapse.
                        .folding(false)
                        // The room past the last line lets the camera level the end of
                        // the document too (the default; stated here because the camera
                        // depends on it).
                        .scroll_beyond_last_line(EditorScrollBeyond::Screen)
                        .styles(styles)
                        .decorations(move || {
                            let mut decorations = punctuation.get();
                            decorations.extend(glow::document_decorations(
                                &decorations_state,
                                &decorations_tab,
                            ));
                            decorations
                        })
                        .w_full()
                        .flex_1()
                        .on_change(move |value: &SharedString| {
                            // nsh reports the bound value once on mount. A document is
                            // dirty only after the buffer differs from what the editor
                            // already has.
                            if edit_tab.text.get().as_str() == value.as_str() {
                                return;
                            }
                            edit_tab.text.set(value.clone());
                            edit_tab.dirty.set(true);
                            edit_tab.revision.update(|revision| *revision += 1);
                            change_state.note_edit(edit_tab.id);
                        }),
                ),
        )
}

/// Whether a document is open (the shell asks before it shows the column).
pub fn has_document(state: &AppState) -> bool {
    state.active_tab().is_some()
}

/// Whether a surface that levels lines with its docked reading position is in
/// the field (the review, a revision). The column shows the editor's line
/// numbers — a reading pass's gutter — only then; the context reads a document
/// of its own and never moves this one.
fn docked_surfaces(state: &AppState) -> bool {
    state.review_open.get() || revision::is_open(state)
}

/// The editor's half-viewport top inset, in points: half of `visible` while a
/// docked surface holds a camera request for `tab`, nothing otherwise.
///
/// The inset is the room the camera needs — the editor clamps its scroll at 0,
/// so a line near the top of the document can only be levelled if the content
/// carries half a viewport above its first row — and it is tied to the request
/// rather than to the docked surface, so opening a field alone never moves the
/// document: it moves when the camera has somewhere to go.
fn inset(state: &AppState, tab: u64, visible: f32) -> f32 {
    if visible > 0.0 && docked_surfaces(state) && state.document_focus.get().tab == tab {
        visible / 2.0
    } else {
        0.0
    }
}

/// Markdown punctuation at 34% ink, as the prototype's `renderLine` marks it:
/// a heading's `#`s and the space after them, and every backtick of inline
/// code. A heading's line itself steps to 92% ink. Nothing else is marked —
/// your text is not a syntax display.
fn punctuation_decorations(source: &str, palette: &tokens::Palette) -> Vec<EditorDecoration> {
    let punctuation = palette.ink_at(0.34);
    let heading = palette.ink_at(0.92);
    let mut out = Vec::new();
    for (index, line) in source.split('\n').enumerate() {
        let line_index = index as u32;
        let hashes = line.bytes().take_while(|byte| *byte == b'#').count();
        let heading_prefix = hashes > 0
            && line
                .as_bytes()
                .get(hashes)
                .is_some_and(|byte| *byte == b' ');
        let mut column = 0u32;
        if heading_prefix {
            out.push(marker(line_index, 0, hashes as u32 + 1, punctuation));
        }
        for character in line.chars() {
            if character == '`' {
                out.push(marker(line_index, column, column + 1, punctuation));
            }
            column += character.len_utf16() as u32;
        }
        if heading_prefix && column > 0 {
            out.push(line_tint(line_index, column, heading));
        }
    }
    out
}

/// An inline mark over `start..end` (UTF-16 columns) of `line`.
fn marker(line: u32, start: u32, end: u32, color: Color) -> EditorDecoration {
    let mut decoration = EditorDecoration::default();
    decoration.range = EditorRange::new(
        EditorPosition::new(line, start),
        EditorPosition::new(line, end),
    );
    decoration.style = Some(text_ink(color));
    decoration
}

/// A whole heading line (`end` UTF-16 columns wide) at `color`: the rows of
/// the lines the range covers take the style, and the row's text inherits it.
fn line_tint(line: u32, end: u32, color: Color) -> EditorDecoration {
    let mut decoration = EditorDecoration::default();
    decoration.range =
        EditorRange::new(EditorPosition::new(line, 0), EditorPosition::new(line, end));
    decoration.line_style = Some(text_ink(color));
    decoration
}

/// A style that sets the text colour, built from a declaration: the editor
/// replays a decoration's style through [`Style::decls`], which a style built
/// from a typed refinement has none of.
fn text_ink(color: Color) -> Style {
    Style::default().with(StyleKey::Color, PropertyValue::Color(Some(color)))
}

#[cfg(test)]
mod tests {
    use std::{sync::OnceLock, time::Duration};

    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;
    use crate::state::AppState;

    /// The fixture the probes open; the dir travels with it so it outlives the
    /// app. Sixty lines with a sentinel near the top, a heading and inline code
    /// for the punctuation ink, and the document's end spelled out.
    fn probe_path() -> &'static std::path::Path {
        static FIXTURE: OnceLock<(tempfile::TempDir, std::path::PathBuf)> = OnceLock::new();
        FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("temp dir");
                let path = dir.path().join("probe.md");
                let mut text = String::from("# Release plan\n\nthe `agent` runs\n");
                for index in 4..59 {
                    if index == 4 {
                        text.push_str("PRIVATE-CAMERA-SENTINEL\n");
                    } else {
                        text.push_str(&format!("line {index}\n"));
                    }
                }
                text.push_str("END-OF-THE-DOCUMENT\n");
                std::fs::write(&path, text).expect("write the fixture");
                (dir, path)
            })
            .1
            .as_path()
    }

    /// The state with the fixture open, or `None` (the probe's own view).
    fn probe_state() -> AppState {
        let state = AppState::new();
        if let Some(parent) = probe_path().parent() {
            state.open_folder(parent.to_path_buf());
        }
        state.open_file(probe_path().to_path_buf());
        state
    }

    /// A column of a 60-line document with a docked surface open, and two
    /// controls that ask the camera for a line near the top of the document
    /// and for the last one.
    #[component]
    fn CameraProbe() -> impl IntoElement {
        let state = probe_state();
        // A docked surface is reading the document: the review's field. It
        // turns the column's half-viewport inset — and its line numbers — on.
        state.review_open.set(true);
        let first_state = state.clone();
        let last_state = state.clone();
        view()
            .test_id("probe")
            .flex_col()
            .w(px(760.0))
            .h(px(700.0))
            // The shell sets the room's ink on its root; the probe wears it too,
            // so the editor's inherited colour is the one the app shows.
            .text_color(move || tokens::palette().ink)
            .child(
                view()
                    .test_id("probe-focus-first")
                    .w_full()
                    .h(px(8.0))
                    // The fixture opens on a task, so the tab is read when the
                    // press happens, not when the view is made.
                    .on_press(move || {
                        if let Some(tab) = first_state.active_tab() {
                            first_state.focus_document_line(tab.id, 3);
                        }
                    }),
            )
            .child(
                view()
                    .test_id("probe-focus-last")
                    .w_full()
                    .h(px(8.0))
                    .on_press(move || {
                        if let Some(tab) = last_state.active_tab() {
                            last_state.focus_document_line(tab.id, 58);
                        }
                    }),
            )
            .child(column(state, Language::English))
    }

    /// The characters of a ruler line.
    #[cfg(windows)]
    const RULER: usize = 60;

    /// The family names the source editor is tried in, in order: what the
    /// column sets it in, what `tokens::FONT_SOURCE` asks for, the faces'
    /// legacy names (nameID 1 — the name DirectWrite's `FindFamilyName`
    /// matches), their typographic names (nameID 16), the system's own mono
    /// and a family that does not exist (the engine's fallback face).
    #[cfg(windows)]
    const FONT_TRIALS: [&str; 9] = [
        FONT_SOURCE_EDITOR,
        tokens::FONT_SOURCE,
        "Martian Mono Cn Rg",
        "Martian Mono",
        "Martian Grotesk Std Rg",
        "Martian Grotesk",
        "Consolas",
        "DengXian",
        "No Such Family 12345",
    ];

    /// One editor per family in [`FONT_TRIALS`], each with a 60-character
    /// ruler line; the first also holds a second line of 60 `i`s, so a
    /// monospaced face and a proportional one can be told apart by their
    /// widths alone.
    #[cfg(windows)]
    #[component]
    fn FontProbe() -> impl IntoElement {
        let ruler_m: SharedString = "M".repeat(RULER).into();
        let ruler_i: SharedString = "i".repeat(RULER).into();
        let source_text: SharedString = format!("{ruler_m}\n{ruler_i}").into();
        let mut probe = view()
            .test_id("font-probe")
            .flex_col()
            .w(px(760.0))
            .h(px(700.0));
        for (index, family) in FONT_TRIALS.iter().enumerate() {
            let text = if index == 0 {
                source_text.clone()
            } else {
                ruler_m.clone()
            };
            probe = probe.child(font_editor(family, text, family));
        }
        probe
    }

    /// An editor of a fixed height set in `family`.
    #[cfg(windows)]
    fn font_editor(id: &'static str, text: SharedString, family: &'static str) -> impl IntoElement {
        view().test_id(id).w_full().h(px(56.0)).child(
            editor()
                .default_value(text)
                .font_family(family)
                .font_size(px(14.0))
                .line_height(px(24.0))
                .soft_wrap(false)
                .line_numbers(false)
                .folding(false)
                .indent_guides(false)
                .active_line(false)
                .w_full()
                .h(relative(1.0)),
        )
    }

    /// The widths of the rows whose laid-out text is exactly `line`, by their
    /// y (the probe's editors, top to bottom).
    #[cfg(windows)]
    fn ruler_widths(app: &Headless, line: &str) -> Vec<(f32, f32)> {
        let mut widths: Vec<(f32, f32)> = app
            .screen()
            .layout_nodes()
            .iter()
            .filter(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == line)
            })
            .map(|node| (node.bounds.origin.y.0, node.bounds.size.width.0))
            .collect();
        widths.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("finite y"));
        widths
    }

    /// Which family name lets the source editor draw the app's registered
    /// Martian Mono.
    ///
    /// The faces register as `Martian Mono` / `Martian Grotesk` (their
    /// typographic names, nameID 16), but DirectWrite's `FindFamilyName`
    /// matches the files' legacy family names (nameID 1): `Martian Mono Cn Rg`
    /// and `Martian Grotesk Std Rg`. A 60-character ruler line measures the
    /// same whichever letter it is exactly when the face that resolved is
    /// monospaced.
    ///
    /// Windows-only: it lays text out with the platform's own DirectWrite
    /// engine, the one the app draws with.
    #[cfg(windows)]
    #[test]
    fn the_editor_draws_the_registered_source_font() {
        use nsh_platform_windows::DWriteTextEngine;

        // The faces travel with the root, as `main.rs` hands them to the app's:
        // the first frame is already measured with them.
        let mut root = Root::of::<FontProbe>();
        for face in crate::ui::fonts::faces() {
            root = root.font(face);
        }
        let mut app =
            Headless::start_with_text_engine(EngineConfig::new(root), DWriteTextEngine::default)
                .expect("start the probe");
        app.wait_for("id:font-probe").expect("the probe renders");
        app.settle();
        app.wait(Duration::from_millis(200))
            .expect("the faces' registration lands");
        app.settle();

        let ruler_m = "M".repeat(RULER);
        let ruler_i = "i".repeat(RULER);
        let m = ruler_widths(&app, &ruler_m);
        assert_eq!(
            m.len(),
            FONT_TRIALS.len(),
            "one ruler line per trial: {m:?}"
        );
        let i = ruler_widths(&app, &ruler_i);
        assert_eq!(i.len(), 1, "the first trial's second line: {i:?}");
        let source_i = i[0].1;
        let table: Vec<String> = FONT_TRIALS
            .iter()
            .zip(&m)
            .map(|(family, (_, width))| format!("{family}: {width}"))
            .collect();
        println!("ruler widths: {}", table.join(" | "));

        // The first trial is what the column sets the editor in. Whatever it
        // resolves to must be monospaced (60 `M`s and 60 `i`s measure alike)
        // and must not be a face the trials after it resolve to either — the
        // proportional ones (the typographic names, DengXian, the engine's
        // fallback) and the system's own mono.
        let (source, source_i) = (m[0].1, source_i);
        assert!(
            (source - source_i).abs() <= 0.5,
            "the source editor did not resolve to a monospaced face: {RULER} M's are \
             {source} wide, {RULER} i's are {source_i}; all: {table:?}"
        );
        // The same as the family's legacy name on its own (the app's face),
        // and neither the system mono nor DengXian (what the spec's list falls
        // through to today).
        let legacy = m[2].1;
        let consolas = m[6].1;
        let dengxian = m[7].1;
        assert!(
            (source - legacy).abs() <= 0.5
                && (source - consolas).abs() > 0.5
                && (source - dengxian).abs() > 0.5,
            "the source editor resolved to a system face: {source}, legacy {legacy}, \
             Consolas {consolas}, DengXian {dengxian}; all: {table:?}"
        );
    }

    /// The box of the row whose laid-out text is `line`, and the editor's.
    fn row_and_editor(app: &Headless, line: &str) -> (Bounds<Points>, Bounds<Points>) {
        let nodes = app.screen().layout_nodes();
        let editor = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("source-editor"))
            .expect("the source editor is laid out")
            .bounds;
        let row = nodes
            .iter()
            .find(|node| {
                node.text
                    .as_ref()
                    .is_some_and(|text| text.text.text == line)
            })
            .unwrap_or_else(|| panic!("the row `{line}` is in view"))
            .bounds;
        (row, editor)
    }

    /// How far the row's centre is from the editor's, in points.
    fn level_offset(app: &Headless, line: &str) -> f32 {
        let (row, editor) = row_and_editor(app, line);
        let row_middle = (row.origin.y.0 + (row.origin.y.0 + row.size.height.0)) / 2.0;
        let editor_middle = (editor.origin.y.0 + (editor.origin.y.0 + editor.size.height.0)) / 2.0;
        row_middle - editor_middle
    }

    #[test]
    fn punctuation_marks_headings_and_backticks_only() {
        let decorations =
            punctuation_decorations("# Plan\nthe `agent` runs\nplain\n", &tokens::DARK);
        let ranges: Vec<(u32, u32, u32)> = decorations
            .iter()
            .map(|d| (d.range.start.line, d.range.start.column, d.range.end.column))
            .collect();
        // The heading's `# ` and the heading line, then the two backticks.
        assert!(ranges.contains(&(0, 0, 2)), "{ranges:?}");
        assert!(ranges.contains(&(0, 0, 6)), "{ranges:?}");
        assert!(ranges.contains(&(1, 4, 5)), "{ranges:?}");
        assert!(ranges.contains(&(1, 10, 11)), "{ranges:?}");
        assert_eq!(ranges.len(), 4, "{ranges:?}");
    }

    /// The camera levels the anchored line with the field's docked reading
    /// position — the editor's own centre — for a line near the top of the
    /// document (the half-viewport inset) and for the last one
    /// (`scroll_beyond_last_line`).
    #[test]
    fn the_camera_levels_an_anchored_line() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<CameraProbe>())).expect("start the probe");
        app.wait_for("label:Markdown source")
            .expect("source editor");
        app.settle();

        // A docked surface with no camera request moves nothing: the document
        // still starts at the top of the column, so its fourth line sits high
        // above the editor's centre (the inset is the camera's room, not a
        // jump).
        let untouched = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            untouched < -100.0,
            "the document moved without a camera request: {untouched} from the centre"
        );

        app.press("id:probe-focus-first").expect("focus line 3");
        app.settle();
        let offset = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "line 3 sits {offset} points off the editor's centre"
        );

        app.press("id:probe-focus-last")
            .expect("focus the last line");
        app.settle();
        let offset = level_offset(&app, "END-OF-THE-DOCUMENT");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "the last line sits {offset} points off the editor's centre"
        );
    }

    /// The source is set the way the design says: Martian Mono at 14/24, and
    /// the Markdown punctuation at 34% ink.
    #[test]
    fn the_source_is_set_in_the_specs_ink() {
        let mut app =
            Headless::start(EngineConfig::new(Root::of::<CameraProbe>())).expect("start the probe");
        app.wait_for("label:Markdown source")
            .expect("source editor");
        app.settle();

        let body = app
            .screen()
            .span_style("text:line 20")
            .expect("a body line's style");
        assert_eq!(body.font_size.0, tokens::TYPE_SOURCE);
        assert_eq!(body.line_height.0, tokens::TYPE_SOURCE_LINE);
        let family = body
            .font_family
            .as_ref()
            .map(|family| family.name().to_string())
            .unwrap_or_default();
        assert!(
            family.contains("Martian Mono"),
            "the source is set in {family:?}"
        );
        let ink = [tokens::DARK.ink, tokens::LIGHT.ink];
        assert!(
            ink.contains(&body.color),
            "the source body is {:?}, not the room's ink",
            body.color
        );

        let marker = app
            .screen()
            .span_style("text:# ")
            .expect("the heading's marker");
        let punctuation = [tokens::DARK.ink_at(0.34), tokens::LIGHT.ink_at(0.34)];
        assert!(
            punctuation.contains(&marker.color),
            "the punctuation is {:?}, not 34% ink",
            marker.color
        );

        // A heading's own line steps up to 92% ink (the prototype's `.ln.h`).
        let heading = app
            .screen()
            .span_style("text:Release plan")
            .expect("the heading's words");
        let stepped = [tokens::DARK.ink_at(0.92), tokens::LIGHT.ink_at(0.92)];
        assert!(
            stepped.contains(&heading.color),
            "the heading's words are {:?}, not 92% ink",
            heading.color
        );
    }
}
