# 05 — Measure `opt-level = "s"`

Type: experiment
Status: deferred — fresh builds ready; quiet-machine A/B remains in `docs/TODO.md`

## Question

Would size optimization reduce the binary without slowing startup or the
renderer hot path? The baseline is Cargo release's default `opt-level = 3`;
all other release settings stay fixed (`codegen-units = 1`, thin LTO, unwind).

## Builds

The same working tree was built into independent target directories with
`CARGO_INCREMENTAL=0`:

Fresh rebuild on August 25, 2026:

```text
opt-level=3  46,841,856 bytes  build 16m28s
opt-level=s  37,169,152 bytes  build  9m58s
```

`s` saved 9,672,704 bytes (20.65%). Both formula test executables were also
compiled under their matching profiles, so no further build is needed before a
quiet rerun.

## Quiet-machine gate

`scripts/probe.py quiet` now measures CPU with `GetSystemTimes` and disk busy
time with a low-overhead PDH counter. The thresholds were registered before the
rerun:

```text
CPU median <= 5%   CPU p95 <= 10%
disk median <= 2%  disk p95 <= 10%
```

The formal command waits for the first passing 60-second rolling window:

```text
uv run scripts/probe.py quiet --wait-seconds 3600
```

Nowledge's two backend processes were suspended only inside each gate/measurement
window. Resource Monitor and Task Manager were closed for the final attempts and
restored afterward. Defender, DLP, RDP, and Codex were never stopped.

Observed windows:

```text
after builds                 CPU 31.40 / 100.00%  disk 3.35 / 50.38%
after monitor shutdown       CPU  6.69 /  13.48%  disk 0.58 /  1.58%
best later 60-second window  CPU  6.12 /  10.74%  disk 0.70 /  2.83%
3600-second wait final       CPU 79.15 /  95.51%  disk 2.44 / 54.57%
```

The one-hour rolling gate never passed. The combined wrapper therefore exited
before launching either startup or formula A-B-B-A, as intended. No noisy sample
from this rerun is performance evidence.

## Earlier noisy diagnostic

The earlier failed-gate run remains useful only as a diagnostic. Ten startup
rounds, two observations per binary per round:

```text
opt-level=3 median 733.3 ms  p95 896.3 ms
opt-level=s median 609.8 ms  p95 773.2 ms
paired median B-A -121.1 ms  -15.98%
```

Every startup round favored `s`. The first-formula renderer probe, with the
repo's KaTeX fonts explicitly selected, was not stable in the same direction:

```text
opt-level=3 median 4,600 us
opt-level=s median 5,150 us
paired median B-A +675 us  +14.84%
```

The ten paired formula rounds ranged from -23.62% to +41.05%, crossing the 5%
decision boundary. `probe.py formula` now reproduces that A-B-B-A process order
directly from the two prebuilt test executables.

## Decision

Do not add `opt-level = "s"`. Size reduction is confirmed; runtime performance
is still inconclusive because the current enterprise host cannot satisfy the
pre-registered quiet gate. Re-run `quiet`, then `startup` and `formula`, on a
host or maintenance window that passes the gate before reconsidering it.
