//! Owner lane: notices.
//!
//! Everything the app says when things are not calm: the recovery banner for a
//! checkpointed buffer, the external-change conflict with its disk comparison,
//! the destructive close and quit guard, and the quiet status line at the foot
//! of the column.
//!
//! Contract: the shell asks `has_*` before it places a banner, draws
//! [`recovery_banner`] and [`conflict_banner`] at the top of the column and
//! [`status_line`] at its foot, and puts [`guard`] in the overlay layer when
//! [`guard_is_open`] says so.
//!
//! Afterglow: none of this emits light — a notice is local, so it is ambient
//! ink text, hairlines and the designed ring on the one default action. The
//! single warm-grey mark is the conflict's dashed underline, which means "this
//! differs from the file", never "this is the model's".
//!
//! The guard card asks through the state's [`crate::state::GuardRequest`]
//! instead of the platform's dialog, so a close or a quit is decided inside
//! the room: Save, Don't save or Cancel, with Cancel selected first. The card
//! keeps its own selection ring: Left/Right (or Tab/Shift+Tab) move it,
//! Enter takes the selected control's decision, and Escape always cancels.
//! Every control carries an accessibility label.

use std::{
    cell::RefCell,
    ops::Range,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use mt_core::{recovery::RecoveredRecord, settings::Language};
use nsh_base::editor::{EditorDecoration, EditorPosition, EditorRange};
use nsh_core::{
    Color, DecorationStyle, PropertyValue, StyleKey, TextDecoration,
    element::{KeyOptions, Style, on_key_with},
    keymap::When,
    prelude::*,
};

use crate::{
    i18n::{self, Key},
    state::{
        AppState, GUARD_CANCEL_CHOICE, GUARD_DISCARD_CHOICE, GUARD_SAVE_CHOICE, GuardRequest, Tab,
    },
    ui::{
        motion::{HOVER_MS, reduced_motion},
        prims,
        tokens::{
            self, CONSENT_BUTTON_H, CONSENT_BUTTON_W, FOCUS_RING, FOCUS_RING_OFFSET, HAIRLINE,
            SPACE_2, SPACE_3, SPACE_4, SPACE_5,
        },
    },
};

/// The guard card's width: the consent card's widest, so its three controls
/// ([`CONSENT_BUTTON_W`] × 3 and the gaps) sit on one row inside the padding.
const GUARD_CARD_W: f32 = tokens::CONSENT_CARD_MAX_W;
/// How many lines of a disk difference the comparison shows before it
/// summarises the rest.
const COMPARE_MAX_LINES: usize = 12;
/// The most cells (buffer lines × disk lines) the line comparison may fill
/// before it gives up on pairing and calls the whole region one difference.
const COMPARE_LINE_CELLS: usize = 1_000_000;
/// The most cells a word comparison may fill before the whole region's words
/// are marked instead of paired.
const COMPARE_WORD_CELLS: usize = 40_000;

// ─── recovery ─────────────────────────────────────────────────────────────

/// Whether a recovered checkpoint is waiting.
pub fn has_recovery(state: &AppState) -> bool {
    !state.recovery_records.get().is_empty()
}

/// The recovery banner: unsaved work that survived a crash or a quick quit.
/// One row per checkpoint, each with its own Restore and Discard.
pub fn recovery_banner(state: AppState, language: Language) -> impl IntoElement {
    let rows_state = state.clone();
    view()
        .test_id("recovery-banner")
        .w_full()
        .flex_col()
        .pt(px(SPACE_4))
        .child(each(
            move || rows_state.recovery_records.get(),
            |record: &RecoveredRecord| record.record.key.clone(),
            move |record: ReadSignal<RecoveredRecord>, index: ReadSignal<usize>| {
                let record = record.get();
                let index = index.get();
                let label = recovered_line(&record, language);
                let restore_state = state.clone();
                let discard_state = state.clone();
                let ring_state = state.clone();
                let restored = record.clone();
                let discarded = record;
                let restore_id = format!("recovery-restore-{index}");
                let discard_id = format!("recovery-discard-{index}");
                let restore = move || {
                    // The restored document's signals must be the app's: this
                    // row is removed by the press itself.
                    restore_state.owned(|| restore_state.restore_recovered(&restored));
                };
                let mut row = view()
                    .test_id(&format!("recovery-row-{index}"))
                    .w_full()
                    .flex_row()
                    .items_center()
                    .gap(px(SPACE_3))
                    .pb(px(SPACE_3))
                    .child(view().flex_1().child(prims::ink(label)));
                // One ringed action at a time: the first record's Restore,
                // unless the conflict banner owns the ring.
                if index == 0 {
                    let ringed = restore.clone();
                    let ringed_id = restore_id.clone();
                    row = row.child(
                        when(
                            move || !has_conflict(&ring_state),
                            move || {
                                prims::primary_action(
                                    &ringed_id,
                                    s(S::Restore, language),
                                    ringed.clone(),
                                )
                                .into_element()
                            },
                        )
                        .otherwise(move || {
                            prims::quiet_action(
                                &restore_id,
                                s(S::Restore, language),
                                restore.clone(),
                            )
                            .into_element()
                        }),
                    );
                } else {
                    row = row.child(prims::quiet_action(
                        &restore_id,
                        s(S::Restore, language),
                        restore,
                    ));
                }
                row.child(prims::quiet_action(
                    &discard_id,
                    s(S::Discard, language),
                    move || discard_state.discard_recovered(&discarded),
                ))
                .into_element()
            },
        ))
        .child(banner_rule())
}

/// 'Recovered unsaved changes from 14:02 · notes.md': what was recovered and
/// when, at the file's own name (a draft has none).
fn recovered_line(record: &RecoveredRecord, language: Language) -> String {
    let time = checkpoint_time(record.record.checkpointed_at);
    let name = record
        .record
        .metadata
        .source_path
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| s(S::UntitledDraft, language).to_owned());
    match language {
        Language::English => format!("Recovered unsaved changes from {time} · {name}"),
        Language::Chinese => format!("已恢复 {time} 的未保存更改 · {name}"),
    }
}

