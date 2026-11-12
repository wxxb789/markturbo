# App policy test ablation

## Baseline and temporary instrumentation

Original source SHA-256 values:

- `crates/mt-app/src/i18n.rs`: `c9693664146f69e53516210a5989ba6c76af8a7281c569203bc2f2919ea2204c`.
- `crates/mt-app/src/views/document.rs`: `b128894b6a5a7e6fc90cabab1f34400f5f069a5eeab671e2d6b1f38b7bbaa6cb`.
- `crates/mt-app/src/views/mod.rs`: `e02d6007e4e56fc2bfedc03b03bd93cf8c484b1f5d14b63b92270b76da9500bd`.

The experiment build temporarily compiles three independent test-only fault switches.
Each test process will activate exactly one through `MARKTURBO_ABLATION_FAULT`:
an empty English Welcome label, a Text default of Native, or the document-level
layout wrapper incorrectly offering Native for HTML. No production build or
final verification may use these switches; restore all three original hashes
before permanently removing any test.

The wrapper fault is a deliberate negative control: the broad document policy
test may protect a different entry point than the lower-level Layout tests.
Retain it if ablation loses that detection.

## Measured no-fault baseline

`cargo test --locked --profile ci -p mt-app --lib --no-run` exited 0.
With no fault enabled, the resulting test executable exited 0: 328 tests
passed and the existing one ignored test remained ignored. The instrumentation
has not changed the healthy baseline.

## Paired fault results

Each run used the entire App library test executable and exactly one fault.
The provisional cut excluded the three inventory candidates.

| Fault | Full selection | Provisional cut | Decision |
| --- | --- | --- | --- |
| Empty Welcome label | exit 101; 326 passed, 2 failed, 1 ignored | exit 101; 324 passed, 1 failed, 1 ignored, 3 filtered | Complete-catalog test retains detection; remove the Welcome subset test. |
| Text defaults to Native | exit 101; 324 passed, 4 failed, 1 ignored | exit 101; 323 passed, 2 failed, 1 ignored, 3 filtered | Plain-text and offered-default tests retain detection; remove the single Text-default assertion. |
| Document wrapper offers Native for HTML | exit 101; 327 passed, 1 failed, 1 ignored | exit 0; 325 passed, 1 ignored, 3 filtered | Veto removal of the combined document policy test: the proposed survivors miss this adapter fault. |

Rechecking the final two-test cut while retaining the combined policy test
killed the wrapper fault: exit 101, 325 passed, 1 failed, 1 ignored, 2 filtered.
Raw runs are `app-welcome-empty.json`, `app-text-native.json`,
`app-wrapper-native.json`, and `app-wrapper-retained.json`.

## Restoration and retained change

All three original source hashes were restored before deletion. The patch tool
had reformatted an unrelated `vec!` expression; the repository's canonical
`cargo fmt --all` restored the original bytes, verified by the hashes above.
Only the Welcome subset test and single Text-default test are removed. The
combined policy test remains because the experiment demonstrated unique
detection. Final `check ci` exited 0; the App library passed 326 tests with its
one original ignored test still ignored. The workspace total was 1,080 passed
and 11 ignored. The final inventory contains exactly the two planned App unit
test removals and retains the combined policy detector.
