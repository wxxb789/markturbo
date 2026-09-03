# MarkTurbo tooling

Use one entry point for all repeatable development commands:

```sh
uv run --project scripts scripts/mt.py <command>
```

`scripts/pyproject.toml` declares the small Python environment used by the
tooling, and `scripts/uv.lock` pins its complete resolved dependency graph.

## Validation

```sh
uv run --project scripts scripts/mt.py check fast
uv run --project scripts scripts/mt.py check ci
uv run --project scripts scripts/mt.py check full
```

- `fast`: whitespace validation and every explicit non-desktop tooling test.
- `ci`: `fast`, Rust formatting, locked Clippy, and locked release workspace
  tests.
- `full`: `ci` and a locked production build of `markturbo`; it never launches
  the desktop app.

`fast` checks both unstaged and staged whitespace changes locally. In CI, a
complete `BASE_SHA`/`HEAD_SHA` pair checks that revision range instead and does
not depend on the index. The same range can be supplied explicitly:

```sh
uv run --project scripts scripts/mt.py check fast --base <base-sha> --head <head-sha>
```

The test list lives in `markturbo_tools/checks.py`. It is deliberately explicit:
native UI acceptance cannot enter a test run through discovery. The runtime
probe is Windows-only, but its geometry unit test and the other tooling tests
validate portable input, source contracts, or fixture behavior without a desktop.

## Commands

```sh
uv run --project scripts scripts/mt.py icons
uv run --project scripts scripts/mt.py fixtures
uv run --project scripts scripts/mt.py probe -- memory
uv run --project scripts scripts/mt.py capacity
uv run --project scripts scripts/mt.py accept goal-02 -- --help
uv run --project scripts scripts/mt.py accept goal-03 -- --help
```

`icons` regenerates the platform icon outputs. `fixtures` deterministically
regenerates committed performance fixtures. `probe` measures a real Windows
process. `capacity` measures the ignored Windows DPAPI capacity test in fresh
Cargo processes. The two `accept` commands drive real Windows UI workflows and
write fail-closed, hash-bound evidence.

`probe formula` measures the embedded KaTeX path by default. Pass `--font-dir`
only when intentionally measuring a complete external development override.

Goal 04 startup evidence uses app-acknowledged, content-free QPC milestones.
It requires the same active, unlocked Windows 11 x64 desktop as native
acceptance because the harness sends `F24` through `SendInput` and records the
GPUI action acknowledgement. Build each variant through the controlled command;
it requires an empty target directory and writes source, lockfile, feature,
release-profile, Cargo/Rustc versions, hashed Cargo configuration, dependency
graph, executable hash, size, and PE-section provenance. Compared manifests
must use the same recorded toolchain. The `full` and `no-model` manifests also include current
`cargo-bloat 0.12.1` crate attribution and selected dependency features. The
ablation runs the exact no-default-features unavailable-diagnostic test, proves
`genai`/`reqwest`/`rustls` leave the target, and reports Tokio's remaining
non-model features rather than falsely claiming Tokio disappears:

```sh
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant full --target-dir .scratch/goal-04/full \
  --evidence .scratch/goal-04/full-build.json
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant no-model --target-dir .scratch/goal-04/no-model \
  --evidence .scratch/goal-04/no-model-build.json
```

The `no-model` build is measurement apparatus, not a supported product
configuration: provider-backed selection, block, and document Translation all
return the explicit Goal 04 unavailable diagnostic. Before any full/no-model
result is visible, the owner-approved numeric materiality rule must be stored in
the source-bound `markturbo-goal-04-threshold-v1` artifact. Its cache rule must
require both the warm and fresh-profile runs to meet the same threshold before
extraction can be authorized. Generate a passing quiet-gate record immediately
before each comparison:

