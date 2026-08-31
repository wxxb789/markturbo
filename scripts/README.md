# scripts/

The harness. Everything here exists to measure or package this application, and
nothing here is compiled into it.

| Script | What it does |
|---|---|
| `probe.py` | Measures markturbo startup, memory, idle CPU, child windows, and hit testing |
| `goal-02-native-acceptance.py` | Exercises Goal 02's five destructive lifecycle workflows through a real Windows UI and writes hash-bound JSON evidence |
| `test_goal_02_native_acceptance.py` | Exercises the native harness's schema and fail-closed behavior without launching a UI |
| `recovery-capacity.py` | Measures the ignored near-capacity Windows DPAPI checkpoint test in fresh cargo processes |
| `test_recovery_capacity.py` | Exercises recovery-capacity.py parsing without Cargo or DPAPI |
| `gen-perf-fixtures.py` | Regenerates the committed fixtures under `fixtures/perf/` |
| `test_release_automation.py` | Locks the `cargo-release` configuration and GitHub Actions release contract |
| `generate-app-icons.py` | Derives the Windows, macOS, and Linux icons from the 1024 px master PNG |
| `test_generate_app_icons.py` | Exercises icon command discovery and transactional publication failures |
| `test_platform_packaging.py` | Exercises Linux installation and macOS bundle version mapping |
| `install-linux.sh` | Installs an extracted Linux release into the user's XDG data home |
| `package-release.sh` | Builds and stages a distributable archive under `dist/` |

## The rules

**Python for anything that runs more than once**, invoked through `uv` as a
single-file script with inline dependencies:

```python
#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow>=10"]
# ///
```

Run it as `uv run scripts/<name>.py`. There is no virtualenv to create and no
requirements file to keep in sync; `uv` reads the header and does it.

`generate-app-icons.py` has no Python dependencies. On Windows it requires
ImageMagick's `magick`, because `convert.exe` is a filesystem utility; on other
platforms it accepts `magick` or `convert`. It validates every generated
artifact before publishing, and restores the prior icon set if publication
fails partway.

For the WebView single-window acceptance and paired startup comparison:

```bash
uv run scripts/probe.py windows --open page.html \
  --expect-top-level 1 --expect-child-class WRY_WEBVIEW \
  --expect-native-chrome-insets
uv run scripts/probe.py quiet
uv run scripts/probe.py startup --exe target/release-a/markturbo.exe \
  --compare target/release-b/markturbo.exe --rounds 10
uv run scripts/probe.py formula --exe target/release-a/open_document_cost.exe \
  --compare target/release-b/open_document_cost.exe --rounds 10
```

The comparison launches in A-B-B-A order within every round and prints raw
launches plus paired `B-A` deltas in milliseconds and percent. Run it only on a
quiet machine; interleaving reduces drift but cannot remove competing load.

`quiet` takes 60 one-second samples using `GetSystemTimes` and a low-overhead
PDH disk counter. Its default gate is CPU median <=5%, CPU p95 <=10%, disk
median <=2%, and disk p95 <=10%. Run it immediately before a small A/B decision;
do not weaken the thresholds after seeing the result. `--wait-seconds 3600`
waits for the first passing 60-second rolling window instead of accepting a
noisy sample. `formula` runs the ignored first-formula test in fresh processes
and uses the same A-B-B-A order as `startup`.

For the recovery capacity evidence required by Goal 02 on Windows, run:

```bash
uv run scripts/recovery-capacity.py
```

It launches three fresh `cargo test` processes by default. Each process runs the
ignored near-capacity DPAPI test's three internal rounds. The harness rejects
missing or inconsistent Rust duration, ciphertext, median, or maximum output,
then prints every round and a nine-sample aggregate summary. It fails when the
global maximum exceeds the 8-second post-dispatch checkpoint budget. Use
`uv run scripts/test_recovery_capacity.py` to exercise only that parser; it
does not invoke Cargo or DPAPI.

## Goal 02 Native Destructive Acceptance

`goal-02-native-acceptance.py` is the Goal 02 Windows 11 x64 acceptance gate.
It is deliberately separate from `probe.py`: `probe.py windows` verifies a
clean application's window and WebView lifecycle, not dirty-text safety.

Prerequisites: a current x64 `target/release/markturbo.exe`, an **active,
unlocked** Windows 11 desktop session, foreground/input-desktop access, and
`uv`. The script declares `pywinauto==0.6.9`; `uv` resolves it and its
`comtypes` dependency. Do not run it from a disconnected WTS session.

The executable hash is a required evidence binding. Build the executable, measure
its current SHA-256, then pass that measured value to the native harness:

```bash
cargo build --release --locked -p mt-app --bin markturbo
sha256sum target/release/markturbo.exe
uv run scripts/goal-02-native-acceptance.py \
  --exe target/release/markturbo.exe \
  --expect-exe-sha256 <measured-sha256> \
  --evidence .scratch/goal-02-native-acceptance-v1.json
```