/// The checkpoint's clock time, local, as `HH:MM`. std has no local-time
/// conversion, so the offset comes from the platform (see
/// [`local_offset_seconds`]); the `HH:MM` arithmetic itself is
/// [`clock_hhmm`].
fn checkpoint_time(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0);
    clock_hhmm(seconds.saturating_add(local_offset_seconds()))
}

/// `HH:MM` of a Unix-second count, at whatever offset was already applied.
fn clock_hhmm(seconds: i64) -> String {
    let day = seconds.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, (day % 3600) / 60)
}

/// The local time's offset from UTC, in seconds.
///
/// Windows: `GetTimeZoneInformation` reports the zone's bias and whether
/// daylight time is in force (the process already imports kernel32 through
/// std, so no crate is needed). Elsewhere the offset is UTC's: the Linux and
/// macOS builds are compatibility coverage, and the app's public-quality
/// target is Windows.
#[cfg(windows)]
fn local_offset_seconds() -> i64 {
    /// `TIME_ZONE_ID_DAYLIGHT`.
    const DAYLIGHT: u32 = 2;

    /// `SYSTEMTIME`: the eight fields `TIME_ZONE_INFORMATION` lays out for its
    /// transition dates (their values are unused here).
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SysTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    /// `TIME_ZONE_INFORMATION`: the current zone's bias and daylight rules.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct TimeZoneInformation {
        bias: i32,
        standard_name: [u16; 32],
        standard_date: SysTime,
        standard_bias: i32,
        daylight_name: [u16; 32],
        daylight_date: SysTime,
        daylight_bias: i32,
    }

    // SAFETY: the declaration matches `TimeZoneInformation` (all integer
    // fields), which is what the API writes into the buffer.
    unsafe extern "system" {
        fn GetTimeZoneInformation(info: *mut TimeZoneInformation) -> u32;
    }

    // SAFETY: a plain integer struct, and the API fails without writing when
    // it cannot answer.
    let mut info: TimeZoneInformation = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a live, aligned, writable buffer of the declared type.
    let state = unsafe { GetTimeZoneInformation(&mut info) };
    // UTC = local + bias: the conversion back to local negates the sum. A
    // failed call (`TIME_ZONE_ID_INVALID`) leaves the bias at zero, so the
    // time shows as UTC rather than not at all.
    let bias = i64::from(info.bias)
        + if state == DAYLIGHT {
            i64::from(info.daylight_bias)
        } else {
            0
        };
    -bias * 60
}

#[cfg(not(windows))]
fn local_offset_seconds() -> i64 {
    0
}

// ─── conflict ─────────────────────────────────────────────────────────────

/// Whether the active document differs from what is on disk.
pub fn has_conflict(state: &AppState) -> bool {
    state.active_tab().is_some_and(|tab| tab.conflict.get())
}

/// The external-change banner: your file is not what it was on disk. It offers
/// the comparison and the three ways out: keep your text, take the disk's, or
/// write your text to a new file (`save_as_active`, the way the status copy
/// promises).
pub fn conflict_banner(state: AppState, language: Language) -> impl IntoElement {
    let compare_state = state.clone();
    let keep_state = state.clone();
    let disk_state = state.clone();
    let save_as_state = state.clone();
    let panel_state = state.clone();
    view()
        .test_id("conflict-banner")
        .w_full()
        .flex_col()
        .pt(px(SPACE_4))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .gap(px(SPACE_3))
                .child(
                    view()
                        .flex_1()
                        .child(prims::ink(s(S::ChangedOnDisk, language))),
                )
                .child(prims::primary_action(
                    "conflict-compare",
                    s(S::Compare, language),
                    move || toggle_compare(&compare_state),
                ))
                .child(prims::quiet_action(
                    "conflict-keep-mine",
                    s(S::KeepMine, language),
                    move || keep_state.overwrite_active(),
                ))
                .child(prims::quiet_action(
                    "conflict-use-disk",
                    s(S::UseDiskVersion, language),
                    move || disk_state.reload_active(),
                ))
                .child(prims::quiet_action(
                    "conflict-save-as",
                    i18n::t(language, Key::SaveAsPicker),
                    move || save_as_state.save_as_active(),
                )),
        )
        .child(when(
            {
                let state = panel_state.clone();
                move || compare_open(&state).is_some()
            },
            move || compare_panel(panel_state.clone(), language).into_element(),
        ))
        .child(banner_rule())
}

/// Whether `state`'s active tab has its comparison open: the comparison was
/// asked for *and* its disk text is there to show. The text is read when the
/// comparison opens and dropped when a new conflict is noticed, so an old
/// read can never reopen, or be shown as, this conflict's disk version.
fn compare_open(state: &AppState) -> Option<Tab> {
    let tab = state.active_tab()?;
    let asked = tab.conflict.get() && state.conflict_compare.get() == Some(tab.id);
    (asked && disk_text(state, &tab).is_some()).then_some(tab)
}

/// Whether a conflict comparison is on screen — the column's lines are being
/// levelled against the disk version, so `ui::document` treats it as a docked
/// reading surface (its camera, its inset, its line numbers).
pub fn compare_is_open(state: &AppState) -> bool {
    compare_open(state).is_some()
}

