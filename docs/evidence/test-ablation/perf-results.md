# Performance fixture test ablation

## Baseline

- Existing `test_perf_fixtures`: 3 passed, exit 0.
- Generated and committed bytes match for all three benchmark inputs:
  - `large-10k.md`: `5b5085b99d842f4b386eaf5bd4295a30a46007f87464d6c57ed6325dc37b5b19`.
  - `huge-100k.md`: `c9d25eda879d842ab49404a8ce76a847eb2da58880e8865ad76c5ec7d87cd0f5`.
  - `diagram-heavy.md`: `9330acfdef50a21b7d4952f5497c47be50a3259e53ead6ac52ebbbc756761290`.
- The original test module is preserved as `perf-baseline.py` beside this file.

## Candidate

Replace comparing `make(40)` with itself with exact generated-versus-shipped
byte equality for the three actual benchmark inputs. Keep unique diagram and
cross-platform LF publication tests. The replacement protects the workload
against drift; it does not pin natural-language prose independently of the
shipped benchmark data.

No generator or committed fixture is changed.

## Executed comparison

`uv run --locked --project scripts python .scratch/test-ablation/perf-experiment.py`
exited 0. Eight faults were applied individually in memory: numbering, Unicode,
fence kind, truncation, line count, newline conversion, diagram IDs, and default
diagram count. The original self-comparison passed all eight faulty generators.
The replacement produced assertion failures for all eight, with no test errors;
each patch was removed before the next comparison.

`uv run --locked --project scripts python -m unittest scripts.tests.test_perf_fixtures`
then exited 0 with all 3 tests passing. Decision: keep the replacement. It removes
a weak repeated-call check while adding detection of shipped workload drift,
without changing any benchmark data or claiming a runtime performance gain.
