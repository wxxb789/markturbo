# mt-core test ablation inventory

Date: 2026-09-29

## Scope and evidence status

This is a read-only static inventory of every first-party Rust file under
`crates/mt-core/src` and `crates/mt-core/tests` that contains a test, an ignored
diagnostic/acceptance test, or test-only support code. I read
`docs/development.md`, `crates/mt-core/Cargo.toml`, the integration tests, the
test names and relevant source boundaries. I did **not** build or run anything,
so all ablation conclusions below are candidate evidence until the mutation
experiment is performed.

`[lib] doctest = false` disables library doctests. The source has documentation
comments, but the only triple-backtick occurrences are prose references or Rust
string literals; there is no separately runnable doc-test surface. Both
evaluation binaries are included below. Goal 06 needs `model-transport`, so the
complete ordinary selection should use `--all-features`.

- **Retain**: owns a distinct fault boundary, data shape, platform boundary, or integration seam.
- **Retain/support**: test-only injection code makes neighboring race tests deterministic.
- **Candidate**: ablate only after the stated mutation experiment demonstrates unchanged detection.

## Complete file inventory

| File | Owned behavior / proof boundary | Disposition |
|---|---|---|
| `src/agent_artifacts/harness.rs` | Harness registry uniqueness, root derivation, env/XDG fallback, shared-root labeling and separator-safe matching. | Retain: pure registry contract. |
| `src/agent_artifacts/instruction.rs` | Instruction roots/names/types, nested discovery, noise exclusion, dedup/aliasing, labels and hermetic default. | Retain: distinct from Skill discovery. |
| `src/agent_artifacts/package.rs` | Frozen Skill inventory, authenticated identity/revalidation, dirty/non-UTF-8 entrypoints, limits, symlink refusal, anchors and offset transforms. | Retain: security/TOCTOU boundary. |
| `src/agent_artifacts/package/source.rs` | Test-only root-validation hook and bounded supporting-file reader used by package race/limit tests. | Retain/support. |
| `src/agent_artifacts/skill.rs` | Skill spec validation plus global discovery cache correctness, invalidation, retry and alias semantics. | Retain. |
| `src/bin/markturbo-goal06-evaluate.rs` | Fixed 12-artifact corpus/config, manifest/path safety, send acknowledgement, argument/credential restrictions and inspection hashes. | Retain: evaluator CLI policy. |
| `src/bin/markturbo-goal07-evaluate.rs` | Capture decode, content-free receipts, decision binding/replay rejection, identities, file bounds and corpus guard. | Retain: evaluator receipt/CLI seam. |
| `src/credentials.rs` | Credential precedence/non-persistence, verified writes/quarantine, migration races, isolation, redaction, validation and locks; two ignored Windows acceptance tests. | Retain, including ignored tests. |
| `src/diagnostic.rs` | Line formatting and severity ordering. | Retain: direct value semantics. |
| `src/document/block.rs` | Fence mapping, unknown diagrams and renderer-ID dispatch. | Retain: lower-level mapping contract. |
| `src/document/doc.rs` | Explicit-type parsing: source, blocks/frontmatter/MDX, diagnostics, reparsing, ranges and lookup. | Retain; see constructor counterexample. |
| `src/document/doctype.rs` | Type allowlist, artifact conventions, render/open predicates, labels, case and directory-sensitive names. | Retain: canonical classification. |
| `src/document/frontmatter.rs` | BOM/CRLF fence splitting, malformed/empty YAML and exact offsets. | Retain: raw split/offset layer. |
| `src/document/io.rs` | Load/save fidelity, encodings, identity/conflict authorization, Save As/recreate, symlinks and Skill commit races. | Retain: filesystem transaction boundary. |
| `src/document/io/commit.rs` | Test-only commit, exchange, cleanup and reservation race hooks used by `io.rs`. | Retain/support. |
| `src/document/lifecycle.rs` | Dirty close decisions and async revision/generation/save-as revalidation. | Retain: state machine above I/O. |
| `src/document/outline.rs` | JSX labels, Unicode-safe truncation and MDX structural entries. | Retain: direct helper semantics. |
| `src/model.rs` | One-shot consent binding, Skill framing/limits and fresh Revision binding. | Retain: authorization domain. |
| `src/model/endpoint.rs` | URL identity, credential targets, safe persistence, loopback/proxy/TLS and vendor ambient-key policy. | Retain: endpoint trust boundary. |
| `src/recovery.rs` | Encrypted compatibility/validation, root/lease safety, quotas/transactions, cancellation, retirement, scheduling and DPAPI; ignored capacity diagnostic. | Retain: recovery protocol; ignored test owns the 8-second budget. |
| `src/rendering/mod.rs` | Registry/backends/fonts, outputs, panic/time/size containment, cache, PlantUML and diagnostics. | Retain: actual renderer integration. |
| `src/review.rs` | Frozen request/output validation, quote/anchor recovery, scope bounds, binary digest and canonical Skill framing. | Retain: provider-independent invariants. |
| `src/review/provider.rs` | Schemas/payloads, limits, credentials/redaction, cancellation/timeout/streaming, wire formats and Revision coverage. | Retain: transport/wire boundary. |
| `src/review/revision.rs` | Exact edit validation/composition, decisions, bounds, Unicode/CRLF offsets and randomized edits. | Retain: local patch engine. |
| `src/runtime_paths.rs` | Versioned samples, concurrent publication, escape prevention, data roots and overrides. | Retain: runtime filesystem boundary. |
| `src/settings.rs` | Defaults/schema/TOML, secret exclusion, stale-write races, locks, fallback, MRU and settings path. | Retain: persistence boundary. |
| `src/translate.rs` | Lossless segmentation, protected markup, scopes, cardinality, CJK and byte-boundary widening. | Retain: provider-independent transform. |
| `src/translate/provider.rs` | Consent isolation, credential/endpoint policy, clients/redirect/proxy, redaction, loopback payloads and decoding. | Retain: transport boundary. |
| `src/workspace/history.rs` | Forget, branch truncation and navigation suppression. | Retain: independent transitions. |
| `src/workspace/search.rs` | Coordinates/identity/casing/caps, decoding/binary/size/noise, fairness, complexity and symlinks. | Retain: search-specific integration/performance. |
| `src/workspace/tabs.rs` | Active/preview/menu transitions, close/focus, payloads and source identities. | Retain: tab state machine. |
| `src/workspace/tree.rs` | Listing/order/noise, openability, binary visibility, symlink non-descent, paths and errors. | Retain: tree integration. |
| `src/workspace/walk.rs` | Shared noise policy, path filtering, NUL sniff and open gate. | Retain: policy definition. |
| `src/workspace/watcher.rs` | Real notifications, extra roots, synchronization, noise, nonblocking poll, no startup walk, dedup and shape flags. | Retain: OS watcher boundary. |
| `tests/discovery.rs` | Temporary-tree Skill walking: roots, depth, skip list, symlink dedup/cycles, internal/global policy, leaf and missing root. | Retain: authoritative leaf regression. |
| `tests/fixtures.rs` | Checked-in Markdown/diagram/MDX/Skill corpus across parser/discovery seams. | Retain file; candidate only `discovery_does_not_descend_into_a_skill`. |
| `tests/model_first_use_cost.rs` | Ignored first-versus-subsequent loopback transport timing through preparation/consent/execute. | Retain ignored diagnostic. |
| `tests/performance.rs` | Serialized 10K/100K parse bounds, overhead, background justification, repeated/no-op edits, diagrams and lookup cost. | Retain: size/shape-specific proofs. |