/// Fold the comparison open (at its first difference), or away.
fn toggle_compare(state: &AppState) {
    let Some(tab) = state.active_tab() else {
        return;
    };
    // What is *shown* decides, not the marker: after a conflict is noticed
    // again the marker may still name this tab (from the conflict before), and
    // pressing Compare must open the comparison, not close nothing.
    if compare_open(state).is_some() {
        state.conflict_compare.set(None);
    } else {
        // Read the file as it is now: the conflict was noticed earlier.
        state.refresh_conflict_disk(&tab);
        state.conflict_difference.set(0);
        state.conflict_compare.set(Some(tab.id));
        focus_line(state, &tab);
    }
}

/// Level the column on the comparison's current difference, the way a review
/// racks a finding.
fn focus_line(state: &AppState, tab: &Tab) {
    if let Some(line) = conflict_focus_line(state, tab) {
        state.focus_document_line(tab.id, line);
    }
}

/// `tab`'s file-on-disk text, as read for the comparison.
fn disk_text(state: &AppState, tab: &Tab) -> Option<String> {
    state
        .conflict_disks
        .get()
        .into_iter()
        .find(|(id, _)| *id == tab.id)
        .map(|(_, text)| text)
}

/// Close the comparison, keeping the banner's decisions on screen.
fn close_compare(state: &AppState) {
    state.conflict_compare.set(None);
}

/// Walk to the next (`delta` +1) or previous (−1) difference, wrapping.
fn step_difference(state: &AppState, delta: isize) {
    let Some(tab) = compare_open(state) else {
        return;
    };
    let Some(disk) = disk_text(state, &tab) else {
        return;
    };
    let total = differences(&tab.text.get(), &disk).len();
    if total == 0 {
        return;
    }
    let current = state.conflict_difference.get().min(total - 1) as isize;
    let next = (current + delta).rem_euclid(total as isize) as usize;
    state.conflict_difference.set(next);
    focus_line(state, &tab);
}

/// The comparison: the counter, the keys, and the disk version of the current
/// difference with its differing words dashed warm-grey. Entering it binds J,
/// K and Escape.
///
/// Its lines are keyed by (difference, line), so stepping to another
/// difference replaces them, and the counter is bound to the state.
fn compare_panel(state: AppState, language: Language) -> impl IntoElement {
    bind_compare_keys(&state);
    let counter_state = state.clone();
    let lines_state = state.clone();
    let hidden_state = state.clone();
    let back_state = state.clone();
    let forward_state = state.clone();
    view()
        .test_id("conflict-compare")
        .w_full()
        .flex_col()
        .gap(px(SPACE_2))
        .pt(px(SPACE_4))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .gap(px(SPACE_2))
                .child(
                    text(move || counter_text(&counter_state, language))
                        .font_family(tokens::FONT_UI)
                        .text_size(px(tokens::TYPE_UI))
                        .line_height(px(tokens::TYPE_UI_LINE))
                        .tabular_nums()
                        .text_color(move || tokens::palette().ink_rest),
                )
                .child(view().flex_1())
                .child(prims::kbd("J"))
                .child(prims::kbd("K"))
                .child(prims::quiet_action(
                    "conflict-previous",
                    s(S::Previous, language),
                    move || step_difference(&back_state, -1),
                ))
                .child(prims::quiet_action(
                    "conflict-next",
                    s(S::Next, language),
                    move || step_difference(&forward_state, 1),
                )),
        )
        .child(
            view()
                .w_full()
                .flex_col()
                .pl(px(SPACE_3))
                .border_l(px(HAIRLINE))
                .border_l_color(move || tokens::palette().hairline)
                .child(each(
                    move || compare_lines(&lines_state),
                    |line: &CompareLine| (line.difference, line.line),
                    move |line: ReadSignal<CompareLine>, _| {
                        compare_line(&line.get().spans).into_element()
                    },
                )),
        )
        .child(when(
            {
                let state = hidden_state.clone();
                move || hidden_lines(&state) > 0
            },
            move || prims::faint(more_lines_text(hidden_lines(&hidden_state), language)),
        ))
}

/// One line of the disk region: source type, ink glyphs, and a dashed
/// warm-grey underline under the words the buffer does not have.
fn compare_line(spans: &[Marked]) -> impl IntoElement {
    let content: SharedString = if spans.is_empty() {
        // A blank source line keeps its row.
        " ".into()
    } else {
        SharedString::default()
    };
    let mut line = text(content)
        // The app's mono: a text view draws through the engine, which sees
        // the registered faces under their legacy family names.
        .font_family(tokens::FONT_SOURCE)
        .text_size(px(tokens::TYPE_SOURCE))
        .line_height(px(tokens::TYPE_SOURCE_LINE))
        .text_color(move || tokens::palette().ink);
    for (chunk, differs) in spans {
        let span = text(chunk.clone());
        line = line.child(if *differs {
            span.underline()
                .text_decoration_dashed()
                .text_decoration_color(move || tokens::palette().conflict)
                .into_element()
        } else {
            span.into_element()
        });
    }
    line
}

/// One line of the disk region, with its words.
#[derive(Clone, PartialEq)]
struct CompareLine {
    /// Which difference it belongs to (the panel's key).
    difference: usize,
    /// Its ordinal within that difference.
    line: usize,
    /// Its words.
    spans: Vec<Marked>,
}

/// One run of a region's text: its words, and whether the buffer does not
/// have them (the dashed mark). Whitespace never marks.
type Marked = (String, bool);

/// 'Difference 2 of 5' for the comparison's current state, empty while it is
/// closed or nothing differs.
fn counter_text(state: &AppState, language: Language) -> String {
    let Some((index, total)) = compare_summary(state) else {
        return String::new();
    };
    difference_text(index + 1, total, language)
}

