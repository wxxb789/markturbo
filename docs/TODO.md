# TODO

## Deferred

- [ ] **静默 A/B** — Re-run the `opt-level = 3` versus `opt-level = "s"`
  startup and first-formula comparisons after
  `uv run scripts/probe.py quiet --wait-seconds 3600` passes. Keep the current
  decision not to adopt `s` until the quiet-machine measurements are complete.
  See [ticket 05](../.scratch/perf-and-size/issues/05-opt-level-s.md).
