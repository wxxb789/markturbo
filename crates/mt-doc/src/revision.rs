//! Locally validated, user-approved revisions.
//!
//! Provider edits are only input data.  This module validates their expected
//! source bytes, assigns local change identities, computes immutable hunks,
//! and composes a preview from explicitly approved changes.

use std::{collections::HashMap, fmt};

use crate::review::{ByteRange, SourceSnapshot};

/// A local identity for one inseparable provider change.
///
/// Provider-supplied identifiers are deliberately not retained as the UI
/// identity.  IDs are allocated in validated input order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeId(pub u32);

/// Bounds applied while validating one revision proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionLimits {
    pub max_source_bytes: usize,
    pub max_output_bytes: usize,
    pub max_replacement_bytes: usize,
    pub max_rationale_bytes: usize,
    pub max_changes: usize,
    pub max_hunks: usize,
}

impl Default for RevisionLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 4 * 1024 * 1024,
            max_output_bytes: 4 * 1024 * 1024,
            max_replacement_bytes: 4 * 1024 * 1024,
            max_rationale_bytes: 16 * 1024,
            max_changes: 128,
            max_hunks: 512,
        }
    }
}

/// One provider-supplied edit.  Its range is untrusted until a proposal is
/// validated against the exact reviewed source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionEdit {
    range: ByteRange,
    expected_source: String,
    replacement: String,
}

impl RevisionEdit {
    pub fn new(
        range: ByteRange,
        expected_source: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        Self {
            range,
            expected_source: expected_source.into(),
            replacement: replacement.into(),
        }
    }

    pub const fn range(&self) -> ByteRange {
        self.range
    }

    pub fn expected_source(&self) -> &str {
        &self.expected_source
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

/// A rationale-bearing group of edits that must be accepted or rejected as a
/// unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionChange {
    rationale: String,
    edits: Vec<RevisionEdit>,
}

impl RevisionChange {
    pub fn new(rationale: impl Into<String>, edits: Vec<RevisionEdit>) -> Self {
        Self {
            rationale: rationale.into(),
            edits,
        }
    }

    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    pub fn edits(&self) -> &[RevisionEdit] {
        &self.edits
    }
}

/// A locally computed hunk.  Its source range and change ID are owned by this
/// module rather than copied from provider metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionHunk {
    change_id: ChangeId,
    source: ByteRange,
    expected_source: String,
    replacement: String,
    rationale: String,
}

impl RevisionHunk {
    pub fn change_id(&self) -> ChangeId {
        self.change_id
    }

    pub const fn source(&self) -> ByteRange {
        self.source
    }

    pub fn expected_source(&self) -> &str {
        &self.expected_source
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }

    pub fn rationale(&self) -> &str {
        &self.rationale
    }
}

/// A fully validated revision proposal bound to one reviewed source snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionProposal {
    snapshot: SourceSnapshot,
    source: String,
    hunks: Vec<RevisionHunk>,
    change_ids: Vec<ChangeId>,
    limits: RevisionLimits,
}

