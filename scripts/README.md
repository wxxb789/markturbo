# scripts/

The harness. Everything here exists to measure or package this application, and
nothing here is compiled into it.

| Script | What it does |
|---|---|
| `probe.py` | Measures a running markturbo: memory, idle CPU, child windows, hit testing |
| `gen-perf-fixtures.py` | Regenerates the committed fixtures under `fixtures/perf/` |
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