The evidence file is `.scratch/goal-02-native-acceptance-v1.json`. A new build
requires a new measured SHA-256 argument; never reuse an old hash or evidence
file as proof for a different executable.

The harness may use an absolute `MARKTURBO_DATA_DIR` for isolated WebView and
log paths. Production recovery storage is separately local-only: UNC,
mapped/remote, and unsupported-volume roots are rejected before creation, and
an unavailable recovery store does not prevent editing or source-file Save.

The five required workflows are:

- Pointer tab close -> Cancel -> `Ctrl+S`.
- `Ctrl+W` -> Discard.
- `WM_CLOSE` -> Save with exact source bytes.
- Same-length, same-mtime external change -> detected conflict -> explicit Overwrite.
- CJK/emoji edit -> checkpoint -> `TerminateProcess` -> restart -> Discard -> restart without recovery.

Exit status is evidence-bearing: `0` means `PASS` only when all five workflows
pass; `1` means `FAIL`; `2` means `BLOCKED`, including an unavailable active
desktop or UI dependency. `BLOCKED` is not a passing result and must be rerun in
an eligible session.

The harness is privacy-preserving and fail-closed. It records hashes, byte
counts, status codes, and content-free logs rather than document text; it scans
runtime artifacts for the test sentinel and rejects leaked text, missing
preconditions, stale hash bindings, incomplete case sets, and ambiguous UI
locators. An incomplete or failed evidence file is never acceptance evidence.

The single-window count ignores only Windows' hidden zero-sized `IME` and
`MSCTFIME UI` helpers. Hidden product windows and every non-zero top-level HWND
still fail acceptance.

`windows` counts every process-owned top-level window, including hidden and
zero-sized windows. With `--expect-top-level 1`, the titled main window must be
the only one. Each expected child must be visible, non-zero, and contained by
the main client rectangle. The probe then alternates and restores the main size
twice, requires expected child bounds to react, posts `WM_CLOSE`, and waits up
to five seconds for a clean exit. It launches with an isolated
copy of the executable and `MARKTURBO_DATA_DIR`, requires the per-process
application log to contain its startup record, requires WebView2 to populate
`webview2/` under that isolated root, rejects `<exe>.WebView2` beside the copied
binary, and scans both the application log and stderr for
`RefCell already borrowed`. Without `--log`, stderr goes to a temporary file
that is removed after the check. Use `--forbid-log-substring` for additional
fatal text in either stream.

`--expect-native-chrome-insets` is the Web-mode chrome contract: each expected
child must leave both a positive top inset for native title/tab/document chrome
and a positive bottom inset for native status chrome. Without this explicit
flag, the generic child check continues to allow a child that fills the client.

The harness deliberately does not synthesize `Ctrl+,` to toggle Settings.
`SendInput` depends on foreground permission and keyboard layout, so it is not a
stable headless acceptance mechanism. This command covers WebView `Bounds` and
`Shutdown`; `Hide`/`Show` needs an in-process test or an interactive probe.

**No PowerShell** in a committed script. PowerShell 5 and 7 differ in ways that
bite exactly here — assembly loading, `Add-Type` reference resolution, quoting
when invoked from a shell that is not PowerShell — and a script that works on
one is not evidence it works on the other. `probe.py` was six `.ps1` files
first, and one of them silently captured the wrong window and nearly became
evidence for a fix. An ad-hoc `pwsh -Command` while investigating is fine; a
committed harness is not the place.

Where a script needs Win32, reach it with `ctypes` rather than through a shell.
`probe.py` does process counters, window enumeration and hit testing that way,
and it is portable across every Python on the machine.

**Bash only where the task is genuinely a build pipeline.**
`package-release.sh` stays bash because it is `cargo build`, `cp`, `zip` — and
because a release script should be readable by anyone who has ever packaged
anything. It runs `set -euo pipefail` and `cd`s to the repository root, so it
behaves the same from any working directory.

## What a harness script owes you

- **Measure, do not estimate.** Every number this repository's documents cite
  came from one of these, run on a real process. If a script cannot measure
  something honestly, it should say so and exit rather than print a plausible
  number — `probe.py shot` refuses when another window covers markturbo,
  because a screen capture would otherwise show someone else's content.
- **Be re-runnable.** `gen-perf-fixtures.py` fixes its seed so a regeneration is
  a no-op in `git status`. A harness that produces a different answer each run
  cannot be used to decide anything.
- **Say what it needs.** A missing binary, a stale build, an obstructed window:
  name it and exit non-zero.

## Machine notes

Measurements taken here so far are all `x86_64-pc-windows-msvc` on a machine
with **no GPU** — WARP software rasterization. Memory that would live in VRAM on
real hardware counts as process private bytes, so figures from this machine are
an upper bound rather than a typical one. `probe.py` is Windows-only for the
same reason its subject is: the child-window and hit-testing questions it
answers are Win32 questions.

The machine is also noisy. A concurrent release build has been observed to
change the same measurement by 2x, which is why `probe.py cpu` takes a series
rather than one sample — a single reading lands on startup transients and cannot
tell "busy while starting" from "never converges".