impl RevisionProposal {
    /// Validate provider edits and compute local, source-ordered hunks.
    pub fn validate(
        source: &str,
        snapshot: SourceSnapshot,
        changes: Vec<RevisionChange>,
        limits: RevisionLimits,
    ) -> Result<Self, RevisionError> {
        if source.len() > limits.max_source_bytes {
            return Err(RevisionError::SourceTooLarge {
                byte_size: source.len(),
                limit: limits.max_source_bytes,
            });
        }
        if changes.len() > limits.max_changes {
            return Err(RevisionError::TooManyChanges {
                count: changes.len(),
                limit: limits.max_changes,
            });
        }

        let source_len = source.len() as u64;
        let mut change_ids = Vec::with_capacity(changes.len());
        let mut candidates = Vec::new();
        let mut change_totals = Vec::with_capacity(changes.len());
        let mut hunk_count = 0_usize;

        for (index, change) in changes.into_iter().enumerate() {
            let value = u32::try_from(index).map_err(|_| RevisionError::ChangeIdOverflow)?;
            let change_id = ChangeId(value);
            change_ids.push(change_id);

            let RevisionChange { rationale, edits } = change;

            if rationale.trim().is_empty() {
                return Err(RevisionError::EmptyRationale { change_id });
            }
            if rationale.len() > limits.max_rationale_bytes {
                return Err(RevisionError::RationaleTooLarge {
                    byte_size: rationale.len(),
                    limit: limits.max_rationale_bytes,
                    change_id,
                });
            }
            if edits.is_empty() {
                return Err(RevisionError::EmptyChange { change_id });
            }
            change_totals.push((0_usize, 0_usize));

            hunk_count =
                hunk_count
                    .checked_add(edits.len())
                    .ok_or(RevisionError::TooManyHunks {
                        count: usize::MAX,
                        limit: limits.max_hunks,
                    })?;
            if hunk_count > limits.max_hunks {
                return Err(RevisionError::TooManyHunks {
                    count: hunk_count,
                    limit: limits.max_hunks,
                });
            }

            for edit in edits {
                let range = edit.range;
                if range.start > range.end {
                    return Err(RevisionError::InvalidRange {
                        start: range.start,
                        end: range.end,
                    });
                }
                if range.end > source_len {
                    return Err(RevisionError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len,
                    });
                }

                let start = usize::try_from(range.start).map_err(|_| {
                    RevisionError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len,
                    }
                })?;
                let end =
                    usize::try_from(range.end).map_err(|_| RevisionError::RangeOutsideSource {
                        start: range.start,
                        end: range.end,
                        source_len,
                    })?;
                if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
                    return Err(RevisionError::RangeNotUtf8Boundary {
                        start: range.start,
                        end: range.end,
                    });
                }

                let actual = &source[start..end];
                if actual != edit.expected_source {
                    return Err(RevisionError::ExpectedSourceMismatch {
                        start: range.start,
                        end: range.end,
                    });
                }
                if actual == edit.replacement {
                    return Err(RevisionError::NoOpEdit {
                        change_id,
                        start: range.start,
                        end: range.end,
                    });
                }
                if edit.replacement.len() > limits.max_replacement_bytes {
                    return Err(RevisionError::ReplacementTooLarge {
                        byte_size: edit.replacement.len(),
                        limit: limits.max_replacement_bytes,
                        change_id,
                    });
                }

                let totals = &mut change_totals[index];
                totals.0 = totals
                    .0
                    .checked_add(range.len() as usize)
                    .ok_or(RevisionError::OutputSizeOverflow)?;
                totals.1 = totals
                    .1
                    .checked_add(edit.replacement.len())
                    .ok_or(RevisionError::OutputSizeOverflow)?;

