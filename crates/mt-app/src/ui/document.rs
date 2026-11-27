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
//! Docked surfaces request source positioning through
//! [`AppState::focus_document_line`].

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use nsh_base::{
    editor::{
        Editor, EditorApi, EditorDecoration, EditorPosition, EditorRange, EditorScrollBeyond,
        EditorStyles, editor,
    },
    text_view::PartStyle,
};
use nsh_core::{
    Color, PropertyValue, StyleKey, StyleValue,
    element::{Style, StyleRefinement},
    prelude::*,
};

use crate::{
    state::{AppState, Tab},
    ui::{glow, motion, notices, review, revision, shell, titlebar, tokens},
};

/// The text stage's source view. The shell keys the slot by tab identity;
/// changing the view track never replaces the mounted editor.
pub fn column(state: AppState) -> impl IntoElement {
    let active = state.clone();
    each(
        move || active.active_tab().into_iter().collect::<Vec<_>>(),
        |tab| tab.id,
        move |tab: ReadSignal<Tab>, _| source_document(state.clone(), tab.get_untracked()),
    )
}

fn source_document(state: AppState, tab: Tab) -> impl IntoElement {
    let reference = element_ref();
    let document_tab = tab.id;
    state.attach_document_view(document_tab, reference);
    let focus_state = state.clone();
    #[cfg(test)]
    let reported_focus = state.clone();
    view()
        .test_id("document")
        .focusable(move || !shell::shows_full_preview(&focus_state))
        .element_ref(&reference)
        .on_focus(move || {
            #[cfg(test)]
            reported_focus
                .focus_last
                .set(Some(crate::state::FocusTarget::Document(document_tab)));
        })
        .flex_1()
        .h(relative(1.0))
        // Banners belong above the document, outside this inset.
        .pt(px(tokens::SPACE_5))
        .child(source(state, tab))
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
    // Review selection reads this editor's selection live: once the editor
    // is unmounted, the handle reaches nothing.
    state.attach_source_editor(tab.id, editor_ref);
    let camera = editor_ref;

    // The column's camera: a docked surface asked to level a line, and the
    // column glides there ([`level_line`]). Reading the signal subscribes; a
    // request for another tab is ignored.
    //
    // The camera waits one frame: the half-viewport inset (the `styles` below,
    // which turns on with the same request) gives the editor its room to
    // scroll into, and the editor computes its scroll target from the rows as
    // laid out, so the inset must be a frame old by then.
    //
    // On mount (a new tab or a tab switch), a camera request the column
    // already holds cuts to the line instead of replaying the glide.
    let glide = Glide::default();
    let camera_glide = glide.clone();
    let mounting = Cell::new(true);
    let last_request = Cell::new(None);
    effect(move || {
        let mounted = mounting.replace(false);
        let focus = camera_state.document_focus.get();
        if last_request.replace(Some(focus.request)) == Some(focus.request) {
            return;
        }
        let instant = mounted || !focus.animated;
        if focus.tab != camera_tab.id || !docked_surfaces(&camera_state) {
            return;
        }
        let glide = camera_glide.clone();
        request_animation_frame(move |_| level_line(camera, &glide, focus.line, instant));
    });

    // The half-viewport inset, in points: the visible height of the column's
    // text region, while a docked surface holds a camera request for this tab.
    let viewport = signal(0.0f32);
    let inset_tab = tab.id;
    let room = computed(move || inset(&styles_state, inset_tab, viewport.get()));
    // The inset is room above the text, not a move: the scroll offset grows
    // (or shrinks) with it in the same update, so the text stays where it was
    // and the camera glides from there instead of the document first jumping
    // half a viewport.
    let held = Cell::new(0.0f32);
    effect(move || {
        let now = room.get();
        let before = held.replace(now);
        if now == before {
            return;
        }
        if let Some(api) = camera.api::<EditorApi>() {
            let offset = api.scroll_offset();
            api.set_scroll_offset(Point::new(offset.x, offset.y + px(now - before)));
        }
    });
    let styles = move || {
        let mut styles = EditorStyles::default();
        styles.content = Some(PartStyle::from(Style::from(
            StyleRefinement::default()
                .pt(px(room.get()))
                // The editor draws its text in the theme's `foreground`; the
                // source's ink is the room's, at the body level.
                .text_color(tokens::palette().ink),
        )));
        let reading = docked_surfaces(&numbers_state);
        styles.gutter = Some(PartStyle::from(gutter_style(reading)));
        styles.line_number = Some(PartStyle::from(line_number_style(reading)));
        styles
    };

    // The punctuation ink is built once per text (and appearance), not once
    // per binding read: a long document is scanned on a change, not a frame.
    let punctuation =
        computed(move || punctuation_decorations(&punctuation_tab.text.get(), tokens::palette()));

    // `Ctrl+Enter` stays the app's from the editor: the editor binds its own
    // `mod+enter` (insert a line below) each time it takes the focus, and the
    // keymap fires the newest binding. nsh offers no binding priority, so the
    // app binds again after the editor, on both signs of the focus arriving:
    // the editor's own `Focus` event (queued after the round that bound its
    // keys; alone it lost on the Windows host's first launch) and the box's
    // focus-in (heard at the end of the focus round; alone it lost in about
    // one headless `--lib` run in four). The later of the two wins.
    let focuses = signal(0u64);
    let editor_box = element_ref();
    let focus_state = state.clone();
    #[cfg(test)]
    let box_focus_state = state.clone();
    editor_box.on_focus_in(move || {
        focuses.update(|count| *count += 1);
        focus_state.source_editor_focused.set(true);
        #[cfg(test)]
        box_focus_state
            .focus_last
            .set(Some(crate::state::FocusTarget::Editor));
    });
    let blur_state = state.clone();
    editor_box.on_focus_out(move || blur_state.source_editor_focused.set(false));
    let keys_state = state.clone();
    effect(move || {
        focuses.get();
        titlebar::bind_mod_enter(&keys_state);
    });

    let mount_focus = state.clone();
    let tab_id = tab.id;
    let first_layout = Cell::new(true);
    view()
        .test_id("source")
        .flex_1()
        .min_h(px(0.0))
        .h(relative(1.0))
        .on_layout(move |frame: &Bounds<Points>| {
            viewport.set(frame.size.height.0);
            if first_layout.replace(false) {
                // Layout is delivered once the editor's native input exists.
                mount_focus.focus_on_document_mount(tab_id);
            }
        })
        .child(
            // The editor's element carries no `test_id` of its own; the box
            // around it does, and it is the editor's own box.
            view()
                .test_id("source-editor")
                .element_ref(&editor_box)
                // The editor reaches back over the column's left gap: its
                // gutter fills the gap and its text starts at the measure,
                // where it would without a gutter.
                .ml(px(-tokens::COLUMN_GAP))
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
                        // `tokens::FONT_SOURCE` leads with the face's legacy family
                        // name (`Martian Mono Cn Rg`), the name the Windows text
                        // engine resolves the registered face by; measured with 60
                        // `M`s at 14pt against the system faces — see
                        // `the_editor_draws_the_registered_source_font`.
                        .font_family(tokens::FONT_SOURCE)
                        .font_size(px(tokens::TYPE_SOURCE))
                        .line_height(px(tokens::TYPE_SOURCE_LINE))
                        // The strip of gutter marks: the spec's 2px heat tick and
                        // inference edge (the editor's default is 3).
                        .gutter_decorations_width(px(2.0))
                        // The measure is the window's, not a horizontal scroll: as the
                        // prototype wraps, so does the editor.
                        .soft_wrap(true)
                        // Always on, so the gutter is always laid out: it
                        // sits in the column's left gap ([`gutter_style`])
                        // and only its numbers' opacity follows a reading
                        // pass, so opening one never moves your text.
                        .line_numbers(true)
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
                            // The conflict's dashed underlines, under the review's
                            // light: the same editor, the same rows, one pass.
                            decorations.extend(notices::conflict_decorations(
                                &decorations_state,
                                &decorations_tab,
                            ));
                            // The revision's cold strikes on the words its
                            // undecided hunks would remove.
                            decorations.extend(revision::column_decorations(
                                &decorations_state,
                                &decorations_tab,
                            ));
                            decorations
                        })
                        .w_full()
                        .flex_1()
                        .on_focus({
                            #[cfg(test)]
                            let focus_state = state.clone();
                            move || {
                                focuses.update(|count| *count += 1);
                                #[cfg(test)]
                                focus_state
                                    .focus_last
                                    .set(Some(crate::state::FocusTarget::Editor));
                            }
                        })
                        .on_change(move |value: &SharedString| {
                            // nsh reports the bound value once on mount. A document is
                            // dirty only after the buffer differs from what the editor
                            // already has.
                            if edit_tab.text.get().as_str() == value.as_str() {
                                return;
                            }
                            // Typing is the user's: the camera lets go.
                            glide.stop();
                            edit_tab.text.set(value.clone());
                            edit_tab.dirty.set(true);
                            edit_tab.revision.update(|revision| *revision += 1);
                            change_state.note_edit(edit_tab.id);
                        }),
                ),
        )
}

