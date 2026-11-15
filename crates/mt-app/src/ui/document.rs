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

use mt_core::settings::Language;
use nsh_base::editor;
use nsh_core::prelude::*;

use crate::{
    state::{AppState, Pane, Tab, language_for_path},
    ui::{
        glow, preview,
        tokens::{self, FONT_SOURCE_UI},
    },
};

/// The column's content for the active document, by pane. Empty while no
/// document is open (the shell shows [`crate::ui::welcome::welcome`] then).
pub fn column(state: AppState, language: Language) -> impl IntoElement {
    let state_for_tabs = state.clone();
    view().flex_1().h(relative(1.0)).child(each(
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
/// Not yet here (document lane): the measure and the punctuation ink, the
/// review camera, the gutter strip, and the `test_id`s the layout tests use.
fn source(state: AppState, tab: Tab) -> impl IntoElement {
    let change_state = state.clone();
    let edit_tab = tab.clone();
    let decorations_state = state.clone();
    let decorations_tab = tab.clone();
    editor()
        .value(move || Some(tab.text.get()))
        .language(Some(SharedString::from(language_for_path(&tab.path))))
        .accessibility_label(Some(SharedString::from("Markdown source")))
        // The editor draws through the platform's text input, which cannot see
        // the app's registered fonts in nsh 0.5.1: `FONT_SOURCE_UI` is the
        // system mono that stands in (see its doc).
        .font_family(FONT_SOURCE_UI)
        .font_size(px(tokens::TYPE_SOURCE))
        .line_height(px(tokens::TYPE_SOURCE_LINE))
        // The measure is the window's, not a horizontal scroll: as the
        // prototype wraps, so does the editor.
        .soft_wrap(true)
        .decorations(move || glow::document_decorations(&decorations_state, &decorations_tab))
        .w_full()
        .flex_1()
        .on_change(move |value: &SharedString| {
            // nsh reports the bound value once on mount. A document is dirty
            // only after the buffer differs from what the editor already has.
            if edit_tab.text.get().as_str() == value.as_str() {
                return;
            }
            edit_tab.text.set(value.clone());
            edit_tab.dirty.set(true);
            edit_tab.revision.update(|revision| *revision += 1);
            change_state.note_edit(edit_tab.id);
        })
}

/// Whether a document is open (the shell asks before it shows the column).
pub fn has_document(state: &AppState) -> bool {
    state.active_tab().is_some()
}
