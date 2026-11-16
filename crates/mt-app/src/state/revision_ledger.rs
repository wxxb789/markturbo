//! Where every hunk of a live Approved Revision sits in its tab's text.
//!
//! A decision writes its text at once: accepting a change replaces each of
//! its hunks' reviewed bytes with the replacement, and taking the decision
//! back (an undo, or a reject after an accept) writes the reviewed bytes back
//! — exactly those bytes, nothing composed from elsewhere. The ledger keeps
//! each hunk's byte range in the text as it is now, so a hunk further down
//! still lands on its own bytes after an earlier one changed length.
//!
//! The user may edit the text between decisions. The ledger compares the
//! text it last saw with the current one, takes the one span the edit
//! covers, and moves every hunk wholly before or after it. A hunk the edit
//! touched loses its place for good: it can no longer be accepted ("this part
//! changed") and, once accepted, no longer be taken back. Every write also
//! checks that the bytes at a hunk's place are the ones it expects, so a
//! refused write is the only failure — text is never written over bytes the
//! ledger does not recognise.

use std::ops::Range;

use mt_core::review::revision::{ChangeId, RevisionProposal};

/// Why a write was refused; the text is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerRefusal {
    /// The proposal has no such change.
    UnknownChange,
    /// The part of the text one of the change's hunks covers was edited.
    PartChanged,
}

/// The ledger of one proposal over one tab.
#[derive(Debug, Clone)]
pub(crate) struct RevisionLedger {
    tab_id: u64,
    /// The tab's text as the ledger last saw it; every range is in it.
    text: String,
    hunks: Vec<LedgerHunk>,
}

#[derive(Debug, Clone)]
struct LedgerHunk {
    change_id: ChangeId,
    before: String,
    after: String,
    applied: bool,
    /// Where the hunk's current side (`after` once applied, else `before`)
    /// sits in `text`; `None` once an edit touched it.
    range: Option<Range<usize>>,
}

impl LedgerHunk {
    fn current_side(&self) -> &str {
        if self.applied {
            &self.after
        } else {
            &self.before
        }
    }
}

impl RevisionLedger {
    /// A ledger for `proposal`, whose hunks are placed in its reviewed
    /// source, bound to the tab `tab_id`.
    pub(crate) fn new(tab_id: u64, proposal: &RevisionProposal) -> Self {
        let hunks = proposal
            .hunks()
            .iter()
            .map(|hunk| {
                let source = hunk.source();
                LedgerHunk {
                    change_id: hunk.change_id(),
                    before: hunk.expected_source().to_owned(),
                    after: hunk.replacement().to_owned(),
                    applied: false,
                    range: usize::try_from(source.start)
                        .ok()
                        .zip(usize::try_from(source.end).ok())
                        .map(|(start, end)| start..end),
                }
            })
            .collect();
        Self {
            tab_id,
            text: proposal.source().to_owned(),
            hunks,
        }
    }

    pub(crate) fn tab_id(&self) -> u64 {
        self.tab_id
    }

    /// Whether the change's text is in the buffer.
    pub(crate) fn applied(&self, change_id: ChangeId) -> bool {
        self.hunks
            .iter()
            .any(|hunk| hunk.change_id == change_id && hunk.applied)
    }

    /// Follow the text to `current`: hunks before or after the edited span
    /// keep (or move with) their bytes, a hunk inside it loses its place.
    pub(crate) fn observe(&mut self, current: &str) {
        if self.text == current {
            return;
        }
        let span = EditSpan::between(&self.text, current);
        for hunk in &mut self.hunks {
            hunk.range = hunk.range.take().and_then(|range| span.rebase(range));
        }
        current.clone_into(&mut self.text);
    }

    /// Whether the hunk at `index` no longer finds its current side intact
    /// in `current`: an edit touched it. Reads nothing but its arguments.
    pub(crate) fn part_changed(&self, index: usize, current: &str) -> bool {
        let Some(hunk) = self.hunks.get(index) else {
            return true;
        };
        let range = if self.text == current {
            hunk.range.clone()
        } else {
            let span = EditSpan::between(&self.text, current);
            hunk.range.clone().and_then(|range| span.rebase(range))
        };
        range.is_none_or(|range| current.get(range) != Some(hunk.current_side()))
    }