                candidates.push(RevisionHunk {
                    change_id,
                    source: range,
                    expected_source: edit.expected_source,
                    replacement: edit.replacement,
                    rationale: rationale.clone(),
                });
            }
        }

        candidates.sort_by(|left, right| {
            left.source
                .start
                .cmp(&right.source.start)
                .then_with(|| left.source.end.cmp(&right.source.end))
                .then_with(|| left.change_id.cmp(&right.change_id))
        });

        for pair in candidates.windows(2) {
            let previous = &pair[0];
            let current = &pair[1];
            if current.source.start < previous.source.end
                || current.source.start == previous.source.start
            {
                return Err(RevisionError::OverlappingEdits {
                    previous_start: previous.source.start,
                    previous_end: previous.source.end,
                    start: current.source.start,
                    end: current.source.end,
                });
            }
        }

        let output_size = checked_output_size(source.len(), &candidates)?;
        let maximum_subset_size = checked_maximum_subset_size(source.len(), &change_totals)?;
        if output_size > limits.max_output_bytes {
            return Err(RevisionError::OutputTooLarge {
                byte_size: output_size,
                limit: limits.max_output_bytes,
            });
        }
        if maximum_subset_size > limits.max_output_bytes {
            return Err(RevisionError::SubsetOutputTooLarge {
                byte_size: maximum_subset_size,
                limit: limits.max_output_bytes,
            });
        }

        Ok(Self {
            snapshot,
            source: source.to_owned(),
            hunks: candidates,
            change_ids,
            limits,
        })
    }

    pub fn snapshot(&self) -> SourceSnapshot {
        self.snapshot
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn hunks(&self) -> &[RevisionHunk] {
        &self.hunks
    }

    pub fn compose(&self, decisions: &[(ChangeId, bool)]) -> Result<String, RevisionError> {
        let decisions = self.decision_map(decisions)?;
        let mut output = String::with_capacity(self.source.len());
        let mut cursor = 0_usize;

        for hunk in &self.hunks {
            let start = hunk.source.start as usize;
            let end = hunk.source.end as usize;
            output.push_str(&self.source[cursor..start]);
            if decisions.get(&hunk.change_id).copied().unwrap_or(false) {
                output.push_str(&hunk.replacement);
            } else {
                output.push_str(&self.source[start..end]);
            }
            cursor = end;
        }
        output.push_str(&self.source[cursor..]);

        if output.len() > self.limits.max_output_bytes {
            return Err(RevisionError::OutputTooLarge {
                byte_size: output.len(),
                limit: self.limits.max_output_bytes,
            });
        }
        Ok(output)
    }

    pub fn compose_against(
        &self,
        current_text: &str,
        current_snapshot: SourceSnapshot,
        decisions: &[(ChangeId, bool)],
    ) -> Result<String, RevisionError> {
        if !self.snapshot.matches(current_snapshot) {
            return Err(RevisionError::StaleSnapshot {
                expected: self.snapshot,
                actual: current_snapshot,
            });
        }
        if current_text != self.source {
            return Err(RevisionError::SourceMismatch);
        }
        self.compose(decisions)
    }

    pub fn accept_all(&self) -> String {
        let decisions: Vec<_> = self
            .change_ids
            .iter()
            .copied()
            .map(|change_id| (change_id, true))
            .collect();
        self.compose(&decisions)
            .expect("validated proposal must accept all local changes")
    }

    pub fn reject_all(&self) -> String {
        self.source.clone()
    }

    fn decision_map(
        &self,
        decisions: &[(ChangeId, bool)],
    ) -> Result<HashMap<ChangeId, bool>, RevisionError> {
        if decisions.len() > self.change_ids.len() || decisions.len() > self.limits.max_changes {
            return Err(RevisionError::TooManyDecisions {
                count: decisions.len(),
                limit: self.change_ids.len().min(self.limits.max_changes),
            });
        }
        let mut result = HashMap::with_capacity(decisions.len());
        for &(change_id, accepted) in decisions {
            if !self.change_ids.contains(&change_id) {
                return Err(RevisionError::UnknownChangeId(change_id));
            }
            if result.insert(change_id, accepted).is_some() {
                return Err(RevisionError::DuplicateDecision(change_id));
            }
        }
        Ok(result)
    }
}

fn checked_output_size(source_len: usize, hunks: &[RevisionHunk]) -> Result<usize, RevisionError> {
    let mut output_size = source_len;
    for hunk in hunks {
        output_size = output_size
            .checked_sub(hunk.source.len() as usize)
            .and_then(|value| value.checked_add(hunk.replacement.len()))
            .ok_or(RevisionError::OutputSizeOverflow)?;
    }
    Ok(output_size)
}

