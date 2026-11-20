# Goal 14 — Upgrade every nsh crate to 0.6.1

## Objective

MarkTurbo builds against the newest nsh release, 0.6.1 on crates.io (released 2026-11-20), with
every nsh crate on the same version and all existing behaviour preserved. The follow-on design
work then targets the 0.6 animation and interaction APIs, not 0.5.1.

## Done when

1. Every nsh dependency in the workspace manifests is pinned to `0.6.1`. This covers `nsh-core`,
   `nsh-base`, `nsh-engine`, `nsh-testing` and the three `nsh-platform-*` crates. `Cargo.lock`
   resolves one version of each nsh crate, 0.6.1, and was updated together with the manifests.
   `cargo tree -d` shows no duplicate nsh crate.
2. The vendored RaTeX patch (`vendor/ratex-parser`) still matches the RaTeX version that
   `nsh-base` 0.6.1 selects, and its allocation clamp is retained.
   - If nsh-base 0.6.1 moves to a newer RaTeX, re-vendor that exact version and keep the clamp.
   - Record the version in `Cargo.toml`'s comment and in `AGENTS.md`.
   - A provenance change (a new source or a git pin) is not allowed. Stop and report instead.
3. Every 0.6.0/0.6.1 breaking change that touches this code is migrated at the call site, with no
   compatibility shims. Changes known from the 0.6.0 release notes:
   - `Transition` is `#[non_exhaustive]`.
   - `ContextMenu` listeners take `ContextMenuEvents`.
   - `clip_utf16` is now `clip_chars`, and `maxLength` counts characters.
   - `AlertDialogOptions::show` returns a named type.
   - `ModalConfig` is no longer `Copy`.
   - `RichEditorEvent` gains `CompositionChange`.
   - App capabilities must list `application` / `homeDir` if declared.
   - Windows WebView2 and media became optional features.

   For the last item: if MarkTurbo uses either one, enable it explicitly. The preview WebView must
   still work.
4. Every statement in code comments, `AGENTS.md`, `README.md` and `docs/` that names an nsh 0.5.1
   behaviour is re-verified against the 0.6.1 crate source. Each is either updated to 0.6.1 with
   the behaviour confirmed, or corrected where the behaviour changed. Never bump a version number
   without checking the claim. Key claims to check:
   - Windows inputs load only the first font family, and only from system fonts
     (`tokens::FONT_INPUT`).
   - Windows focuses only text inputs (`consent.rs`, `notices.rs`).
   - No key-up events are delivered (`consent.rs`).
   - The UIA exposure rules in `docs/verification.md` and
     `scripts/markturbo_tools/native/runtime.py`.

   Behaviour that changed in MarkTurbo's favour is reported, but not exploited, in this goal.
5. `uv run --locked --project scripts scripts/mt.py check ci` passes on Windows. It runs fast,
   formatting, optimized Clippy and the workspace tests, including the `nsh_testing::Headless` UI
   tests. The per-PR CI is green on the PR head (Linux tests, macOS build, Windows release tests).
6. The release build passes native acceptance on an active Windows 11 desktop:
   - `mt.py accept all --exe <release exe> --expect-exe-sha256 <hash>` passes goals 02, 03, 06 and
     07;
   - the visual matrix `mt.py visual -- --exe <release exe> --out .scratch/goal-14-visual` captures
     every state.

   If the desktop is unavailable, this clause stays pending and the PR records it. It is not
   waived.

7. The on-disk data location is unchanged: settings, the recovery store and the companion font.
   nsh 0.6.0 added an app-named data directory (#2232). Check whether any path that MarkTurbo or
   nsh resolves for MarkTurbo moves. A unit test, or a recorded before/after resolution, should
   show that the paths are the same.
   - If a path would move, stop and report. Do not migrate, copy or drop user files without the
     owner's decision.

## Failure control

- Two versions of any nsh crate in `Cargo.lock`, or a manifest still on 0.5.x.
- A test deleted, ignored or weakened to get the upgrade green.
- A comment or doc that says 0.6.1 for a behaviour nobody checked against the 0.6.1 source.
- A RaTeX allocation clamp that is dropped, or a RaTeX version that no longer matches nsh-base.
- A new compiler or Clippy warning in `mt-app`.

## Out of scope

- Adopting the new 0.6 features: spring transitions, layout animation and so on. The design
  upgrade goal that follows does that.
- Exploiting a fixed limitation, for example dropping `FONT_INPUT`'s Windows exception. Report it
  instead.
- Upgrading non-nsh dependencies, unless 0.6.1 requires it. If it does, name each one.

## Evidence plan

- Local: `mt.py check ci` on Windows. `cargo tree -d -p mt-app` for duplicates.
- CI: the per-PR pipeline on the PR head.
- Native: `mt.py accept all` and `mt.py visual` on the release build, on an active desktop.
  Receipts stay under the git-ignored `.scratch/`.

## Mergeable alone

Yes. It is a dependency upgrade with call-site migrations, and it needs no sibling change.
