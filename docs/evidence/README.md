# Evidence index

Versioned acceptance, ablation, and measurement evidence for the goals in
[`docs/`](../). Files keep the names used by the goal completion records so the
cited paths remain unambiguous.

| Path | Content |
|---|---|
| `goal-02-*` | Goal 02 acceptance, symlink, validation, and ablation receipts |
| `goal-03-native-acceptance-v1.json` | Goal 03 native acceptance result |
| `goal-04/final-v5/quiet.json` | Goal 04 diagnostic quiet measurement |
| `goal-04/final-v6/*.json` | Goal 04 release build/size receipts |
| `goal-05a-settings-privacy.png` | Goal 05a settings privacy visual evidence |
| `goal-06/`, `goal-06-native-*.json` | Goal 06 evaluation and native acceptance evidence |
| `goal-07/`, `goal-07-*.json` | Goal 07 accepted review, receipt, and ablation evidence |
| `goal-09/final4/` | Goal 09 release-build visual matrix (132 cells) |
| `perf-and-size/` | Versioned performance and size issue notes |
| `test-ablation/` | Test-ablation inventories, result tables, and experiment scripts |
| `ablation-*-inventory.md` | Per-layer test inventories |

Native acceptance receipts are content-free; visual and owner-review artifacts
are named explicitly. Do not add credentials, user document text, or local
absolute paths to new evidence.

Goal 02/03/06/07 acceptance runs write working receipts to the ignored
`target/native-acceptance/` directory by default, not to this historical archive.
Review new receipts before copying them here under distinct names and updating
their citations. An explicit `--evidence` path replaces its destination, even
when acceptance fails; do not point routine or debug runs at retained records.