/// Which difference of how many the comparison shows, clamped to what
/// exists.
fn compare_summary(state: &AppState) -> Option<(usize, usize)> {
    let tab = compare_open(state)?;
    let disk = disk_text(state, &tab)?;
    let total = differences(&tab.text.get(), &disk).len();
    if total == 0 {
        return None;
    }
    Some((state.conflict_difference.get().min(total - 1), total))
}

/// The disk lines of the comparison's current difference, at most
/// [`COMPARE_MAX_LINES`] of them, each marked against the buffer.
fn compare_lines(state: &AppState) -> Vec<CompareLine> {
    compare_region(state).0
}

/// How many lines of the current difference the panel's cap hides.
fn hidden_lines(state: &AppState) -> usize {
    compare_region(state).1
}

/// The comparison's current region: the disk lines the panel shows, and how
/// many more lines the difference has.
fn compare_region(state: &AppState) -> (Vec<CompareLine>, usize) {
    let Some((index, _)) = compare_summary(state) else {
        return (Vec::new(), 0);
    };
    let Some(tab) = compare_open(state) else {
        return (Vec::new(), 0);
    };
    let Some(disk) = disk_text(state, &tab) else {
        return (Vec::new(), 0);
    };
    let mine = tab.text.get().to_string();
    let diffs = differences(&mine, &disk);
    let Some(difference) = diffs.get(index) else {
        return (Vec::new(), 0);
    };
    let mine_lines = split_lines(&mine);
    let disk_lines = split_lines(&disk);
    let mine_region = mine_lines[difference.mine.clone()].join("\n");
    let disk_region = disk_lines[difference.disk.clone()].join("\n");
    let all = marked_lines(&word_marks(&mine_region, &disk_region));
    let hidden = all.len().saturating_sub(COMPARE_MAX_LINES);
    let lines = all
        .into_iter()
        .enumerate()
        .take(COMPARE_MAX_LINES)
        .map(|(line, spans)| CompareLine {
            difference: index,
            line,
            spans,
        })
        .collect();
    (lines, hidden)
}

/// `text.split('\n')` without the trailing empty line a final newline makes
/// (it is the same on both sides, so it never carries a difference).
fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Split marked runs at their newlines, so the panel can draw one text per
/// line. A blank line comes out with no runs; [`compare_line`] gives it a
/// space to keep its row.
fn marked_lines(marked: &[Marked]) -> Vec<Vec<Marked>> {
    let mut lines: Vec<Vec<Marked>> = vec![Vec::new()];
    for (text, differs) in marked {
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            push_run(lines.last_mut().expect("one line is open"), first, *differs);
        }
        for part in parts {
            lines.push(Vec::new());
            push_run(lines.last_mut().expect("just pushed"), part, *differs);
        }
    }
    if lines.len() == 1 && lines[0].is_empty() {
        lines[0].push((" ".to_owned(), false));
    }
    lines
}

/// Add one run to a line, merging into the run before it when both carry the
/// same mark.
fn push_run(line: &mut Vec<Marked>, run: &str, differs: bool) {
    if run.is_empty() {
        return;
    }
    match line.last_mut() {
        Some((text, marked)) if *marked == differs => text.push_str(run),
        _ => line.push((run.to_owned(), differs)),
    }
}

/// One run of lines that differ between the buffer and the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Difference {
    /// The buffer's lines (0-based, half-open) in the difference.
    mine: Range<usize>,
    /// The disk's lines (0-based, half-open) in the difference.
    disk: Range<usize>,
}

/// The runs of lines that differ between `mine` and `disk`: the common head
/// and tail are cut off, the middle is paired by longest common subsequence
/// (so an insertion is one difference, not the rest of the file), and a
/// middle too large to pair is one difference in itself.
fn differences(mine: &str, disk: &str) -> Vec<Difference> {
    let mine_lines = split_lines(mine);
    let disk_lines = split_lines(disk);
    let (head, tail) = common_ends(&mine_lines, &disk_lines);
    let mine_middle = &mine_lines[head..mine_lines.len() - tail];
    let disk_middle = &disk_lines[head..disk_lines.len() - tail];
    let mut runs: Vec<Run> = Vec::new();
    if mine_middle
        .len()
        .saturating_mul(disk_middle.len())
        .saturating_mul(4)
        > COMPARE_LINE_CELLS
    {
        if !mine_middle.is_empty() || !disk_middle.is_empty() {
            runs.push((0, mine_middle.len(), 0, disk_middle.len()));
        }
    } else {
        for run in align_lines(mine_middle, disk_middle) {
            runs.push(run);
        }
    }
    runs.into_iter()
        .map(|(mine_start, mine_end, disk_start, disk_end)| Difference {
            mine: head + mine_start..head + mine_end,
            disk: head + disk_start..head + disk_end,
        })
        .collect()
}