All 38 test-bearing/test-support files are classified. Files such as
`src/model/transport.rs` and `src/review/provider/revision.rs` are covered by
parent tests but contain no test or test-only item, so are not inventory entries.

## Static duplicate/no-detection candidate

### Candidate A: `fixtures::discovery_does_not_descend_into_a_skill`

The test claims to prove that a Skill directory is a leaf, but checked-in
`fixtures/skills/skills/valid-skill` contains only `assets/template.json`,
`references/guide.md`, and `scripts/run.sh`; no nested `SKILL.md` exists.
`skill::walk` emits a Skill only when `entry_path` finds that file. Descending
into these directories therefore cannot produce the forbidden names `scripts`
or `references`; the assertion stays green if the production leaf `return` is
removed. It has zero detection for its named behavior.

`tests/discovery.rs::a_skill_directory_is_a_leaf` creates both
`skills/outer/SKILL.md` and `skills/outer/inner/SKILL.md`, calls the same public
`skill::discover`, and requires `outer` present and `inner` absent. It strictly
owns the scenario.

Smallest mutation: in `agent_artifacts/skill.rs::walk`, delete only the `return;`
after `out.insert(skill);`. Expected: retained test fails by finding `inner`;
candidate remains green; the full mutated suite is red with or without candidate.