    /// Write the change as `applied` (its replacement) or not (its reviewed
    /// bytes) into `current`: the new text, or `None` when it already reads
    /// that way. Every hunk of the change is written or none is.
    pub(crate) fn set_applied(
        &mut self,
        change_id: ChangeId,
        applied: bool,
        current: &str,
    ) -> Result<Option<String>, LedgerRefusal> {
        if !self.hunks.iter().any(|hunk| hunk.change_id == change_id) {
            return Err(LedgerRefusal::UnknownChange);
        }
        self.observe(current);
        let flips = |hunk: &LedgerHunk| hunk.change_id == change_id && hunk.applied != applied;
        if !self.hunks.iter().any(flips) {
            return Ok(None);
        }
        // First pass: every flipping hunk must find its bytes where it left
        // them, and the placed ranges must be in order; nothing is mutated
        // until the whole write is known to be good.
        let mut output = String::with_capacity(self.text.len());
        let mut ranges = Vec::with_capacity(self.hunks.len());
        let mut cursor = 0;
        for hunk in &self.hunks {
            let Some(range) = hunk.range.clone() else {
                if flips(hunk) {
                    return Err(LedgerRefusal::PartChanged);
                }
                ranges.push(None);
                continue;
            };
            let (Some(gap), Some(bytes)) = (
                self.text.get(cursor..range.start),
                self.text.get(range.clone()),
            ) else {
                return Err(LedgerRefusal::PartChanged);
            };
            if range.start < cursor || bytes != hunk.current_side() {
                return Err(LedgerRefusal::PartChanged);
            }
            output.push_str(gap);
            let start = output.len();
            if flips(hunk) {
                output.push_str(if applied { &hunk.after } else { &hunk.before });
            } else {
                output.push_str(bytes);
            }
            ranges.push(Some(start..output.len()));
            cursor = range.end;
        }
        output.push_str(&self.text[cursor..]);
        for (hunk, range) in self.hunks.iter_mut().zip(ranges) {
            if hunk.change_id == change_id {
                hunk.applied = applied;
            }
            hunk.range = range;
        }
        self.text.clone_from(&output);
        Ok(Some(output))
    }
}

/// The one span an edit replaced: `old[start..old_end]` became
/// `new[start..new_end]`, the bytes around it common to both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EditSpan {
    start: usize,
    old_end: usize,
    new_end: usize,
}