/// The equal-length head and tail shared by both files, in lines.
fn common_ends(mine: &[&str], disk: &[&str]) -> (usize, usize) {
    let shortest = mine.len().min(disk.len());
    let mut head = 0;
    while head < shortest && mine[head] == disk[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < shortest - head && mine[mine.len() - 1 - tail] == disk[disk.len() - 1 - tail] {
        tail += 1;
    }
    (head, tail)
}

/// The runs of non-matching lines in `align_lines`'s walk: each is
/// `(mine start, mine end, disk start, disk end)` within the two slices.
type Run = (usize, usize, usize, usize);

/// Pair two line slices by longest common subsequence and return the runs that
/// did not pair. A matching line ends the run before it, so consecutive
/// inserted, removed or replaced lines come out as one difference.
fn align_lines(mine: &[&str], disk: &[&str]) -> Vec<Run> {
    let (n, m) = (mine.len(), disk.len());
    // `table[i * (m + 1) + j]` is the LCS length of `mine[i..]` and `disk[j..]`.
    let width = m + 1;
    let mut table = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if mine[i] == disk[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut runs: Vec<Run> = Vec::new();
    let mut open: Option<Run> = None;
    while i < n || j < m {
        if i < n && j < m && mine[i] == disk[j] {
            if let Some(run) = open.take() {
                runs.push(run);
            }
            i += 1;
            j += 1;
            continue;
        }
        let run = open.get_or_insert((i, i, j, j));
        if j < m && (i == n || table[i * width + j + 1] >= table[(i + 1) * width + j]) {
            j += 1;
            run.3 = j;
        } else {
            i += 1;
            run.1 = i;
        }
    }
    if let Some(run) = open {
        runs.push(run);
    }
    runs
}

/// The buffer's lines that differ from disk, for the column's conflict marks.
fn differing_lines(mine: &str, disk: &str) -> Vec<u32> {
    let line_count = split_lines(mine).len();
    let mut lines = Vec::new();
    for difference in differences(mine, disk) {
        for line in difference.mine {
            if line < line_count {
                lines.push(line as u32);
            }
        }
    }
    lines
}

/// The column's conflict marks: one dashed warm-grey underline per buffer line
/// that differs from the file on disk.
///
/// `ui::document` (or `ui::glow`) must merge these with the review's
/// decorations — this module does not own the editor — and it must not apply
/// them to a document whose `conflict` is false. The compared file's text is
/// read when the comparison opens ([`crate::state::AppState::refresh_conflict_disk`]);
/// until then there is nothing to mark.
pub fn conflict_decorations(state: &AppState, tab: &Tab) -> Vec<EditorDecoration> {
    if !tab.conflict.get() {
        return Vec::new();
    }
    let mine = tab.text.get().to_string();
    let Some(disk) = disk_text(state, tab) else {
        return Vec::new();
    };
    differing_lines(&mine, &disk)
        .into_iter()
        .map(|line| {
            // `EditorDecoration` is `#[non_exhaustive]`, so it is a default and
            // its fields, not a struct literal.
            #[allow(
                clippy::field_reassign_with_default,
                reason = "nsh-base's EditorDecoration cannot be built from a literal"
            )]
            let mut decoration = EditorDecoration::default();
            // An empty range covers its line's row, which is what the mark
            // styles: the row's text is underlined, dashed, at 8% ink's
            // neighbour (the conflict colour).
            decoration.range =
                EditorRange::new(EditorPosition::new(line, 0), EditorPosition::new(line, 0));
            decoration.line_style = Some(conflict_line_style());
            decoration
        })
        .collect()
}

/// The buffer line the comparison is on, for the document lane to bring into
/// view (`reveal_position`, centred) while `tab`'s comparison is open. The
/// first differing line of the current difference; `None` when the
/// comparison is closed or nothing differs.
pub fn conflict_focus_line(state: &AppState, tab: &Tab) -> Option<u32> {
    if !tab.conflict.get() {
        return None;
    }
    let (index, _) = compare_summary(state)?;
    let disk = disk_text(state, tab)?;
    let diffs = differences(&tab.text.get(), &disk);
    let difference = diffs.get(index)?;
    u32::try_from(difference.mine.start).ok()
}

/// The style of a conflict line's row: a 1px dashed warm-grey underline.
fn conflict_line_style() -> Style {
    let line = TextDecoration {
        underline: true,
        line_through: false,
    };
    Style::default()
        .with(
            StyleKey::TextDecorationLine,
            PropertyValue::TextDecoration(Some(line)),
        )
        .with(
            StyleKey::TextDecorationColor,
            PropertyValue::Color(Some(tokens::palette().conflict)),
        )
        .with(
            StyleKey::TextDecorationStyle,
            PropertyValue::DecorationStyle(Some(DecorationStyle::Dashed)),
        )
}

/// Mark `disk`'s words against `mine`'s: a word the buffer does not have gets
/// the dashed mark. Whitespace never marks. A region too large to pair marks
/// every word it has.
fn word_marks(mine: &str, disk: &str) -> Vec<Marked> {
    let mine_words = words(mine);
    let disk_words = words(disk);
    let mut marked: Vec<Marked> = Vec::with_capacity(disk_words.len());
    let paired = mine_words
        .len()
        .saturating_mul(disk_words.len())
        .saturating_mul(4)
        <= COMPARE_WORD_CELLS;
    let matched = if paired {
        lcs_matches(&mine_words, &disk_words)
    } else {
        Vec::new()
    };
    for (index, word) in disk_words.iter().enumerate() {
        let differs = if paired {
            !matched.contains(&index)
        } else {
            true
        };
        marked.push(((*word).to_owned(), differs && !word.trim().is_empty()));
    }
    marked
}

/// Split text into alternating runs of whitespace and non-whitespace.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0usize;
    let mut whitespace: Option<bool> = None;
    for (index, character) in text.char_indices() {
        let class = character.is_whitespace();
        if whitespace.is_some_and(|previous| previous != class) {
            words.push(&text[start..index]);
            start = index;
        }
        whitespace = Some(class);
    }
    if start < text.len() {
        words.push(&text[start..]);
    }
    words
}

/// Which of `right`'s words have a match in `left`, by longest common
/// subsequence.
fn lcs_matches(left: &[&str], right: &[&str]) -> Vec<usize> {
    let (n, m) = (left.len(), right.len());
    let width = m + 1;
    let mut table = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * width + j] = if left[i] == right[j] {
                table[(i + 1) * width + j + 1] + 1
            } else {
                table[(i + 1) * width + j].max(table[i * width + j + 1])
            };
        }
    }
    let mut matched = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if left[i] == right[j] {
            matched.push(j);
            i += 1;
            j += 1;
        } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matched
}