fn checked_maximum_subset_size(
    source_len: usize,
    change_totals: &[(usize, usize)],
) -> Result<usize, RevisionError> {
    let mut maximum = source_len;
    for &(removed, replacement) in change_totals {
        if replacement > removed {
            maximum = maximum
                .checked_add(replacement - removed)
                .ok_or(RevisionError::OutputSizeOverflow)?;
        }
    }
    Ok(maximum)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevisionError {
    SourceTooLarge {
        byte_size: usize,
        limit: usize,
    },
    TooManyChanges {
        count: usize,
        limit: usize,
    },
    TooManyHunks {
        count: usize,
        limit: usize,
    },
    EmptyRationale {
        change_id: ChangeId,
    },
    RationaleTooLarge {
        byte_size: usize,
        limit: usize,
        change_id: ChangeId,
    },
    EmptyChange {
        change_id: ChangeId,
    },
    NoOpEdit {
        change_id: ChangeId,
        start: u64,
        end: u64,
    },
    ChangeIdOverflow,
    InvalidRange {
        start: u64,
        end: u64,
    },
    RangeOutsideSource {
        start: u64,
        end: u64,
        source_len: u64,
    },
    RangeNotUtf8Boundary {
        start: u64,
        end: u64,
    },
    ExpectedSourceMismatch {
        start: u64,
        end: u64,
    },
    ReplacementTooLarge {
        byte_size: usize,
        limit: usize,
        change_id: ChangeId,
    },
    OverlappingEdits {
        previous_start: u64,
        previous_end: u64,
        start: u64,
        end: u64,
    },
    OutputSizeOverflow,
    OutputTooLarge {
        byte_size: usize,
        limit: usize,
    },
    SubsetOutputTooLarge {
        byte_size: usize,
        limit: usize,
    },
    TooManyDecisions {
        count: usize,
        limit: usize,
    },
    UnknownChangeId(ChangeId),
    DuplicateDecision(ChangeId),
    StaleSnapshot {
        expected: SourceSnapshot,
        actual: SourceSnapshot,
    },
    SourceMismatch,
}

impl fmt::Display for RevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLarge { byte_size, limit } => {
                write!(formatter, "source is {byte_size} bytes; maximum is {limit}")
            }
            Self::TooManyChanges { count, limit } => {
                write!(
                    formatter,
                    "proposal has {count} changes; maximum is {limit}"
                )
            }
            Self::TooManyHunks { count, limit } => {
                write!(formatter, "proposal has {count} hunks; maximum is {limit}")
            }
            Self::EmptyRationale { change_id } => {
                write!(formatter, "change {change_id:?} has an empty rationale")
            }
            Self::RationaleTooLarge {
                byte_size,
                limit,
                change_id,
            } => write!(
                formatter,
                "rationale for {change_id:?} is {byte_size} bytes; maximum is {limit}"
            ),
            Self::EmptyChange { change_id } => {
                write!(formatter, "change {change_id:?} has no edits")
            }
            Self::NoOpEdit {
                change_id,
                start,
                end,
            } => write!(
                formatter,
                "change {change_id:?} contains a no-op edit at {start}..{end}"
            ),
            Self::ChangeIdOverflow => formatter.write_str("change ID space is exhausted"),
            Self::InvalidRange { start, end } => {
                write!(formatter, "invalid source range {start}..{end}")
            }
            Self::RangeOutsideSource {
                start,
                end,
                source_len,
            } => write!(
                formatter,
                "source range {start}..{end} exceeds source length {source_len}"
            ),
            Self::RangeNotUtf8Boundary { start, end } => {
                write!(
                    formatter,
                    "source range {start}..{end} is not on UTF-8 boundaries"
                )
            }
            Self::ExpectedSourceMismatch { start, end } => write!(
                formatter,
                "expected source does not match reviewed bytes at {start}..{end}"
            ),
            Self::ReplacementTooLarge {
                byte_size,
                limit,
                change_id,
            } => write!(
                formatter,
                "replacement for {change_id:?} is {byte_size} bytes; maximum is {limit}"
            ),
            Self::OverlappingEdits {
                previous_start,
                previous_end,
                start,
                end,
            } => write!(
                formatter,
                "source edits {previous_start}..{previous_end} and {start}..{end} overlap"
            ),
            Self::OutputSizeOverflow => formatter.write_str("composed output size overflowed"),
            Self::OutputTooLarge { byte_size, limit } => write!(
                formatter,
                "composed output is {byte_size} bytes; maximum is {limit}"
            ),
            Self::SubsetOutputTooLarge { byte_size, limit } => write!(
                formatter,
                "a change subset could produce {byte_size} bytes; maximum is {limit}"
            ),
            Self::TooManyDecisions { count, limit } => write!(
                formatter,
                "decision list has {count} entries; maximum is {limit}"
            ),
            Self::UnknownChangeId(change_id) => {
                write!(formatter, "unknown local change ID {change_id:?}")
            }
            Self::DuplicateDecision(change_id) => {
                write!(formatter, "change {change_id:?} has duplicate decisions")
            }
            Self::StaleSnapshot { expected, actual } => write!(
                formatter,
                "proposal snapshot {expected:?} does not match current snapshot {actual:?}"
            ),
            Self::SourceMismatch => {
                formatter.write_str("current source differs from reviewed source")
            }
        }
    }
}

