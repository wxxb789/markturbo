//! The conflict panel's text diff: how the buffer's text differs from the
//! file on disk, as line runs and as marked words.
//!
//! Pure text: no document, no state, no UI. The panel draws what this
//! returns; `ui::notices` owns the presentation (its 12-line cap, its
//! copy, its decoration styles).

use std::ops::Range;

/// One run of a region's text: its words, and whether the buffer does not
/// have them (the dashed mark). Whitespace never marks.
pub type Marked = (String, bool);

/// One run of lines that differ between the buffer and the file on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// The buffer's lines (0-based, half-open) in the difference.
    pub mine: Range<usize>,
    /// The disk's lines (0-based, half-open) in the difference.
    pub disk: Range<usize>,
}

/// Pairing work above this many cells is one difference instead of a
/// longest-common-subsequence walk (a large rewrite is not worth pairing).
const LINE_CELLS: usize = 1_000_000;
/// The same bound for word pairing.
const WORD_CELLS: usize = 40_000;

/// `text.split('\n')` without the trailing empty line a final newline makes
/// (it is the same on both sides, so it never carries a difference).
pub fn split_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Split marked runs at their newlines, so the panel can draw one text per
/// line. A blank line comes out with no runs; the panel gives it a space to
/// keep its row.
pub fn marked_lines(marked: &[Marked]) -> Vec<Vec<Marked>> {
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

/// The runs of lines that differ between `mine` and `disk`: the common head
/// and tail are cut off, the middle is paired by longest common subsequence
/// (so an insertion is one difference, not the rest of the file), and a
/// middle too large to pair is one difference in itself.
pub fn differences(mine: &str, disk: &str) -> Vec<Difference> {
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
        > LINE_CELLS
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
pub fn differing_lines(mine: &str, disk: &str) -> Vec<u32> {
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

/// Mark `disk`'s words against `mine`'s: a word the buffer does not have gets
/// the dashed mark. Whitespace never marks. A region too large to pair marks
/// every word it has.
pub fn word_marks(mine: &str, disk: &str) -> Vec<Marked> {
    let mine_words = words(mine);
    let disk_words = words(disk);
    let mut marked: Vec<Marked> = Vec::with_capacity(disk_words.len());
    let paired = mine_words
        .len()
        .saturating_mul(disk_words.len())
        .saturating_mul(4)
        <= WORD_CELLS;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
