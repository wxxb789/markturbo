# 04 — Incremental global skill discovery

Type: task
Status: resolved

## Question

The Harness panel rescanned every global skill root after any relevant watcher
event, folder change, or scope refresh. A root-directory mtime alone is not a
valid cache key because editing `SKILL.md` does not change the root directory.

## Resolution

`DiscoveryCache` stores one unfiltered snapshot per global root. Workspace
roots remain uncached. A reusable root fingerprint contains:

- the canonical root path;
- every visited directory's mtime;
- every discovered `SKILL.md` / `skill.md` mtime and size;
- an explicit missing-root state.

The cache preserves canonical identity and all aliases, reuses the same
snapshot when `include_internal` changes, keeps snapshots while global discovery
is disabled, and invalidates only a dirty or reintroduced root. Manual Rescan
clears every snapshot.

The workspace watcher now refreshes Harness only for paths that discovery can
actually observe: configured skill roots, recognized instruction files, and the
`rules`, `instructions`, or `memories` directories searched by instruction
discovery. Ordinary source-file atomic saves and unrelated tree changes still
refresh the file explorer, but no longer restart Harness discovery or its 250ms
loading state. Removed or renamed-out directories are matched against the last
successful Harness result, so dotted directory names remain correct after the
filesystem path itself has disappeared.

Any traversal, metadata, entry read, or support-directory probe error makes the
root uncacheable, so a transient partial scan is retried rather than persisted.
When a rescan fails after a successful scan, discovery keeps serving the last
successful snapshot until a later rescan succeeds.

## Measured

Command:

```text
MARKTURBO_BENCH_DIR=Q:/repos/markturbo \
cargo test --release -p mt-app --test open_folder_cost \
  attribute_the_cost_of_opening_a_folder -- --ignored --nocapture
```

Observed on August 25, 2026, 257 total skills:

```text
cold  268 ms  250 ms  278 ms   median 268 ms
warm   67 ms   64 ms   62 ms   75 ms   63 ms   median 64 ms
```

The warm median is 76.12% below the cold median. The ordinary tests assert scan
counts and result equivalence rather than wall time.

## Guarded by

Thirty-five `skill::tests`, including entry edits with unchanged root mtime,
directory add/rename/delete, per-root invalidation, missing roots appearing,
manual clear, internal filtering, root eviction/restoration, I/O failures, and
cached/uncached alias equivalence.

Workspace coverage adds `only_harness_discovery_paths_trigger_a_rescan` and
`unrelated_tree_changes_do_not_rescan_harness`, including `.github/workflows`,
`.claude/settings.json`, ordinary source files, and names that merely contain
`skill` or `rules`.
