# mt-app test-ablation inventory

Date: 2026-09-29. Scope: every first-party file under `crates/mt-app/src` and
`crates/mt-app/tests` containing a test, ignored test, or `#[cfg(test)]` support.
This is a static inventory of the current working tree, which already had
unrelated edits. No test/build was run and no source was changed.
`crates/mt-app/Cargo.toml` sets `[lib] doctest = false`; the scoped search found
no fenced Rust doctest (the ` ```rust` occurrences are test data/comments).
There are no runnable doc tests in scope.

The inventory finds **27 files**, **370 declared tests**, and **8 ignored
tests**. Counts precede target-`cfg` filtering. “Candidate” means eligible only
after the paired experiment below, not approved for deletion now.

## Ablation rules

1. A survivor must detect the same plausible production fault, not merely pass
   the same happy path. Preserve source bytes/newlines/encoding, dirty state,
   recovery ownership/durable retirement, consent/frozen identity, trust
   restriction, and OS evidence.
2. Unit, GPUI, integration, and native tests are not interchangeable. Units
   localize policy; GPUI proves entity/event/focus/rendered-control wiring;
   integration proves real files/composed pipelines; Windows acceptance proves
   IME, clipboard, dialogs, DPAPI/session, WebView2 HWND/STA, and foreground.
3. Native and Web paths are distinct. Native preview exercises GPUI text/SVG;
   Web tests exercise HTML/CSP/URLs; `web_surface` tests exercise lifecycle
   state but do not instantiate real WebView2. Pending WebView native acceptance
   is not a deletion opportunity.
4. Ignored diagnostics stay ignored. Delete one only when a survivor emits the
   same decision-useful measurements, not because it normally does not run.

## Complete file disposition

| File | Declared | Behavior / proof boundary | Disposition |
|---|---:|---|---|
| `src/app_paths.rs` | 2 | Debug Welcome uses repository sample; embedded materialization opens and preserves edits. Real temp filesystem/installer. | **Retain.** Asset presence/Welcome opening do not prove repeat-install preservation. |
| `src/assets.rs` | 8 | Exact font/sample/license/icon bytes, delegate fallback, lists, shadow de-duplication, TTF/SVG identity. | **Retain.** Unique release artifact/source-identity proof. |
| `src/i18n.rs` | 14 | Key/language completeness, BCP47, labels/paths, exact model/Revision/Skill disclosures, transport wording, credential privacy. | **Candidate only:** `welcome_strings_are_translated_in_both_languages`; its non-empty checks are an exact subset of `every_key_has_a_string_in_every_language` (and Chinese coverage). Retain 13. |
| `src/main.rs` | 10 | Log privacy/append, CLI targets/errors/help/version, desktop app/icon identity. | **Retain.** Binary-private parsing precedes Welcome tests. |
| `src/metrics.rs` | 6 + const block | Grid, proportional/clamped panels, document floor, ordering, indentation. | **Retain.** Rendered interactions do not exhaust numeric policy; compile-time relations are not runtime duplication. |
| `src/settings.rs` | 4 | Failed persistence cannot publish security state; reconciliation; TOML schema; theme IDs. | **Retain.** Unique security publication failure boundary. |
| `src/startup.rs` | 1 | Versioned, content-free, de-duplicated startup trace. | **Retain.** Privacy/schema boundary. |
| `src/theme.rs` | 15 | Presets/fallback, interpolation/contrast/type, GPUI token copy, selection visibility, tab signal, font fallback. | **Retain.** Web palette consumers cannot detect GPUI token-field drift. |
| `src/views/document.rs` | 27 | Recovery handoff; Web retry UI; reload identity; titles/grammars/IDs; Apply staleness/undo/dirty history/CRLF save; trust-before-mutation. GPUI crosses editor/event/undo/filesystem. | **Candidates:** `document_types_choose_supported_layouts_and_editor_languages` is split across surviving HTML/plain layout and extension tests; `text_documents_default_to_the_source_layout` is exactly subsumed by `views::tests::plain_text_is_offered_the_editor_only`. Retain 25. |
| `src/views/document/preview.rs` | support | Exposes exact Web payload generation to tests. | **Retain while callers survive;** not independent coverage. |
| `src/views/explorer.rs` | 2 | Artifact/file icon distinctions and expanded directory icon. | **Retain.** Semantic mapping below pixel review. |
| `src/views/harness.rs` | 10 | Sections/fallback, dotted-path invalidation, grouping completeness/order, spinner policy. | **Retain.** Core discovery does not prove app grouping/state. |
| `src/views/mod.rs` | 13 | Exhaustive layout visibility/renderer/WebView/key/label/default/availability for every `DocType`. | **Retain;** survivor for both document-layout candidates. Similar GPUI tests prove later wiring, not policy exhaustiveness. |
| `src/views/model_settings.rs` | support | Accessors for masked credential/base-URL drafts and pending operation. | **Retain while Settings tests survive.** Enables privacy observation without public API. |
| `src/views/search.rs` | 4 | Recovered-vs-disk same path, tab order at cap, identical rows with distinct targets, scope labels. | **Retain.** Core search cannot prove app recovered-target identity. |
| `src/views/settings_page.rs` | 8 (1 ignored) | Strings/formatting, secret non-prefill, cancelled migration, endpoint authorization, saved-draft prerequisite, vendor defaults; ignored DirectX secret-invariant screenshot. | **Retain.** Keep screenshot acceptance; replace its fixed sleep with frame readiness before promotion, do not delete it. |
| `src/views/workspace.rs` | 157 | GPUI regressions for tab/save-as/overwrite/symlink/encoding/watcher preservation; translation/Review consent and freshness; trust; recovery deadlines/failures/startup/retirement/destruction; Revision answers/apply/save; recovered search/history; panels; Skill source identity. | **Retain all.** No exact duplicates demonstrated. Similar names cover distinct owner/source/path generations, durable markers, or continuations. Primary app safety detector. |
| `src/views/workspace/recovery.rs` | support | Under test, disables production DPAPI auto-open; tests inject owned reversible/failing stores. | **Retain.** Deterministic recovery isolation does not pretend to prove DPAPI. |
| `src/views/workspace/review.rs` | support | Observers/setters/lens cycler for recovered records, freshness, result, panel state. | **Retain while workspace tests survive.** |
| `src/views/workspace/tests/welcome.rs` | 18 | Eligibility plus GPUI rendered click/type/focus, clipboard routing, picker cancellation, failed open, sample/recent lifecycle, suppression, scrolling. | **Retain all.** Handler vs rendered controls differ; neither proves OS clipboard/dialogs. |
| `src/views/workspace/web_surface.rs` | 14 gated | Windows bounds/hide acknowledgement/channel failure; payload cloning, leases, stale navigation/page events, failure/retry/scroll. | **Retain all.** Deterministic lifecycle, not native WebView acceptance. Retain hide/focus coverage while acceptance is pending. |
| `src/web.rs` | 27 | Markdown/math/MDX, escaped source-preserving diagnostics, CSP, opaque data URLs, theme CSS, block order, raw HTML head/doctype, trusted file URLs. | **Retain all.** Integration composes these; units pinpoint sandbox transforms. |
| `tests/end_to_end.rs` | 13 | Real fixture load -> parse -> render paths -> save; bytes/newlines; diagnostics/source; MDX trust; tree/Skills; 100K lines. No window. | **Retain all.** “Both render paths” proves shared model/output, not pixels/native WebView. |
| `tests/open_document_cost.rs` | 2 ignored | Cold/warm formula and load/parse/render/per-fence timing. | **Retain both.** Cold process state and phase attribution differ. |
| `tests/open_folder_cost.rs` | 2 ignored | Folder phases including watcher/discovery cache; flat `read_dir`. | **Candidate:** `read_dir_on_a_flat_directory` repeats the depth-0 call timed by `attribute_the_cost_of_opening_a_folder`; prove same count on one flat directory. Retain phase diagnostic. |
| `tests/sample_workspace.rs` | 10 | Shipped sample claims: clean open, renders/diagnostics, artifact types/Skills, translation preservation, browsing/Web, LF bytes. | **Retain all.** Pins shipped content, unlike generic fixtures/manifest. |
| `tests/search_cost.rs` | 3 ignored | Real folder/harness attribution and common-vs-rare cap cost. | **Candidate:** `the_cap_bounds_the_filesystem_work_not_only_the_list` repeats `document_paths` and common/rare queries from `attribute_the_cost_of_a_folder_search`; it cannot prove its title because the complete walk occurs first. Retain folder/harness diagnostics. |

## Exact/subsumed candidate ledger

Only these five declarations are candidates; no helper is independently
removable yet.

| Candidate | Surviving detector | Required proof |
|---|---|---|
| `i18n::tests::welcome_strings_are_translated_in_both_languages` | `every_key_has_a_string_in_every_language`, `chinese_covers_every_key` | Candidate absent; blank one Welcome value; completeness must fail; restore green. |
| `document::tests::document_types_choose_supported_layouts_and_editor_languages` | `html_is_offered_the_webview_and_never_the_native_preview`, `plain_text_is_offered_the_editor_only`, `the_editor_highlights_a_file_by_its_extension` | Independently mutate HTML availability, Text default, `.rs` grammar; named survivor fails per mutant. |
| `document::tests::text_documents_default_to_the_source_layout` | `views::tests::plain_text_is_offered_the_editor_only` | Mutate Text default to Native; survivor fails with both candidates absent. |
| `open_folder_cost::read_dir_on_a_flat_directory` | `attribute_the_cost_of_opening_a_folder` depth-0 row | Same flat directory: same entry count; after ablation survivor remains. Diagnostic equivalence, not mutation score. |
| `search_cost::the_cap_bounds_the_filesystem_work_not_only_the_list` | `attribute_the_cost_of_a_folder_search` | Same directory: compare common/rare counts, truncation, walk/search timings; if any useful field is absent, keep/fix it. |

The two document rows name two tests, so total candidates are five. No workspace,
recovery, consent, trust, real-file integration, or OS detector is a candidate.

## Smallest paired experiments and commands

Use disposable clean worktrees so full and cut use one revision/toolchain; do
not use this already-dirty inventory tree.

```sh
git worktree add ../markturbo-ablation-full HEAD
git worktree add ../markturbo-ablation-cut HEAD
```

### Baseline and candidate-only comparison

In each worktree run once; in `-cut`, remove only the five candidates. The
ordinary suite remains green and `--list` drops three default tests (two removed
ones are ignored diagnostics).

```sh
cargo test --locked --profile ci -p mt-app -- --list
cargo test --locked --profile ci -p mt-app
cargo test --locked --profile ci -p mt-app every_key_has_a_string_in_every_language
cargo test --locked --profile ci -p mt-app plain_text_is_offered_the_editor_only
cargo test --locked --profile ci -p mt-app html_is_offered_the_webview_and_never_the_native_preview
cargo test --locked --profile ci -p mt-app the_editor_highlights_a_file_by_its_extension
```

### Candidate fault injection (cut worktree)

Apply one mutant, run its survivor, require red for the intended assertion,
then revert and require green:

* `i18n.rs`: return `""` for one Welcome key/language -> run
  `cargo test --locked --profile ci -p mt-app every_key_has_a_string_in_every_language`.
* `views/mod.rs`: Text defaults to Native -> run
  `cargo test --locked --profile ci -p mt-app plain_text_is_offered_the_editor_only`.
* `views/mod.rs`: add Native to HTML -> run
  `cargo test --locked --profile ci -p mt-app html_is_offered_the_webview_and_never_the_native_preview`.
* `document.rs`: map `.rs` to Plain -> run
  `cargo test --locked --profile ci -p mt-app the_editor_highlights_a_file_by_its_extension`.

A passing mutant vetoes ablation. Static similarity is not evidence.

### Ignored diagnostic equivalence

```sh
MARKTURBO_BENCH_DIR=Q:/path/to/flat-notes cargo test --locked --release -p mt-app --test open_folder_cost -- --ignored --nocapture
MARKTURBO_BENCH_DIR=Q:/path/to/vault cargo test --locked --release -p mt-app --test search_cost -- --ignored --nocapture
```

Capture counts, truncation, and timing fields in full and cut. Green exit status
does not establish observational equivalence.

### Mandatory safety mutation controls

Each representative mutant must be killed by a survivor; revert between them.

| Mutant | Surviving focused commands |
|---|---|
| Force LF save or allow overwrite after stamp drift | `cargo test --locked --profile ci -p mt-app a_crlf_document_is_not_converted_by_a_save`; `cargo test --locked --profile ci -p mt-app loaded_crlf_unicode_markdown_applies_selected_hunks_and_saves_crlf`; `cargo test --locked --profile ci -p mt-app save_action_refuses_decode_loss_without_changing_original_bytes` |
| Accept stale Apply, clear warning from stale completion, or retire before durable marker | `cargo test --locked --profile ci -p mt-app manual_edit_before_apply_returns_stale_without_mutating_any_document_state`; `cargo test --locked --profile ci -p mt-app stale_recovery_completion_cannot_clear_a_newer_attempt`; `cargo test --locked --profile ci -p mt-app discard_keeps_the_tab_open_when_recovery_retirement_fails` |
| Send before consent or read post-consent editor text | `cargo test --locked --profile ci -p mt-app cancelling_translation_consent_sends_no_request`; `cargo test --locked --profile ci -p mt-app changing_the_document_invalidates_pending_translation_consent`; `cargo test --locked --profile ci -p mt-app approving_translation_consent_sends_one_frozen_request` |
| Remove `default-src 'none'` or mutate trusted HTML/MDX before restricting | `cargo test --locked --profile ci -p mt-app restricted_csp_blocks_scripts_and_network`; `cargo test --locked --profile ci -p mt-app trusted_html_apply_restricts_before_mutation_and_undo_keeps_it_restricted`; `cargo test --locked --profile ci -p mt-app trusted_mdx_save_as_html_is_restricted_before_the_new_web_payload` |
| Accept stale navigation/hide acknowledgement | `cargo test --locked --profile ci -p mt-app stale_worker_navigation_event_cannot_finish_a_replacement_attempt`; Windows: `cargo test --locked --profile ci -p mt-app stale_source_hide_acknowledgement_is_rejected` |

After reverting mutants, run the app gate once:

```sh
cargo test --locked --profile ci -p mt-app
python scripts/mt.py check ci
```

`check ci` is headless per `docs/development.md`; use the repository's locked
Python launcher where required.

### Native evidence remains additive

At the next eligible Windows checkpoint, bind results to the final executable
hash and run the applicable complete workflow, not diagnostic `--case`:

```sh
uv run --locked --project scripts python scripts/mt.py accept goal-03
uv run --locked --project scripts python scripts/mt.py accept goal-07
cargo test --locked --release -p mt-app rendered_privacy_surfaces_are_secret_invariant -- --ignored
```

Goal 03/07 are listed because current WebView/focus and consent/revision work
crosses OS boundaries. The DirectX test still does not prove clipboard, IME,
dialogs, DPAPI, WebView2 focus, or session behavior. Pending native WebView
acceptance remains pending, never inferred from `web_surface` units.

## Decision

Experiment only on these five tests. Everything else is retained. Expected
default-suite reduction is three tests, with two ignored diagnostics removed
only after output equivalence. Any surviving mutant or missing diagnostic field
means keep the test. This report is not executed evidence.