Executable evidence plan (disposable worktree; **not run in this audit**):

```sh
cargo test --locked --profile ci -p mt-core --all-features
cargo test --locked --profile ci -p mt-core --all-features --test fixtures \
  discovery_does_not_descend_into_a_skill -- --exact
cargo test --locked --profile ci -p mt-core --all-features --test discovery \
  a_skill_directory_is_a_leaf -- --exact

# Remove only the leaf return after out.insert(skill), then:
cargo test --locked --profile ci -p mt-core --all-features --test fixtures \
  discovery_does_not_descend_into_a_skill -- --exact
cargo test --locked --profile ci -p mt-core --all-features --test discovery \
  a_skill_directory_is_a_leaf -- --exact
cargo test --locked --profile ci -p mt-core --all-features

# Delete only the candidate while retaining the mutation, then compare:
cargo test --locked --profile ci -p mt-core --all-features
```

Accept ablation only if baseline is green, the mutation fails the retained test,
the candidate stays green, and full versus ablated mutated selections fail for
the same retained reason. Restore the mutation before proposing deletion.

## Similar-looking tests that are not redundant

`document::doc::tests::source_is_preserved_verbatim` uses `Document::with_type`;
`fixtures::markdown_fixture_survives_a_lossless_round_trip` uses path-based
`Document::new`. A discriminating fault is to normalize only the `source` stored
by `Document::new` (for example `source.trim_end().to_owned()`). The fixture test
fails while the unit passes; the reverse constructor-only fault reverses the
result. Their equal assertions are therefore not redundant.

The same protects apparent MDX/diagram duplicates: fixture tests cross
`DocType::of` plus `Document::new`; units use explicit type plus `with_type`.
Skill fixture assertions cross `discover -> load -> parse`; dropping diagnostics
or choosing the wrong entry after parsing can evade direct parser units.

## Ignored tests and coverage limits

Four ignored tests are in scope:

1. `credentials::tests::goal_05a_credential_lifecycle_global_lock_fails_closed_when_busy`
2. `credentials::tests::goal_05a_credential_lifecycle_windows_round_trip`
3. `recovery::tests::recovery_capacity_batch_commits_within_budget`
4. `model_first_use_cost::first_model_transport_use_cost`

They are outside ordinary green selection. Run documented owner/platform probes
separately before ablation in those domains; ordinary `cargo test` is not proof.

Coverage gaps:

- Candidate A has not had its mutation experiment; static analysis is not execution evidence.
- Symlink tests may return early on Windows without link privileges; report skips.
- Ignored Windows/DPAPI/cost tests are not selected by the ordinary full command.
- `untrusted_mdx_opens_without_executing_anything` asserts type/source, not an observable no-execution side effect. Retain it as a threat-model sentinel, not sandbox proof.
- Serialized wall-clock tests remain hardware diagnostics; static analysis cannot establish thresholds.

No other whole-test ablation is recommended statically. Repeated values generally
cross another constructor, filesystem, authorization, serialization or CLI seam;
deleting one merely because both are green is the prohibited shortcut.
