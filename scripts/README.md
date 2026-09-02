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