/// The camera's glide in flight, if any. A new target, the user's own scroll
/// and the user's typing end it.
#[derive(Clone, Default)]
struct Glide(Rc<RefCell<Option<Spawned<()>>>>);

impl Glide {
    /// Ends the glide in flight, leaving the column where it is.
    fn stop(&self) {
        if let Some(task) = self.0.borrow_mut().take() {
            task.cancel();
        }
    }

    /// Runs `task` as the glide, ending the one before it.
    fn run(&self, task: Spawned<()>) {
        self.stop();
        *self.0.borrow_mut() = Some(task);
    }
}

fn level_line(editor: ComponentRef<Editor>, glide: &Glide, line: u32, instant: bool) {
    let Some(api) = editor.api::<EditorApi>() else {
        return;
    };
    glide.stop();
    let position = EditorPosition::new(line, 0);
    if instant || motion::reduced_motion() {
        api.reveal_position(position, true);
        return;
    }
    let from = api.scroll_offset();
    // Measure the editor's own reveal target, then restore the starting offset
    // so measuring or retargeting a glide does not jump to its end.
    api.reveal_position(position, true);
    let to = api.scroll_offset().y.0;
    api.set_scroll_offset(from);
    if (to - from.y.0).abs() < 0.5 {
        return;
    }
    glide.run(spawn(glide_to(editor, from.y.0, to)));
}