// ─── guard ────────────────────────────────────────────────────────────────

/// Whether the destructive close or quit guard is on screen.
pub fn guard_is_open(state: &AppState) -> bool {
    state.guard.get().is_some()
}

/// The guard card, in the overlay layer: 'Close without saving?' with Save,
/// Don't save and Cancel. Cancel starts selected — the designed ring shows
/// which control the keyboard is on — and Left/Right (or Tab/Shift+Tab) move
/// the ring, so every decision is keyboard-reachable without moving focus.
pub fn guard(state: AppState, language: Language) -> impl IntoElement {
    match state.guard.get() {
        Some(request) => guard_card(state, request, language).into_element(),
        None => view().test_id("guard-card").into_element(),
    }
}

/// The card itself, for the request the state is holding.
fn guard_card(state: AppState, request: GuardRequest, language: Language) -> AnyElement {
    // The card owns its keys: the source editor gives up the focus.
    state.blur_source_editor();
    bind_guard_keys(&state);
    let names_state = state.clone();
    let save_state = state.clone();
    let discard_state = state.clone();
    let cancel_state = state.clone();
    let choice = request.choice;
    let busy = request.busy;
    let notice = request.notice;
    view()
        .test_id("guard-card")
        .w(px(GUARD_CARD_W))
        .flex_col()
        .gap(px(SPACE_4))
        .p(px(SPACE_5))
        .bg(move || tokens::palette().column_ground)
        .border(px(HAIRLINE))
        .border_color(move || tokens::palette().hairline)
        .child(prims::ink(s(S::CloseWithoutSaving, language)))
        .child(
            text(move || documents_line(&names_state, request.tab, language))
                .font_family(tokens::FONT_UI)
                .text_size(px(tokens::TYPE_UI))
                .line_height(px(tokens::TYPE_UI_LINE))
                .text_color(move || tokens::palette().ink_rest),
        )
        .child(when(
            move || notice.get().is_some(),
            move || {
                text(move || notice.get().unwrap_or_default())
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .text_color(move || tokens::palette().danger)
            },
        ))
        .child(
            view()
                .w_full()
                .flex_row()
                .gap(px(SPACE_2))
                .child(card_action(
                    "guard-save",
                    s(S::Save, language),
                    false,
                    move || choice.get() == GUARD_SAVE_CHOICE,
                    move || {
                        if !busy.get() {
                            save_state.guard_save();
                        }
                    },
                ))
                .child(card_action(
                    "guard-dont-save",
                    s(S::DontSave, language),
                    true,
                    move || choice.get() == GUARD_DISCARD_CHOICE,
                    move || {
                        if !busy.get() {
                            discard_state.guard_discard();
                        }
                    },
                ))
                .child(card_action(
                    "guard-cancel",
                    s(S::Cancel, language),
                    false,
                    move || choice.get() == GUARD_CANCEL_CHOICE,
                    move || {
                        if !busy.get() {
                            cancel_state.guard_cancel();
                        }
                    },
                )),
        )
        .into_element()
}

fn bind_guard_keys(state: &AppState) {
    let escape = state.clone();
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        move |_| escape.guard_cancel(),
    );
    for (key, activate) in [
        ("enter", true),
        ("shift+enter", false),
        ("space", false),
        ("shift+space", false),
    ] {
        let state = state.clone();
        on_key_with(
            key,
            KeyOptions {
                when: When::Always,
                // A pointer-opened guard can leave focus on a background button.
                for_focus: true,
                ..KeyOptions::default()
            },
            move |_| {
                if activate {
                    state.guard_activate();
                }
            },
        );
    }
    for (key, delta) in [
        ("left", -1isize),
        ("shift+tab", -1),
        ("right", 1),
        ("tab", 1),
    ] {
        let state = state.clone();
        on_key_with(
            key,
            KeyOptions {
                when: When::Always,
                ..KeyOptions::default()
            },
            move |_| state.guard_move(delta),
        );
    }
}

/// The documents a guard covers, as one quiet line: their names, at most two
/// of them, then how many more there are.
fn documents_line(state: &AppState, tab: Option<u64>, language: Language) -> String {
    let tabs = state.tabs.get();
    let names: Vec<String> = match tab {
        Some(id) => tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| vec![tab.title.to_string()])
            .unwrap_or_default(),
        // Quitting covers whatever is dirty when the decision is made, so
        // the card lists exactly that, as it changes.
        None => tabs
            .iter()
            .filter(|tab| tab.dirty.get())
            .map(|tab| tab.title.to_string())
            .collect(),
    };
    names_line(&names, language)
}

/// [`documents_line`]'s shape for a list of names.
fn names_line(names: &[String], language: Language) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        2 => names.join(", "),
        count => match language {
            Language::English => format!("{}, {} and {} more", names[0], names[1], count - 2),
            Language::Chinese => format!("{}、{} 等 {} 个文件", names[0], names[1], count),
        },
    }
}

