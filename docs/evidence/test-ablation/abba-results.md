# A3 A-B-B-A arithmetic ablation results

Date: 2026-09-29

## Decision

Refactor. `metrics.measure_abba` and `goal04.milestone_comparison` now call the
pure `metrics.summarize_abba_samples` helper for adjacent pair indexing,
B-minus-A deltas, and baseline-relative percentages. The helper accepts the
pair reducer because the callers' existing floating-point overflow behavior is
not equivalent. The scheduler remains in `measure_abba_samples`; Goal 04 retains
its matching-even-length validation and JSON field/list shapes; `measure_abba`
retains its zero-baseline `ValueError` translation.

## Differential experiment

The original detector was the arithmetic in each caller plus the four existing
focused tests recorded in `abba-baseline-tests.txt`. The surviving detectors are
the same scheduler, scalar-statistics, zero-baseline, and forged-comparison tests,
plus the structured adjacent-pair test added for cross-pair and pair-median faults.

`abba-original.json` and `abba-refactored.json` serialize these inputs:

- fixed two-pair numeric values where individual pair means differ from the raw values;
- a nonzero negative numeric baseline;
- zero numeric A and zero rounds;
- fixed three-pair `StartupSample` values whose paired median delta differs from
  subtracting the raw variant medians;
- unequal, odd, and zero-A structured sample vectors.

Both files are 3,686 bytes and have SHA-256
`e0f76c67b0a0640e50225c917b5aa4a879e693a2e5624b57b6872597a7cf2f8a`.
The byte comparison exited 0 (`abba-differential.txt`). This includes identical
exception types and messages: matching-even-length `ValueError`, startup zero-A
`ZeroDivisionError`, numeric zero-A translated `ValueError`, and invalid-round
`ValueError`.

A material pair-reducer counterexample was retained rather than normalized:
for `(1e308, 1e308)`, the original scalar `statistics.fmean` raises
`OverflowError: intermediate overflow in fsum`, while Goal 04's original
`sum(pair) / 2` returns `inf`. `abba-contract-counterexample.txt` shows the
refactored callers preserve both outcomes, and a behavior test now pins them.

Baseline focused tests: 4 passed, exit 0. Refactored focused tests: 6 passed,
exit 0. Their complete outputs are in `abba-baseline-tests.txt` and
`abba-refactored-tests.txt`.

## Discriminating fault controls

Each production mutation was applied alone, its named surviving test was run,
and the source was restored before the next mutation. All controls were killed:

| Injected fault | Surviving detector | Exit |
| --- | --- | ---: |
| Change collection order from A-B-B-A to A-B-A-B | scheduler order/separation test | 1 |
| Cross A pair indexes instead of adjacent indexes | structured adjacent-pair test | 1 |
| Reverse B-minus-A to A-minus-B | scalar paired-statistics test | 1 |
| Permit a zero A baseline with a fabricated 0% | zero-baseline exception test | 1 |
| Subtract raw variant medians instead of taking the paired-delta median | structured pair-median test | 1 |

`abba-fault-controls.json` contains each test ID, exit code, output hash, and full
failure output. `abba-fault-controls.txt` records the aggregate control exit 0.

## Final verification

- `python -m unittest -v scripts.tests.test_probe`: 55 passed, exit 0; complete
  output is in `abba-portable-tests.txt`.
- `python -m py_compile` on both changed modules, the changed test module, and
  both experiment scripts: exit 0.
- `git diff --check` on the three changed tracked files: exit 0.

No timing or performance claim is made.