impl std::error::Error for RevisionError {}

#[cfg(test)]
mod tests {
    use super::super::{ByteRange, SourceSnapshot};
    use super::{ChangeId, RevisionChange, RevisionEdit, RevisionLimits, RevisionProposal};

    fn limits() -> RevisionLimits {
        RevisionLimits::default()
    }

    fn edit(source: &str, needle: &str, replacement: &str) -> RevisionEdit {
        let start = source
            .find(needle)
            .unwrap_or_else(|| panic!("fixture is missing {needle:?}")) as u64;
        RevisionEdit::new(
            ByteRange::new(start, start + needle.len() as u64).unwrap(),
            needle,
            replacement,
        )
    }

    fn change(rationale: &str, edits: Vec<RevisionEdit>) -> RevisionChange {
        RevisionChange::new(rationale, edits)
    }

    fn proposal(source: &str, changes: Vec<RevisionChange>) -> RevisionProposal {
        RevisionProposal::validate(source, SourceSnapshot::new(7, 3), changes, limits())
            .expect("valid revision fixture")
    }

    fn bounded_limits(max_output_bytes: usize) -> RevisionLimits {
        RevisionLimits {
            max_source_bytes: 4 * 1024,
            max_output_bytes,
            max_replacement_bytes: 4 * 1024,
            max_rationale_bytes: 1024,
            max_changes: 16,
            max_hunks: 32,
        }
    }

    fn apply_edits(source: &str, edits: &[(ByteRange, &str)]) -> String {
        let mut output = String::new();
        let mut cursor = 0_usize;
        for (range, replacement) in edits {
            let start = range.start as usize;
            let end = range.end as usize;
            output.push_str(&source[cursor..start]);
            output.push_str(replacement);
            cursor = end;
        }
        output.push_str(&source[cursor..]);
        output
    }

    #[test]
    fn reject_all_is_byte_identical_to_the_reviewed_source() {
        let source = "before\n中文 🚀\nafter\n";
        let proposal = proposal(
            source,
            vec![change(
                "replace the heading",
                vec![edit(source, "before", "updated")],
            )],
        );

        assert_eq!(proposal.reject_all(), source);
    }

    #[test]
    fn accepting_one_of_multiple_changes_applies_exactly_that_change() {
        let source = "first\nsecond\nthird\n";
        let proposal = proposal(
            source,
            vec![
                change(
                    "revise the first line",
                    vec![edit(source, "first", "FIRST")],
                ),
                change(
                    "revise the second line",
                    vec![edit(source, "second", "SECOND")],
                ),
                change(
                    "revise the third line",
                    vec![edit(source, "third", "THIRD")],
                ),
            ],
        );

        let composed = proposal
            .compose(&[
                (ChangeId(0), false),
                (ChangeId(1), true),
                (ChangeId(2), false),
            ])
            .expect("known local change IDs compose");
        assert_eq!(composed, "first\nSECOND\nthird\n");
    }