/// One control of the guard card: 164×36, square and flat, never warm — a
/// local action is lit by ambient light only. `danger` is the destructive
/// control's ink, and `selected` draws the designed ring on the control the
/// keyboard is on.
fn card_action(
    test_id: &str,
    label: &'static str,
    danger: bool,
    selected: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let selected = selected.into_reactive();
    view()
        .p(px(FOCUS_RING_OFFSET))
        .border(px(FOCUS_RING))
        .border_color({
            let selected = selected.clone();
            move || {
                if selected.get() {
                    tokens::palette().ink
                } else {
                    Color::TRANSPARENT
                }
            }
        })
        .child(
            view()
                .w(px(CONSENT_BUTTON_W))
                .h(px(CONSENT_BUTTON_H))
                .flex_col()
                .items_center()
                .justify_center()
                .bg(Color::TRANSPARENT)
                .border(px(HAIRLINE))
                .border_color(move || {
                    if danger {
                        tokens::palette().hairline
                    } else {
                        tokens::palette().ink_caption
                    }
                })
                .transition_colors()
                .duration(HOVER_MS)
                .child(
                    // The control itself carries the test id (UI Automation's
                    // AutomationId): a plain press target, as `prims`' are.
                    view()
                        .test_id(test_id)
                        .role(AccessibilityRole::Button)
                        .optional_label(Some(SharedString::from(label)))
                        .w_full()
                        .h_full()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .bg(Color::TRANSPARENT)
                        .cursor_pointer()
                        .rounded(px(0.0))
                        .on_press(on_press)
                        .child(
                            text(label)
                                .font_family(tokens::FONT_UI)
                                .text_size(px(tokens::TYPE_UI))
                                .line_height(px(tokens::TYPE_UI_LINE))
                                .text_color(move || {
                                    if danger {
                                        tokens::palette().danger
                                    } else {
                                        tokens::palette().ink
                                    }
                                }),
                        ),
                ),
        )
}

// ─── status line ──────────────────────────────────────────────────────────

/// How long a done message stays at resting ink before it fades.
const STATUS_HOLD_MS: u64 = 3_000;
/// The fade of a done message; reduced motion drops it at once.
const STATUS_FADE_MS: u64 = 240;

/// The quiet line at the foot of the column: what the app last did, at resting
/// ink. Local, so it never glows. Something done fades after a moment and
/// leaves the line empty; an error, a refusal or a wait
/// ([`AppState::notify_held`]) stays until the next message. The line keeps
/// its height either way, so nothing under or above it moves.
pub fn status_line(state: AppState, language: Language) -> impl IntoElement {
    let _ = language;
    let opacity = animated_value(1.0);
    let steps: Rc<RefCell<Vec<Spawned<()>>>> = Rc::default();
    let watched = state.clone();
    effect(move || {
        let message = watched.status.get();
        let held = watched.status_held.get_untracked();
        // A new message cancels the last one's fade and shows at full ink.
        steps.borrow_mut().clear();
        opacity.stop();
        opacity.set(1.0);
        if message.is_empty() || held {
            return;
        }
        let fade = set_timeout(Duration::from_millis(STATUS_HOLD_MS), move || {
            if reduced_motion() {
                opacity.set(0.0);
            } else {
                opacity.timing_with(0.0, Timing::new(STATUS_FADE_MS as f32, Easing::EaseInOut));
            }
        });
        // Faded, the message leaves the line, so nothing reads it any more —
        // unless another one took its place meanwhile.
        let clearing = watched.clone();
        let clear = set_timeout(
            Duration::from_millis(STATUS_HOLD_MS + STATUS_FADE_MS),
            move || {
                if clearing.status.get_untracked() == message {
                    clearing.status.set(SharedString::default());
                }
            },
        );
        steps.borrow_mut().extend([fade, clear]);
    });
    view().test_id("status").w_full().pt(px(SPACE_3)).child(
        view()
            .w_full()
            .h(px(tokens::TYPE_UI_LINE))
            .bind_opacity(&opacity)
            .child(
                text(move || state.status.get())
                    .font_family(tokens::FONT_UI)
                    .text_size(px(tokens::TYPE_UI))
                    .line_height(px(tokens::TYPE_UI_LINE))
                    .number_of_lines(1)
                    .text_color(move || tokens::palette().ink_rest),
            ),
    )
}

/// The hairline a banner uses to separate itself from the document.
pub fn banner_rule() -> impl IntoElement {
    view().w_full().pt(px(SPACE_4)).child(prims::hairline())
}

// ─── the comparison's keys ────────────────────────────────────────────────

/// J and K walk the differences while the comparison is on screen (and no text
/// input is being edited, so the editor keeps its own letters); Escape closes
/// it. The bindings die with the panel.
fn bind_compare_keys(state: &AppState) {
    for (key, delta) in [("j", 1isize), ("k", -1isize)] {
        let state = state.clone();
        on_key_with(
            key,
            KeyOptions {
                when: When::NotEditing,
                repeat: true,
                ..KeyOptions::default()
            },
            move |_| step_difference(&state, delta),
        );
    }
    let escape = state.clone();
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        move |_| close_compare(&escape),
    );
}

// ─── copy ─────────────────────────────────────────────────────────────────

/// The notices' copy. New strings live here rather than in `i18n.rs` (a merge
/// hotspot): the match is exhaustive, so a missing translation is a compile
/// error.
#[derive(Clone, Copy)]
enum S {
    UntitledDraft,
    Restore,
    Discard,
    ChangedOnDisk,
    Compare,
    KeepMine,
    UseDiskVersion,
    Previous,
    Next,
    CloseWithoutSaving,
    Save,
    DontSave,
    Cancel,
}