```sh
uv run --project scripts scripts/mt.py probe -- quiet \
  --wait-seconds 3600 \
  --evidence .scratch/goal-04/quiet.json
uv run --project scripts scripts/mt.py probe -- startup \
  --milestones \
  --exe .scratch/goal-04/full/x86_64-pc-windows-msvc/release/markturbo.exe \
  --compare .scratch/goal-04/no-model/x86_64-pc-windows-msvc/release/markturbo.exe \
  --label full \
  --compare-label no-model \
  --build-evidence .scratch/goal-04/full-build.json \
  --compare-build-evidence .scratch/goal-04/no-model-build.json \
  --threshold-evidence .scratch/goal-04/threshold.json \
  --cache-state warm \
  --rounds 10 \
  --quiet-evidence .scratch/goal-04/quiet.json \
  --evidence .scratch/goal-04/full-vs-no-model-warm.json
```

Every milestone sample also records idle working set, private bytes, peak
working set, page faults, and thread count after `--idle-settle`. The default
`warm` mode reuses one isolated data/config profile per variant across warmups
and measured launches. Repeat the command after a new quiet gate with
`--cache-state fresh-profile` and write
`.scratch/goal-04/full-vs-no-model-fresh-profile.json`. This creates fresh
isolated profiles but explicitly does not claim to flush the Windows file cache.
After both runs, bind the owner's final decision to both evidence files:

```sh
uv run --project scripts scripts/mt.py probe -- decide-goal04 \
  --warm-evidence .scratch/goal-04/full-vs-no-model-warm.json \
  --fresh-profile-evidence .scratch/goal-04/full-vs-no-model-fresh-profile.json \
  --decision "keep in-process" \
  --owner-approved \
  --evidence .scratch/goal-04/model-transport-decision.json
```

Build and measure the model first-use test against a deterministic loopback
endpoint, bound to the matching full application build. Every reported sample
starts a fresh process and performs cold transport initialization; declared
warmups affect only the Windows file cache, which is not flushed:

```sh
uv run --project scripts scripts/mt.py probe -- build-goal04 \
  --variant model-first-use --target-dir .scratch/goal-04/model-first-use \
  --evidence .scratch/goal-04/model-first-use-build.json
uv run --project scripts scripts/mt.py probe -- model-first-use \
  --exe .scratch/goal-04/model-first-use/goal04-artifacts/model-first-use.exe \
  --build-evidence .scratch/goal-04/model-first-use-build.json \
  --app-exe .scratch/goal-04/full/x86_64-pc-windows-msvc/release/markturbo.exe \
  --app-build-evidence .scratch/goal-04/full-build.json \
  --quiet-evidence .scratch/goal-04/quiet.json \
  --rounds 10 \
  --evidence .scratch/goal-04/model-first-use.json
```

Forward an underlying script's options after `--`. For example:

```sh
uv run --project scripts scripts/mt.py probe -- startup --rounds 10
uv run --project scripts scripts/mt.py accept goal-03 -- \
  --exe target/release/markturbo.exe \
  --expect-exe-sha256 <sha256> \
  --evidence .scratch/goal-03-native-acceptance-v1.json
```

Delegated commands run from the repository root. Therefore paths supplied to
`--exe`, `--evidence`, and `--open` are repository-relative, just as they are
when the underlying module is invoked directly.

## Native evidence

Native acceptance requires Windows 11 x64, an active unlocked interactive
desktop, `pywinauto`, and a current `target/release/markturbo.exe`. Build first:

```sh
cargo build --release --locked -p mt-app --bin markturbo
sha256sum target/release/markturbo.exe
```

Use the measured hash in the `accept` command. `PASS` is possible only when all
required cases complete against that hash. `BLOCKED`, including inaccessible
foreground/input-desktop access, is not acceptance and must be rerun in an
eligible interactive session. The harnesses preserve user data isolation and
record only content-free timings, hashes, byte counts, status observations, and
OS/session/integrity metadata needed to validate the run.

The native command exit status is part of that contract: `0` is `PASS`, `1` is
`FAIL`, and `2` is `BLOCKED`. Any unexpected child-process exit is reported as
`FAIL` by the CLI.

## Scratch data

`.scratch/` is disposable by default. Keep measured performance evidence under
`.scratch/perf-and-size/` when it is intended to be versioned.