    #[test]
    fn accept_all_is_exactly_the_displayed_final_preview() {
        let source = "alpha\nbeta\ngamma\n";
        let proposed = "ALPHA\nbeta\nGAMMA\n";
        let proposal = proposal(
            source,
            vec![
                change("update alpha", vec![edit(source, "alpha", "ALPHA")]),
                change("update gamma", vec![edit(source, "gamma", "GAMMA")]),
            ],
        );

        assert_eq!(proposal.accept_all(), proposed);
        assert_eq!(proposal.reject_all(), source);
    }

    #[test]
    fn one_change_can_group_inseparable_hunks_without_importing_other_change_bytes() {
        let source = "title\nbody\nfooter\n";
        let proposal = proposal(
            source,
            vec![
                change(
                    "the title and body must stay consistent",
                    vec![edit(source, "title", "TITLE"), edit(source, "body", "BODY")],
                ),
                change("revise the footer", vec![edit(source, "footer", "FOOTER")]),
            ],
        );

        let grouped = proposal
            .compose(&[(ChangeId(0), true), (ChangeId(1), false)])
            .expect("group decision composes both inseparable hunks");
        assert_eq!(grouped, "TITLE\nBODY\nfooter\n");
        assert!(!grouped.contains("FOOTER"));
    }

    #[test]
    fn source_and_snapshot_are_rechecked_before_composition() {
        let source = "stable source\n";
        let proposal = proposal(
            source,
            vec![change(
                "change the source",
                vec![edit(source, "stable", "changed")],
            )],
        );

        assert!(
            proposal
                .compose_against(
                    "source changed before Apply\n",
                    SourceSnapshot::new(7, 3),
                    &[(ChangeId(0), true)],
                )
                .is_err()
        );
        assert!(
            proposal
                .compose_against(source, SourceSnapshot::new(8, 3), &[(ChangeId(0), true)],)
                .is_err()
        );
    }