fn s(key: S, language: Language) -> &'static str {
    match language {
        Language::English => match key {
            S::UntitledDraft => "Untitled draft",
            S::Restore => "Restore",
            S::Discard => "Discard",
            S::ChangedOnDisk => "Changed on disk",
            S::Compare => "Compare",
            S::KeepMine => "Keep mine",
            S::UseDiskVersion => "Use disk version",
            S::Previous => "Previous",
            S::Next => "Next",
            S::CloseWithoutSaving => "Close without saving?",
            S::Save => "Save",
            S::DontSave => "Don't save",
            S::Cancel => "Cancel",
        },
        Language::Chinese => match key {
            S::UntitledDraft => "未命名草稿",
            S::Restore => "恢复",
            S::Discard => "丢弃",
            S::ChangedOnDisk => "磁盘上的文件已更改",
            S::Compare => "比较",
            S::KeepMine => "保留我的",
            S::UseDiskVersion => "使用磁盘版本",
            S::Previous => "上一个",
            S::Next => "下一个",
            S::CloseWithoutSaving => "不保存就关闭？",
            S::Save => "保存",
            S::DontSave => "不保存",
            S::Cancel => "取消",
        },
    }
}

/// 'Difference 2 of 5', one-based for the reader.
fn difference_text(index: usize, total: usize, language: Language) -> String {
    match language {
        Language::English => format!("Difference {index} of {total}"),
        Language::Chinese => format!("第 {index} 个差异，共 {total} 个"),
    }
}

/// '… 4 more lines' under a region the panel had to cut.
fn more_lines_text(hidden: usize, language: Language) -> String {
    match language {
        Language::English => format!("… {hidden} more lines"),
        Language::Chinese => format!("…… 另有 {hidden} 行"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn identical_files_have_no_differences() {
        assert!(differences("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn a_changed_line_is_one_difference() {
        let mine = "one\ntwo\nthree\nfour\nfive\nsix\n";
        let disk = "one\ntwo\nTHREE\nfour\nfive\nsix\n";
        let found = differences(mine, disk);
        assert_eq!(
            found,
            vec![Difference {
                mine: 2..3,
                disk: 2..3
            }]
        );
    }

    #[test]
    fn distant_changes_are_separate_differences() {
        let mine = "a\nb\nc\nd\ne\nf\ng\n";
        let disk = "a\nB\nc\nd\ne\nF\ng\n";
        let found = differences(mine, disk);
        assert_eq!(found.len(), 2);
        assert_eq!((found[0].mine.clone(), found[0].disk.clone()), (1..2, 1..2));
        assert_eq!((found[1].mine.clone(), found[1].disk.clone()), (5..6, 5..6));
    }

    #[test]
    fn an_inserted_line_is_one_difference_for_the_rest_of_the_file() {
        let mine = "a\nb\nc\n";
        let disk = "a\nnew\nb\nc\n";
        let found = differences(mine, disk);
        assert_eq!(
            found,
            vec![Difference {
                mine: 1..1,
                disk: 1..2
            }]
        );
    }

    #[test]
    fn a_wholly_rewritten_middle_is_one_difference() {
        let mine = "head\none\ntwo\nthree\ntail\n";
        let disk = "head\nONE\nTWO\nTHREE\ntail\n";
        let found = differences(mine, disk);
        assert_eq!(
            found,
            vec![Difference {
                mine: 1..4,
                disk: 1..4
            }]
        );
        assert_eq!(differing_lines(mine, disk), vec![1, 2, 3]);
    }

    #[test]
    fn only_the_words_the_buffer_lacks_are_marked() {
        let marked = word_marks("the quick brown fox", "the quick red fox");
        assert_eq!(
            marked,
            vec![
                ("the".to_owned(), false),
                (" ".to_owned(), false),
                ("quick".to_owned(), false),
                (" ".to_owned(), false),
                ("red".to_owned(), true),
                (" ".to_owned(), false),
                ("fox".to_owned(), false),
            ]
        );
    }

    #[test]
    fn an_unpaired_region_marks_every_word() {
        let marked = word_marks("", "one two");
        assert_eq!(
            marked,
            vec![
                ("one".to_owned(), true),
                (" ".to_owned(), false),
                ("two".to_owned(), true)
            ]
        );
    }

    #[test]
    fn marked_lines_split_at_newlines_and_keep_empty_rows() {
        let marked = vec![("one\n\ntwo".to_owned(), true)];
        let lines = marked_lines(&marked);
        assert_eq!(
            lines,
            vec![
                vec![("one".to_owned(), true)],
                vec![],
                vec![("two".to_owned(), true)],
            ]
        );
    }

    #[test]
    fn the_clock_is_hh_mm() {
        assert_eq!(clock_hhmm(0), "00:00");
        assert_eq!(clock_hhmm(14 * 3600 + 2 * 60), "14:02");
        assert_eq!(clock_hhmm(-60), "23:59");
        assert_eq!(clock_hhmm(48 * 3600 + 30 * 60), "00:30");
    }

    #[test]
    fn a_checkpoint_time_applies_the_platform_offset_once() {
        let at = UNIX_EPOCH + Duration::from_secs(14 * 3600 + 2 * 60);
        assert_eq!(
            checkpoint_time(at),
            clock_hhmm(14 * 3600 + 2 * 60 + local_offset_seconds())
        );
    }

    #[test]
    fn the_guard_names_one_two_or_more_documents() {
        let one = vec!["notes.md".to_owned()];
        let two = vec!["notes.md".to_owned(), "draft.md".to_owned()];
        let four = vec![
            "notes.md".to_owned(),
            "draft.md".to_owned(),
            "spec.md".to_owned(),
            "plan.md".to_owned(),
        ];
        assert_eq!(names_line(&[], Language::English), "");
        assert_eq!(names_line(&one, Language::English), "notes.md");
        assert_eq!(names_line(&two, Language::English), "notes.md, draft.md");
        assert_eq!(
            names_line(&four, Language::English),
            "notes.md, draft.md and 2 more"
        );
        assert_eq!(
            names_line(&four, Language::Chinese),
            "notes.md、draft.md 等 4 个文件"
        );
    }
}