async fn glide_to(editor: ComponentRef<Editor>, from: f32, to: f32) {
    let start = nsh_core::time::now();
    let mut own = [from, from];
    loop {
        timer(motion::CAMERA_TICK).await;
        let Some(api) = editor.api::<EditorApi>() else {
            return;
        };
        let at = api.scroll_offset().y.0;
        // Late native scroll reports can still show the previous tick or the
        // probe reveal, so neither counts as a user scroll.
        if own
            .iter()
            .chain(std::iter::once(&to))
            .all(|set| (at - set).abs() > 1.0)
        {
            return;
        }
        let elapsed = (nsh_core::time::now() - start).as_secs_f32() * 1000.0;
        let finished = motion::reduced_motion() || elapsed >= motion::CAMERA_MS;
        let y = if finished {
            to
        } else {
            from + (to - from) * motion::camera_progress(elapsed)
        };
        api.set_scroll_offset(Point::new(px(0.0), px(y)));
        own = [own[1], y];
        if finished {
            return;
        }
    }
}

/// Whether a document is open (the shell asks before it shows the column).
pub fn has_document(state: &AppState) -> bool {
    state.active_tab().is_some()
}

/// Whether a surface that levels lines with its docked reading position is in
/// the field (the review, a revision, a conflict comparison). The column shows
/// the editor's line numbers — a reading pass's gutter — only then; the context
/// reads a document of its own and never moves this one.
fn docked_surfaces(state: &AppState) -> bool {
    review::is_open(state) || revision::is_open(state) || notices::compare_is_open(state)
}

// The editor clamps scroll at zero, so centering an early line needs space
// above the first row. Opening a field without a camera request adds no inset.
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

/// The line numbers' size: 11px, the active line's as every other's.
const LINE_NUMBER_SIZE: f32 = 11.0;

/// The room between a line number's right edge and your text's first glyph
/// (the editor's content adds 4px of its own).
const GUTTER_TO_TEXT: f32 = 6.0;