impl EditSpan {
    /// The narrowest span explaining `old` → `new`: the longest common
    /// prefix, then the longest common suffix of what is left, both on
    /// character boundaries.
    fn between(old: &str, new: &str) -> Self {
        let mut start = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(start) {
            start -= 1;
        }
        let limit = old.len().min(new.len()) - start;
        let mut suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(limit)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix) {
            suffix -= 1;
        }
        Self {
            start,
            old_end: old.len() - suffix,
            new_end: new.len() - suffix,
        }
    }

    /// Where `range` (in the old text) is in the new one: unchanged wholly
    /// before the span, moved wholly after it, `None` when the span touches
    /// it.
    fn rebase(self, range: Range<usize>) -> Option<Range<usize>> {
        if range.end <= self.start {
            Some(range)
        } else if range.start >= self.old_end {
            let moved = |at: usize| at - self.old_end + self.new_end;
            Some(moved(range.start)..moved(range.end))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_core::review::{
        ByteRange, SourceSnapshot,
        revision::{RevisionChange, RevisionEdit, RevisionLimits},
    };

    const SOURCE: &str = "alpha beta gamma\n";

    /// Three changes over `SOURCE`: one shrinks, one grows, one inserts.
    fn ledger() -> RevisionLedger {
        let edit = |start, end, before: &str, after: &str| {
            RevisionEdit::new(ByteRange::new(start, end).unwrap(), before, after)
        };
        let proposal = RevisionProposal::validate(
            SOURCE,
            SourceSnapshot::new(0, 0),
            vec![
                RevisionChange::new("Shrink.", vec![edit(0, 5, "alpha", "a")]),
                RevisionChange::new("Grow.", vec![edit(6, 10, "beta", "BETA-BETA")]),
                RevisionChange::new("Insert.", vec![edit(16, 16, "", "!")]),
            ],
            RevisionLimits::default(),
        )
        .expect("a valid proposal");
        RevisionLedger::new(7, &proposal)
    }

    fn apply(ledger: &mut RevisionLedger, change: u32, applied: bool, text: &str) -> String {
        ledger
            .set_applied(ChangeId(change), applied, text)
            .expect("the write is accepted")
            .expect("the text changes")
    }

    #[test]
    fn decisions_write_at_once_and_later_hunks_follow_length_changes() {
        let mut ledger = ledger();
        assert_eq!(ledger.tab_id(), 7);
        let one = apply(&mut ledger, 1, true, SOURCE);
        assert_eq!(one, "alpha BETA-BETA gamma\n", "only that hunk");
        let two = apply(&mut ledger, 0, true, &one);
        assert_eq!(two, "a BETA-BETA gamma\n");
        let three = apply(&mut ledger, 2, true, &two);
        assert_eq!(three, "a BETA-BETA gamma!\n", "the insertion moved twice");
        assert!(ledger.applied(ChangeId(2)));
        // Taken back out of order, each restores exactly its own bytes.
        let back = apply(&mut ledger, 0, false, &three);
        assert_eq!(back, "alpha BETA-BETA gamma!\n");
        let back = apply(&mut ledger, 1, false, &back);
        assert_eq!(back, "alpha beta gamma!\n");
        let back = apply(&mut ledger, 2, false, &back);
        assert_eq!(back, SOURCE, "byte for byte the reviewed text");
        assert_eq!(
            ledger.set_applied(ChangeId(2), false, SOURCE),
            Ok(None),
            "a write that changes nothing says so"
        );
        assert_eq!(
            ledger.set_applied(ChangeId(9), true, SOURCE),
            Err(LedgerRefusal::UnknownChange)
        );
    }

    #[test]
    fn an_edit_elsewhere_moves_hunks_and_an_edit_inside_one_refuses_it() {
        let mut ledger = ledger();
        let accepted = apply(&mut ledger, 0, true, SOURCE);
        assert_eq!(accepted, "a beta gamma\n");

        // The user types a comma after the accepted word: the hunks after
        // the edit move with it.
        let edited = "a, beta gamma\n";
        ledger.observe(edited);
        assert!(!ledger.part_changed(1, edited));
        let grown = apply(&mut ledger, 1, true, edited);
        assert_eq!(grown, "a, BETA-BETA gamma\n");

        // An edit inside the accepted replacement: that hunk can no longer be
        // taken back, and the refusal leaves the text alone.
        let touched = "A, BETA-BETA gamma\n";
        assert!(ledger.part_changed(0, touched), "seen without observing");
        assert_eq!(
            ledger.set_applied(ChangeId(0), false, touched),
            Err(LedgerRefusal::PartChanged)
        );
        // The other hunks still are where they were.
        let back = apply(&mut ledger, 1, false, touched);
        assert_eq!(back, "A, beta gamma\n");
        let inserted = apply(&mut ledger, 2, true, &back);
        assert_eq!(inserted, "A, beta gamma!\n");
    }

    #[test]
    fn a_changed_base_refuses_the_hunk() {
        let ledger = ledger();
        // The reviewed `beta` was edited before any decision.
        let edited = "alpha bet gamma\n";
        assert!(ledger.part_changed(1, edited));
        assert!(!ledger.part_changed(0, edited));
        assert!(
            !ledger.part_changed(2, edited),
            "an insertion has no base, and its place is untouched"
        );
        let mut ledger = ledger;
        assert_eq!(
            ledger.set_applied(ChangeId(1), true, edited),
            Err(LedgerRefusal::PartChanged)
        );
        assert!(!ledger.applied(ChangeId(1)));
        // The text the ledger does not recognise is never written over, even
        // where the edit happens to leave the same length.
        let replaced = "alpha BETA gamma\n";
        assert!(ledger.part_changed(1, replaced));
    }

    #[test]
    fn a_multi_byte_edit_keeps_character_boundaries() {
        let span = EditSpan::between("é", "è");
        assert_eq!(span.start, 0, "never inside a character");
        assert_eq!((span.old_end, span.new_end), (2, 2));
        let span = EditSpan::between("ab", "aab");
        assert_eq!(span.rebase(0..1), Some(0..1));
        assert_eq!(span.rebase(1..2), Some(2..3));
    }
}