    #[test]
    fn overlapping_edits_reject_the_complete_proposal() {
        let source = "abcdef";
        let changes = vec![
            change(
                "first edit",
                vec![RevisionEdit::new(ByteRange::new(1, 4).unwrap(), "bcd", "X")],
            ),
            change(
                "overlapping edit",
                vec![RevisionEdit::new(ByteRange::new(3, 5).unwrap(), "de", "Y")],
            ),
        ];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits())
                .is_err()
        );
    }

    #[test]
    fn out_of_range_edits_reject_the_complete_proposal() {
        let source = "abcdef";
        let changes = vec![
            change("valid edit", vec![edit(source, "ab", "AB")]),
            change(
                "outside the source",
                vec![RevisionEdit::new(ByteRange::new(5, 8).unwrap(), "f", "F")],
            ),
        ];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits())
                .is_err()
        );
    }

    #[test]
    fn non_utf8_boundary_edits_reject_the_complete_proposal() {
        let source = "a中文b";
        let changes = vec![
            change("valid edit", vec![edit(source, "a", "A")]),
            change(
                "split a UTF-8 scalar",
                vec![RevisionEdit::new(ByteRange::new(2, 3).unwrap(), "中", "X")],
            ),
        ];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits())
                .is_err()
        );
    }

    #[test]
    fn expected_source_mismatch_rejects_all_changes_before_local_diff() {
        let source = "authoritative bytes\n";
        let start = source.find("authoritative").unwrap() as u64;
        let changes = vec![change(
            "provider supplied an untrusted quote",
            vec![RevisionEdit::new(
                ByteRange::new(start, start + "authoritative".len() as u64).unwrap(),
                "provider guessed this text",
                "replacement",
            )],
        )];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits())
                .is_err()
        );
    }

    #[test]
    fn one_oversized_change_rejects_the_entire_mixed_proposal() {
        let source = "keep\nreplace\n";
        let oversized = "x".repeat(8 * 1024 * 1024);
        let changes = vec![
            change("small valid edit", vec![edit(source, "keep", "KEEP")]),
            change(
                "unbounded output",
                vec![edit(source, "replace", &oversized)],
            ),
        ];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits())
                .is_err()
        );
    }

    #[test]
    fn validation_rejects_a_subset_that_can_exceed_the_output_bound() {
        let source = "a".repeat(100);
        let growth = "x".repeat(50);
        let changes = vec![
            change(
                "grow the first byte",
                vec![RevisionEdit::new(
                    ByteRange::new(0, 1).unwrap(),
                    "a",
                    growth,
                )],
            ),
            change(
                "remove the remaining bytes",
                vec![RevisionEdit::new(
                    ByteRange::new(1, 100).unwrap(),
                    &source[1..],
                    "",
                )],
            ),
        ];

        assert!(
            RevisionProposal::validate(
                &source,
                SourceSnapshot::default(),
                changes,
                bounded_limits(100),
            )
            .is_err()
        );
    }

    #[test]
    fn an_empty_proposal_is_valid_when_reject_all_fits_the_output_bound() {
        let source = "unchanged source";
        let proposal = RevisionProposal::validate(
            source,
            SourceSnapshot::default(),
            Vec::new(),
            bounded_limits(source.len()),
        )
        .expect("no safe change is a valid empty proposal");

        assert_eq!(proposal.reject_all(), source);
        assert_eq!(proposal.accept_all(), source);
        assert_eq!(proposal.compose(&[]).unwrap(), source);
    }

    #[test]
    fn an_empty_proposal_is_rejected_when_reject_all_would_exceed_the_bound() {
        let source = "four";
        assert!(
            RevisionProposal::validate(
                source,
                SourceSnapshot::default(),
                Vec::new(),
                bounded_limits(3),
            )
            .is_err()
        );
    }

    #[test]
    fn no_op_edits_are_malformed_even_when_the_source_is_valid() {
        let source = "same source";
        let changes = vec![change(
            "a provider no-op",
            vec![edit(source, "same", "same")],
        )];

        assert!(
            RevisionProposal::validate(source, SourceSnapshot::default(), changes, limits(),)
                .is_err()
        );
    }

    #[test]
    fn insertion_edits_at_valid_boundaries_preserve_exact_bytes() {
        let source = "中🚀文";
        let changes = vec![
            change(
                "insert at the beginning",
                vec![RevisionEdit::new(ByteRange::empty(0), "", "A")],
            ),
            change(
                "insert between scalars",
                vec![RevisionEdit::new(
                    ByteRange::empty("中".len() as u64),
                    "",
                    "B",
                )],
            ),
            change(
                "insert at the end",
                vec![RevisionEdit::new(
                    ByteRange::empty(source.len() as u64),
                    "",
                    "C",
                )],
            ),
        ];
        let proposal = proposal(source, changes);

        assert_eq!(proposal.accept_all(), "A中B🚀文C");
        assert_eq!(proposal.reject_all(), source);
    }

    #[test]
    fn omitted_decisions_reject_and_duplicate_or_unknown_ids_fail_closed() {
        let source = "ab";
        let proposal = proposal(
            source,
            vec![
                change("uppercase a", vec![edit(source, "a", "A")]),
                change("uppercase b", vec![edit(source, "b", "B")]),
            ],
        );

        assert_eq!(proposal.compose(&[]).unwrap(), source);
        assert!(
            proposal
                .compose(&[(ChangeId(0), true), (ChangeId(0), false)])
                .is_err()
        );
        assert!(proposal.compose(&[(ChangeId(99), true)]).is_err());
    }

    #[test]
    fn an_oversized_decision_list_is_rejected_before_decision_storage() {
        let source = "a";
        let proposal = proposal(
            source,
            vec![change("uppercase a", vec![edit(source, "a", "A")])],
        );
        let decisions = vec![(ChangeId(0), true); 2];

        assert!(proposal.compose(&decisions).is_err());
    }

    #[test]
    fn unicode_crlf_frontmatter_fence_and_link_offsets_remain_exact() {
        let source = "---\r\ntitle: 原文\r\n---\r\n# 标题 🚀\r\n\r\n[链接](https://example.com)\r\n\r\n```rust\r\nlet value = 1;\r\n```\r\n";
        let changes = vec![
            change(
                "translate frontmatter title",
                vec![edit(source, "原文", "更新")],
            ),
            change("revise heading", vec![edit(source, "标题 🚀", "标题 🧪")]),
            change("rename link text", vec![edit(source, "链接", "新链接")]),
            change(
                "update fenced source",
                vec![edit(source, "let value = 1;", "let value = 2;")],
            ),
        ];
        let expected = "---\r\ntitle: 更新\r\n---\r\n# 标题 🧪\r\n\r\n[新链接](https://example.com)\r\n\r\n```rust\r\nlet value = 2;\r\n```\r\n";
        let proposal = proposal(source, changes);

        assert_eq!(proposal.accept_all(), expected);
        assert_eq!(proposal.reject_all(), source);
    }

    #[test]
    fn deterministic_randomized_valid_unicode_edits_are_stable_and_complete() {
        for case in 0..64_u64 {
            let mut seed = 0x9e37_79b9_7f4a_7c15_u64 ^ case;
            let source: String = (0..32).map(|_| next_scalar(&mut seed)).collect();
            let boundaries: Vec<usize> = source
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(source.len()))
                .collect();
            let positions = [3_usize, 12, 22];
            let mut changes = Vec::with_capacity(positions.len());
            let mut replacements = Vec::with_capacity(positions.len());
            for position in positions {
                let start = boundaries[position];
                let end = boundaries[position + 1];
                let expected = &source[start..end];
                let mut replacement: String = (0..(1 + (next_u64(&mut seed) % 3)))
                    .map(|_| next_scalar(&mut seed))
                    .collect();
                if replacement == expected {
                    replacement.push(next_scalar(&mut seed));
                }
                replacements.push((
                    ByteRange::new(start as u64, end as u64).unwrap(),
                    replacement.clone(),
                ));
                changes.push(change(
                    "deterministic unicode edit",
                    vec![RevisionEdit::new(
                        ByteRange::new(start as u64, end as u64).unwrap(),
                        expected,
                        replacement,
                    )],
                ));
            }

            let expected: Vec<(ByteRange, &str)> = replacements
                .iter()
                .map(|(range, replacement)| (*range, replacement.as_str()))
                .collect();
            let proposed = apply_edits(&source, &expected);
            let first = proposal(&source, changes.clone());
            let second = proposal(&source, changes);
            let decisions: Vec<(ChangeId, bool)> =
                (0..3).map(|index| (ChangeId(index), true)).collect();

            assert_eq!(first.accept_all(), proposed);
            assert_eq!(first.reject_all(), source);
            assert_eq!(first.compose(&decisions).unwrap(), proposed);
            assert_eq!(first.hunks().len(), second.hunks().len());
            assert_eq!(second.accept_all(), proposed);
        }
    }

    fn next_u64(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *seed
    }

    fn next_scalar(seed: &mut u64) -> char {
        const POOL: &[char] = &['a', 'b', '你', '好', '界', '🚀', '🧪', 'e', '\u{301}'];
        POOL[(next_u64(seed) as usize) % POOL.len()]
    }
}