/// A style of declarations, each written as its JS value; the editor replays
/// a part's style through [`Style::decls`] where a row is in more than one
/// part (the active line's number), which a typed refinement has none of.
fn declared(values: &[(StyleKey, StyleValue<'_>)]) -> Style {
    values.iter().fold(Style::default(), |style, (key, value)| {
        let value = key
            .parse_value(*value)
            .unwrap_or_else(|_| panic!("`{}` does not take {value:?}", key.name()));
        style.with(*key, value)
    })
}

/// The gutter: exactly the column's left gap wide, its numbers set right
/// against your text. Its width never changes — not with the line count, not
/// with a finding's gutter tick, not with a reading pass — so the text never
/// moves. While nobody reads the document it takes no presses: the numbers
/// are invisible then, and a press there is not a line selection.
fn gutter_style(reading: bool) -> Style {
    declared(&[
        (StyleKey::Width, StyleValue::Number(tokens::COLUMN_GAP)),
        (StyleKey::PaddingLeft, StyleValue::Number(0.0)),
        (StyleKey::PaddingRight, StyleValue::Number(GUTTER_TO_TEXT)),
        (StyleKey::AlignItems, StyleValue::Str("flex-end")),
        (
            StyleKey::PointerEvents,
            StyleValue::Str(if reading { "auto" } else { "none" }),
        ),
    ])
}

fn line_number_style(reading: bool) -> Style {
    declared(&[
        (StyleKey::FontSize, StyleValue::Number(LINE_NUMBER_SIZE)),
        (
            StyleKey::Opacity,
            StyleValue::Number(if reading { 1.0 } else { 0.0 }),
        ),
    ])
    .with(
        StyleKey::Color,
        PropertyValue::Color(Some(tokens::palette().ink_faint)),
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::OnceLock, time::Duration};

    use nsh_engine::{EngineConfig, Root};
    use nsh_testing::Headless;

    use super::*;
    use crate::state::AppState;
    use crate::state::Pane;

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
        let preview_state = state.clone();
        let source_state = state.clone();
        let close_review_state = state.clone();
        let open_review_state = state.clone();
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
            .child(
                view()
                    .test_id("probe-pane-preview")
                    .w_full()
                    .h(px(8.0))
                    .on_press(move || {
                        if let Some(tab) = preview_state.active_tab() {
                            preview_state.set_pane(tab.id, Pane::Preview);
                        }
                    }),
            )
            .child(
                view()
                    .test_id("probe-pane-source")
                    .w_full()
                    .h(px(8.0))
                    .on_press(move || {
                        if let Some(tab) = source_state.active_tab() {
                            source_state.set_pane(tab.id, Pane::Source);
                        }
                    }),
            )
            .child(
                view()
                    .test_id("probe-review-close")
                    .w_full()
                    .h(px(8.0))
                    .on_press(move || close_review_state.close_review()),
            )
            .child(
                view()
                    .test_id("probe-review-open")
                    .w_full()
                    .h(px(8.0))
                    .on_press(move || open_review_state.review_open.set(true)),
            )
            .child(column(state))
    }

    /// The characters of a ruler line.
    #[cfg(windows)]
    const RULER: usize = 60;

    /// One family a probe tries: the label its editor carries and the width
    /// tables print, the family list the app sets text in, and the weight the
    /// surface pairs with it.
    #[cfg(windows)]
    #[derive(Clone, Copy)]
    struct FontTrial {
        label: &'static str,
        family: &'static str,
        weight: u16,
    }

    /// The Latin roles and the faces each must resolve to, with the system's
    /// faces and a family that does not exist (the engine's own fallback) as
    /// the contrasts.
    ///
    /// The app's faces are only reachable by their files' legacy family names
    /// (name table ID 1 — `Martian Grotesk Std Lt` and friends, the name
    /// DirectWrite's `FindFamilyName` matches), so a list that names the
    /// typographic family first resolves to a *system* face, which the `M`
    /// ruler sees as a different width.
    #[cfg(windows)]
    const LATIN_TRIALS: [FontTrial; 11] = [
        FontTrial {
            label: "FONT_UI",
            family: tokens::FONT_UI,
            weight: 400,
        },
        FontTrial {
            label: "FONT_MODEL",
            family: tokens::FONT_MODEL,
            weight: 300,
        },
        FontTrial {
            label: "FONT_MEDIUM",
            family: tokens::FONT_MEDIUM,
            weight: 500,
        },
        FontTrial {
            label: "FONT_SOURCE",
            family: tokens::FONT_SOURCE,
            weight: 400,
        },
        FontTrial {
            label: "Std Rg",
            family: "Martian Grotesk Std Rg",
            weight: 400,
        },
        FontTrial {
            label: "Std Lt",
            family: "Martian Grotesk Std Lt",
            weight: 300,
        },
        FontTrial {
            label: "Std Md",
            family: "Martian Grotesk Std Md",
            weight: 500,
        },
        FontTrial {
            label: "Cn Rg",
            family: "Martian Mono Cn Rg",
            weight: 400,
        },
        FontTrial {
            label: "Consolas",
            family: "Consolas",
            weight: 400,
        },
        FontTrial {
            label: "DengXian",
            family: "DengXian",
            weight: 400,
        },
        FontTrial {
            label: "absent",
            family: "No Such Family 12345",
            weight: 400,
        },
    ];

    /// A `―` (U+2015) ruler: no Martian face has the character, and its
    /// advance is 1.0 em in both DengXian cuts against 0.448 em in Microsoft
    /// YaHei UI — so the width says whose glyphs the fallback took.
    #[cfg(windows)]
    const DASH_RULER: usize = 20;

    /// An `ǔ` (U+01D4) ruler: 0.5508 em in DengXian against 0.5352 em in
    /// DengXian Light — about 13 points apart over the ruler — and no Martian
    /// face has it, so the width says which cut the CJK fallback took.
    #[cfg(windows)]
    const PINYIN_RULER: usize = 60;

    /// One editor per Latin trial with a 60-character `M` ruler, and, last,
    /// the mono's `i` ruler, so a fixed-pitch face and a proportional one can
    /// be told apart by their widths alone.
    #[cfg(windows)]
    #[component]
    fn FontProbe() -> impl IntoElement {
        let ruler_m: SharedString = "M".repeat(RULER).into();
        let ruler_i: SharedString = "i".repeat(RULER).into();
        let mut probe = view()
            .test_id("font-probe")
            .flex_col()
            .w(px(760.0))
            .h(px(700.0));
        for trial in LATIN_TRIALS {
            probe = probe.child(font_editor(trial, ruler_m.clone()));
        }
        probe.child(font_editor(
            FontTrial {
                label: "FONT_SOURCE-i",
                family: tokens::FONT_SOURCE,
                weight: 400,
            },
            ruler_i,
        ))
    }

    /// The dash trials distinguish the Windows fallback faces.
    #[cfg(windows)]
    const CJK_TRIALS: [FontTrial; 8] = [
        FontTrial {
            label: "FONT_UI",
            family: tokens::FONT_UI,
            weight: 400,
        },
        FontTrial {
            label: "FONT_MODEL",
            family: tokens::FONT_MODEL,
            weight: 300,
        },
        FontTrial {
            label: "FONT_SOURCE",
            family: tokens::FONT_SOURCE,
            weight: 400,
        },
        FontTrial {
            label: "DengXian",
            family: "DengXian",
            weight: 400,
        },
        FontTrial {
            label: "DengXian Light",
            family: "DengXian Light",
            weight: 300,
        },
        FontTrial {
            label: "Microsoft YaHei UI",
            family: "Microsoft YaHei UI",
            weight: 400,
        },
        FontTrial {
            label: "Microsoft YaHei UI 300",
            family: "Microsoft YaHei UI",
            weight: 300,
        },
        FontTrial {
            label: "absent",
            family: "No Such Family 12345",
            weight: 400,
        },
    ];

    /// The pinyin trials distinguish the companion from a missing family
    /// and identify the companion or DengXian cut each list uses.
    #[cfg(windows)]
    const PINYIN_TRIALS: [FontTrial; 8] = [
        FontTrial {
            label: "FONT_UI",
            family: tokens::FONT_UI,
            weight: 400,
        },
        FontTrial {
            label: "FONT_MODEL",
            family: tokens::FONT_MODEL,
            weight: 300,
        },
        FontTrial {
            label: "FONT_SOURCE",
            family: tokens::FONT_SOURCE,
            weight: 400,
        },
        FontTrial {
            label: "Source Han Sans CN VF",
            family: "Source Han Sans CN VF",
            weight: 400,
        },
        FontTrial {
            label: "Source Han Sans CN VF 300",
            family: "Source Han Sans CN VF",
            weight: 300,
        },
        FontTrial {
            label: "absent",
            family: "No Such Family 12345",
            weight: 400,
        },
        FontTrial {
            label: "DengXian",
            family: "DengXian",
            weight: 400,
        },
        FontTrial {
            label: "DengXian Light",
            family: "DengXian Light",
            weight: 300,
        },
    ];

    /// The CJK probe: every trial's `―` ruler first, then the `ǔ` rulers of
    /// [`PINYIN_TRIALS`].
    #[cfg(windows)]
    #[component]
    fn CjkProbe() -> impl IntoElement {
        let dash: SharedString = "―".repeat(DASH_RULER).into();
        let mut probe = view()
            .test_id("cjk-probe")
            .flex_col()
            .w(px(760.0))
            .h(px(700.0));
        for trial in CJK_TRIALS {
            probe = probe.child(font_editor(trial, dash.clone()));
        }
        for trial in PINYIN_TRIALS {
            probe = probe.child(font_editor(trial, "ǔ".repeat(PINYIN_RULER).into()));
        }
        probe
    }

    /// An editor of a fixed height set in the trial's family and weight.
    #[cfg(windows)]
    fn font_editor(trial: FontTrial, text: SharedString) -> impl IntoElement {
        view().test_id(trial.label).w_full().h(px(56.0)).child(
            editor()
                .default_value(text)
                .font_family(trial.family)
                .font_weight(nsh_core::element::FontWeight(trial.weight))
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

    /// A probe app on the platform's own DirectWrite engine — the one the app
    /// draws with — with the embedded faces travelling with the root, as
    /// `main.rs` hands them to the app's, and the registration given its 200ms
    /// to land.
    #[cfg(windows)]
    fn font_app(root: Root, ready: &str) -> Headless {
        use nsh_platform_windows::DWriteTextEngine;

        let mut root = root;
        for face in crate::ui::fonts::faces() {
            root = root.font(face);
        }
        let mut app =
            Headless::start_with_text_engine(EngineConfig::new(root), DWriteTextEngine::default)
                .expect("start the probe");
        app.wait_for(ready).expect("the probe renders");
        app.settle();
        app.wait(Duration::from_millis(200))
            .expect("the faces' registration lands");
        app.settle();
        app
    }

    #[cfg(windows)]
    #[component]
    fn CountWidths() -> impl IntoElement {
        view()
            .test_id("count-widths")
            .flex_row()
            .child(tokens::mono_count(crate::ui::prims::rest("111")))
            .child(tokens::mono_count(crate::ui::prims::rest("000")))
    }

    #[cfg(windows)]
    #[test]
    fn mono_counts_keep_equal_width_for_ones_and_zeroes() {
        let app = font_app(
            nsh_testing::windowed_root::<CountWidths>(),
            "id:count-widths",
        );
        let [(_, ones)] = ruler_widths(&app, "111")[..] else {
            panic!("expected one text node for 111");
        };
        let [(_, zeroes)] = ruler_widths(&app, "000")[..] else {
            panic!("expected one text node for 000");
        };
        assert!(
            ones > 0.0 && (ones - zeroes).abs() < 0.01,
            "{ones} vs {zeroes}"
        );
    }

    /// Every trial's ruler width, in the probe's own order, by trial label.
    #[cfg(windows)]
    fn widths_by_label(
        app: &Headless,
        trials: &[FontTrial],
        ruler: &str,
    ) -> Vec<(&'static str, f32)> {
        let widths = ruler_widths(app, ruler);
        assert_eq!(
            widths.len(),
            trials.len(),
            "one {ruler:?} ruler per trial: {widths:?}"
        );
        trials
            .iter()
            .zip(&widths)
            .map(|(trial, (_, width))| (trial.label, *width))
            .collect()
    }

    /// The trial `label`'s ruler width in `table`.
    #[cfg(windows)]
    fn width_of(table: &[(&'static str, f32)], label: &str) -> f32 {
        table
            .iter()
            .find(|(trial, _)| *trial == label)
            .map(|(_, width)| *width)
            .unwrap_or_else(|| panic!("trial {label} in {table:?}"))
    }

    /// A width table, for a failure's message.
    #[cfg(windows)]
    fn table_text(table: &[(&'static str, f32)]) -> String {
        table
            .iter()
            .map(|(label, width)| format!("{label}: {width}"))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    /// Which family name lets the source editor draw the app's registered
    /// Martian Mono.
    ///
    /// The faces register under their files' legacy family names (nameID 1:
    /// `Martian Mono Cn Rg`), the names DirectWrite's `FindFamilyName`
    /// matches; the typographic name (nameID 16: `Martian Mono`) is a
    /// fallback. A 60-character ruler line measures the same whichever letter
    /// it is exactly when the face that resolved is monospaced.
    ///
    /// Windows-only: it lays text out with the platform's own DirectWrite
    /// engine, the one the app draws with.
    #[cfg(windows)]
    #[test]
    fn the_editor_draws_the_registered_source_font() {
        let app = font_app(nsh_testing::windowed_root::<FontProbe>(), "id:font-probe");
        let m = widths_by_label(&app, &LATIN_TRIALS, &"M".repeat(RULER));
        let i = ruler_widths(&app, &"i".repeat(RULER));
        assert_eq!(i.len(), 1, "the mono's second ruler: {i:?}");
        let table = table_text(&m);
        println!("ruler widths: {table}");

        // The editor is set in `tokens::FONT_SOURCE`. Whatever it resolves to
        // must be the registered mono: the width its own legacy family name
        // measures (not a face a list falls through to — the system's mono,
        // DengXian, the engine's fallback), and monospaced (60 `M`s and 60
        // `i`s measure alike).
        let source = width_of(&m, "FONT_SOURCE");
        assert!(
            (source - width_of(&m, "Cn Rg")).abs() <= 0.5,
            "the source editor does not draw the registered mono: {source} vs \
             {}; {table}",
            width_of(&m, "Cn Rg")
        );
        assert!(
            (source - i[0].1).abs() <= 0.5,
            "the source editor is not monospaced: {RULER} M's are {source} wide, \
             {RULER} i's are {}; {table}",
            i[0].1
        );
        for face in ["Consolas", "DengXian", "absent"] {
            assert!(
                (source - width_of(&m, face)).abs() > 0.5,
                "the source editor fell through to {face}: {table}"
            );
        }
    }

    /// Every role's list resolves to its own Martian face through the engine
    /// the app draws with: the UI and preview body to the `Std Rg` instance,
    /// the model's voice to `Std Lt`, the medium labels to `Std Md`, source
    /// mode to the condensed mono. The three Grotesk rulers differ by their
    /// advance widths, so the equality proves the *instance*, not just the
    /// family.
    #[cfg(windows)]
    #[test]
    fn the_ui_and_model_lists_resolve_to_martian() {
        let app = font_app(nsh_testing::windowed_root::<FontProbe>(), "id:font-probe");
        let table = widths_by_label(&app, &LATIN_TRIALS, &"M".repeat(RULER));
        println!("ruler widths: {}", table_text(&table));

        let (lt, rg, md) = (
            width_of(&table, "Std Lt"),
            width_of(&table, "Std Rg"),
            width_of(&table, "Std Md"),
        );
        assert!(
            lt < rg && rg < md,
            "the instances' rulers must differ: {lt} / {rg} / {md}"
        );
        for (list, face) in [
            ("FONT_UI", "Std Rg"),
            ("FONT_MODEL", "Std Lt"),
            ("FONT_MEDIUM", "Std Md"),
            ("FONT_SOURCE", "Cn Rg"),
        ] {
            let (width, expected) = (width_of(&table, list), width_of(&table, face));
            assert!(
                (width - expected).abs() <= 0.5,
                "{list} resolves to {width}, not {face}'s {expected}: {}",
                table_text(&table)
            );
            for fallback in ["Consolas", "DengXian", "absent"] {
                assert!(
                    (width - width_of(&table, fallback)).abs() > 0.5,
                    "{list} resolves like {fallback}: {}",
                    table_text(&table)
                );
            }
        }
    }

    /// After Martian, the three lists prefer Source Han Sans CN VF when
    /// DirectWrite can resolve it, then DengXian, then Microsoft YaHei UI.
    #[cfg(windows)]
    #[test]
    fn the_cjk_fallback_takes_the_first_available_face() {
        let app = font_app(nsh_testing::windowed_root::<CjkProbe>(), "id:cjk-probe");
        let dash = widths_by_label(&app, &CJK_TRIALS, &"―".repeat(DASH_RULER));
        let pinyin = widths_by_label(&app, &PINYIN_TRIALS, &"ǔ".repeat(PINYIN_RULER));
        println!("dash widths: {}", table_text(&dash));
        println!("pinyin widths: {}", table_text(&pinyin));

        let companion = width_of(&pinyin, "Source Han Sans CN VF");
        if (companion - width_of(&pinyin, "absent")).abs() > 0.5 {
            for (list, face) in [
                ("FONT_UI", "Source Han Sans CN VF"),
                ("FONT_MODEL", "Source Han Sans CN VF 300"),
                ("FONT_SOURCE", "Source Han Sans CN VF"),
            ] {
                assert!(
                    (width_of(&pinyin, list) - width_of(&pinyin, face)).abs() <= 0.5,
                    "{list}'s CJK is not {face}: {}",
                    table_text(&pinyin)
                );
            }
            return;
        }

        let dengxian = width_of(&dash, "DengXian");
        // Each list's YaHei at the list's own weight: the model's 300 draws
        // YaHei's light face, whose ruler differs from the regular one.
        let yahei = |list: &str| {
            width_of(
                &dash,
                if list == "FONT_MODEL" {
                    "Microsoft YaHei UI 300"
                } else {
                    "Microsoft YaHei UI"
                },
            )
        };
        // DengXian is optional on Windows Server images (the CI runner has
        // none); there the lists must fall through to Microsoft YaHei UI.
        let fonts =
            std::path::Path::new(&std::env::var("WINDIR").unwrap_or_default()).join("Fonts");
        if !["Deng.ttf", "Dengl.ttf"]
            .iter()
            .all(|file| fonts.join(file).is_file())
        {
            for list in ["FONT_UI", "FONT_MODEL", "FONT_SOURCE"] {
                let (taken, yahei) = (width_of(&dash, list), yahei(list));
                assert!(
                    (taken - yahei).abs() <= 0.5,
                    "without DengXian, {list}'s CJK is not YaHei's: {taken} against {yahei}: {}",
                    table_text(&dash)
                );
            }
            return;
        }
        for list in ["FONT_UI", "FONT_MODEL", "FONT_SOURCE"] {
            let (taken, yahei) = (width_of(&dash, list), yahei(list));
            assert!(
                (taken - dengxian).abs() <= 0.5 && (taken - yahei).abs() > 0.5,
                "{list}'s CJK is not DengXian's: {taken} against {dengxian} / {yahei}: {}",
                table_text(&dash)
            );
        }

        let regular = width_of(&pinyin, "DengXian");
        let light = width_of(&pinyin, "DengXian Light");
        assert!(
            regular - light > 1.0,
            "the cuts' rulers must differ: {regular} / {light}"
        );
        for (list, cut, width) in [
            ("FONT_MODEL", "DengXian Light", light),
            ("FONT_UI", "DengXian", regular),
            ("FONT_SOURCE", "DengXian", regular),
        ] {
            assert!(
                (width_of(&pinyin, list) - width).abs() <= 0.5,
                "{list}'s CJK is not the {cut} cut: {}",
                table_text(&pinyin)
            );
        }
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
        // On the manual clock, each glide is stepped through to its end: on
        // the system's, a loaded machine could end a settle mid-glide.
        let mut app = glide_probe(false);
        let glide = Duration::from_millis(motion::CAMERA_MS as u64 + 60);

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
        app.run_for(glide).expect("the glide");
        let offset = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "line 3 sits {offset} points off the editor's centre"
        );

        app.press("id:probe-focus-last")
            .expect("focus the last line");
        app.run_for(glide).expect("the glide");
        let offset = level_offset(&app, "END-OF-THE-DOCUMENT");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "the last line sits {offset} points off the editor's centre"
        );
    }

    /// The probe on a manual clock, the camera's glide stepped by the test,
    /// with the column laid out and nothing asked of the camera yet.
    fn glide_probe(reduced: bool) -> Headless {
        let mut config = EngineConfig::new(nsh_testing::windowed_root::<CameraProbe>());
        config.clock = nsh_core::time::Clock::manual(std::time::SystemTime::now());
        let mut app = Headless::start(config).expect("start the probe");
        if reduced {
            app.set_environment("reduced motion", |env| env.reduce_motion = true)
                .expect("reduce motion");
        }
        app.wait_for("label:Markdown source")
            .expect("source editor");
        app.settle();
        app
    }

    #[test]
    fn the_camera_glides_to_the_anchored_line() {
        let mut app = glide_probe(false);
        let start = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(start < -100.0, "the sentinel starts high: {start}");

        app.press("id:probe-focus-first").expect("focus line 3");
        let pressed = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            (pressed - start).abs() <= tokens::TYPE_SOURCE_LINE,
            "the column jumped at the request: {pressed} from the centre (from {start})"
        );

        app.run_for(Duration::from_millis(60)).expect("mid-glide");
        let mid = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            mid > start + 1.0 && mid < -1.0,
            "mid-glide the line sits at {mid}, not between {start} and the centre"
        );

        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64))
            .expect("the glide ends");
        let end = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            end.abs() <= tokens::TYPE_SOURCE_LINE,
            "after the glide line 3 sits {end} points off the editor's centre"
        );
    }

    #[test]
    fn enabling_reduced_motion_finishes_a_running_camera_glide() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-first").expect("focus line 3");
        app.run_for(Duration::from_millis(32)).expect("begin glide");
        let mid = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        app.set_environment("reduced motion", |env| env.reduce_motion = true)
            .expect("reduce motion");
        app.run_for(motion::CAMERA_TICK).expect("next camera tick");
        let end = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            mid < -tokens::TYPE_SOURCE_LINE && end.abs() <= tokens::TYPE_SOURCE_LINE,
            "reduced motion must finish the running glide: {mid} -> {end}"
        );
    }

    /// Under reduced motion the camera cuts: the line is level as soon as the
    /// request is handled, with no time passing.
    #[test]
    fn under_reduced_motion_the_camera_cuts() {
        let mut app = glide_probe(true);
        app.press("id:probe-focus-first").expect("focus line 3");
        let offset = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "under reduced motion line 3 sits {offset} points off the centre at once"
        );
    }

    /// A new request mid-glide retargets from where the column is — no jump
    /// back, no jump ahead — and ends on the new line.
    #[test]
    fn a_new_target_mid_glide_retargets_from_where_the_column_is() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-first").expect("focus line 3");
        app.run_for(Duration::from_millis(60)).expect("mid-glide");
        let (row, _) = row_and_editor(&app, "line 30");
        let before = row.origin.y.0;

        app.press("id:probe-focus-last")
            .expect("focus the last line");
        let (row, _) = row_and_editor(&app, "line 30");
        assert!(
            (row.origin.y.0 - before).abs() <= tokens::TYPE_SOURCE_LINE,
            "the retarget jumped from {before} to {}",
            row.origin.y.0
        );

        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64 + 60))
            .expect("the new glide ends");
        let end = level_offset(&app, "END-OF-THE-DOCUMENT");
        assert!(
            end.abs() <= tokens::TYPE_SOURCE_LINE,
            "after the retarget the last line sits {end} points off the centre"
        );
    }

    /// Cycling views retains the editor and its held camera position.
    #[test]
    fn a_view_cycle_keeps_the_editor_and_held_line() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-first").expect("focus line 3");
        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64 + 60))
            .expect("the glide ends");

        let editor_id = |app: &Headless| {
            app.screen()
                .layout_nodes()
                .iter()
                .find(|node| node.test_id.as_deref() == Some("source-editor"))
                .expect("the source editor stays mounted")
                .id
        };
        let editor = editor_id(&app);
        app.press("id:probe-pane-preview").expect("to Preview");
        assert_eq!(editor_id(&app), editor);
        app.press("id:probe-pane-source").expect("back to Source");
        assert_eq!(editor_id(&app), editor);
        let offset = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            offset.abs() <= tokens::TYPE_SOURCE_LINE,
            "back in Source line 3 sits {offset} points off the centre with no time passing"
        );
    }

    #[test]
    fn reopening_review_does_not_replay_the_last_camera_request() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-last")
            .expect("the result has an anchor near the end");
        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64 + 60))
            .expect("the result's camera glide ends");
        let anchored = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");

        app.press("id:probe-review-close").expect("close Review");
        app.scroll("id:source-editor", px(-500.0))
            .expect("scroll elsewhere");
        app.wait(Duration::from_millis(20))
            .expect("the scroll lands");
        let scrolled = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            (scrolled - anchored).abs() > 2.0 * tokens::TYPE_SOURCE_LINE,
            "the test scroll must leave the result anchor: {anchored} -> {scrolled}"
        );

        app.press("id:probe-review-open")
            .expect("prepare another Review without a result yet");
        app.run_for(Duration::from_millis(60))
            .expect("a frame before another result arrives");
        let reopened = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            (reopened - scrolled).abs() <= 1.0,
            "Review replayed the previous anchor before a new result: {scrolled} -> {reopened}"
        );
    }

    /// The user's typing ends the glide: the column stays where it was when
    /// the text changed, not carried on to the camera's target.
    #[test]
    fn the_users_typing_ends_the_glide() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-first").expect("focus line 3");
        app.run_for(Duration::from_millis(20))
            .expect("early in the glide");
        app.type_text("id:source-editor", "x")
            .expect("the user types");
        let typed = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64))
            .expect("the glide's time passes");
        let after = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            (after - typed).abs() < 1.0,
            "the camera kept moving after the user typed: {typed} -> {after}"
        );
        assert!(
            after < -tokens::TYPE_SOURCE_LINE,
            "the camera finished its glide over the user's typing: {after} from the centre"
        );
    }

    /// The user's own scroll ends the glide: the camera stops fighting the
    /// wheel and leaves the column where the user put it.
    #[test]
    fn the_users_scroll_ends_the_glide() {
        let mut app = glide_probe(false);
        app.press("id:probe-focus-first").expect("focus line 3");
        app.run_for(Duration::from_millis(20))
            .expect("early in the glide");
        app.scroll("id:source-editor", px(-10.0))
            .expect("the user scrolls back");
        let scrolled = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        app.run_for(Duration::from_millis(motion::CAMERA_MS as u64))
            .expect("the glide's time passes");
        let after = level_offset(&app, "PRIVATE-CAMERA-SENTINEL");
        assert!(
            (after - scrolled).abs() < 1.0,
            "the camera kept moving after the user scrolled: {scrolled} -> {after}"
        );
        assert!(
            after < -tokens::TYPE_SOURCE_LINE,
            "the camera finished its glide over the user's scroll: {after} from the centre"
        );
    }

    /// The source is set the way the design says: Martian Mono at 14/24, and
    /// the Markdown punctuation at 34% ink.
    #[test]
    fn the_source_is_set_in_the_specs_ink() {
        let mut app = Headless::start(EngineConfig::new(
            nsh_testing::windowed_root::<CameraProbe>(),
        ))
        .expect("start the probe");
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
