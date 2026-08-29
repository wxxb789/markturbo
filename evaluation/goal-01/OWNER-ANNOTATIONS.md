# Project-Owner Evaluation Annotations

**Authorship record:** On 2026-08-29 the project owner placed an approval comment
after every artifact, then explicitly stated in the Goal 01 thread that each
`LGTM` approves and adopts all three annotation fields above it. Those fields are
therefore the owner's evaluation intent. The agent only normalized the approval
record and retained the owner's additional notes below.

The `Human source` entries identify evidence used to ground the annotations.
Their wording is not a required model response: a Review result passes when it
surfaces equivalent grounded meaning without contradicting the artifact.

## TP-01 — Upgrade GPUI Without Preview Regression

- Useful questions or findings: Identify that recursive layout must remain
  stack-safe after `stacker` became opt-in, and that the `windows` upgrade is
  constrained by the `lb-wry` source boundary.
- Scoring items: `TP-01-HI-01`, `TP-01-HI-02` in `SCORING.md`.
- Intent-violating change: Treating a successful compile as sufficient while
  preview behavior, the WebView acceptance path, or the intended feature graph
  regresses.
- Observable outcome: Release workspace tests, the Python harness, Windows
  WebView acceptance, Clippy, and the expected GPUI feature graph all pass.
- Human source: commit `a29308f` subject and body.

## TP-02 — Keep Web Preview In One Window

- Useful questions or findings: Identify the need for a dedicated Windows STA
  worker, a private `WS_CHILD` host, and overlay-free chrome around the active
  browser child window.
- Scoring items: `TP-02-HI-01`, `TP-02-HI-02`, `TP-02-HI-03` in `SCORING.md`.
- Intent-violating change: A companion-window or hide-on-overlay design that
  splits the application or blanks the preview.
- Observable outcome: One application top-level window survives the resize
  transitions, closes cleanly through `WM_CLOSE`, and produces no
  `RefCell already borrowed` diagnostic.
- Human source: commit `b56881d` subject and body.

Owner note: Make the WebView look as native as practical.

## TP-03 — Diagnose Duplicate Git Crates

- Useful questions or findings: Surface the buried `multiple different versions`
  note and explain that `git = "URL"` and `git = "URL", rev = "..."` are distinct
  Cargo sources even when they resolve to equivalent code.
- Scoring items: `TP-03-HI-01`, `TP-03-HI-02` in `SCORING.md`.
- Intent-violating change: Adding trait imports, toggling feature flags, or
  tightening the manual revision pin after the duplicate-source evidence is
  established.
- Observable outcome: Every shared git dependency uses the same URL and source
  selector, `Cargo.lock` supplies the revision, and `gpui` appears as one
  dependency instance.
- Human source: `docs/solutions/build-errors/cargo-git-rev-pin-duplicate-crate.md`.

## TP-04 — Measure The Release Profile On A Quiet Host

- Useful questions or findings: Call out the pre-registered quiet-machine gate,
  the A-B-B-A comparison, and the distinction between binary-size evidence and
  runtime evidence.
- Scoring items: `TP-04-HI-01`, `TP-04-HI-02`, `TP-04-HI-03` in `SCORING.md`.
- Intent-violating change: Adopting `opt-level = "s"` from its smaller binary
  alone or reporting runtime results from a host that never passed the quietness
  gate.
- Observable outcome: Either a valid quiet-host startup and first-formula
  comparison supports a decision, or the comparison remains explicitly deferred
  and the current release profile is unchanged.
- Human source: commit `11d6c67` and `docs/TODO.md`.
Owner note: This case is used to measure performance.

## SP-01 — Guarantee User-Text Safety

- Useful questions or findings: Find any path where a normal action,
  asynchronous result, external change, encoding conversion, symbolic-link save,
  or recovery operation could silently replace newer user-authored text.
- Scoring items: `SP-01-HI-01` through `SP-01-HI-06` in `SCORING.md`.
- Intent-violating change: Continuous autosave to the source file, cloud
  recovery, or a lifecycle shortcut that bypasses Save / Discard / Cancel.
- Observable outcome: Every enumerated destructive path preserves exact text or
  obtains an explicit decision, and interruption restores the latest completed
  checkpoint within the approved loss window.
- Human source: `snapshots/goals/02-guarantee-user-text-safety.md`, especially Product
  invariant, In scope, Required cases, and Out of scope.
Owner note: Be careful and pragmatic, and do not harm performance.

## SP-02 — Create First-Use Document Flow

- Useful questions or findings: Identify that a new user must reach an editable
  Markdown buffer without another editor, a pre-existing file, or terminal
  knowledge, and that the unsaved buffer must reuse Goal 02's lifecycle path.
- Scoring items: `SP-02-HI-01` through `SP-02-HI-05` in `SCORING.md`.
- Intent-violating change: Requiring a workspace before pasting, introducing a
  proprietary file type, or pulling Review, Translation, accounts, or cloud
  storage into the first-use goal.
- Observable outcome: A clean-profile user creates or pastes, saves through the
  existing safe Save As path, reopens the same ordinary `.md` bytes, and can use
  the bundled sample without a terminal.
- Human source: `snapshots/goals/03-create-first-use-document-flow.md`, especially User
  outcome, In scope, Out of scope, and Completion evidence.
## SP-03 — Protect Model Credentials And Request Privacy

- Useful questions or findings: Identify every credential path, endpoint-identity
  transition, consent boundary, redirect risk, request-body exposure, and place a
  key or private payload could enter settings, recovery, logs, arguments, errors,
  screenshots, or fixtures.
- Scoring items: `SP-03-HI-01` through `SP-03-HI-12` in `SCORING.md`.
- Intent-violating change: Falling back to plaintext persistence, silently
  reusing a vendor key for a custom host, or treating consent for one selection
  and endpoint as consent for a workspace.
- Observable outcome: Persistent keys live only in the approved secure store,
  displayed outbound scope equals the user content supplied to the adapter, and
  sentinel scans find no unintended key or request-body occurrence.
- Human source:
  `snapshots/goals/05a-protect-model-credentials-and-request-privacy.md`, especially In
  scope, Required proof cases, and Completion evidence.
Owner note: Learn from comparable applications such as Alma and Cherry Studio.

## SP-04 — Deliver Read-Only Review

- Useful questions or findings: Distinguish stated goal, context, constraints,
  deliverable, success evidence, inferred assumptions, and unresolved decisions;
  ask only questions whose answers could materially change the outcome.
- Scoring items: `SP-04-HI-01` through `SP-04-HI-08` in `SCORING.md`.
- Intent-violating change: Rewriting source, emitting a generic numerical score,
  inventing line anchors, hiding omitted selection context, or displaying more
  than five clarification questions.
- Observable outcome: Document and selection Review decode into inert structured
  output, remain source-byte neutral in every state, navigate grounded findings,
  and meet the pre-registered corpus threshold.
- Human source: `snapshots/goals/06-deliver-read-only-intent-review.md`, especially Product
  behavior, In scope, Out of scope, and Completion evidence.
## AI-01 — Repository Agent Instructions

- Useful questions or findings: Identify the required product, goal,
  architecture, and vocabulary sources; the numbered-goal prerequisite rule;
  measurement requirements; and the structural invariants that must survive a
  change.
- Scoring items: `AI-01-HI-01` through `AI-01-HI-07` in `SCORING.md`.
- Intent-violating change: Bypassing an earlier goal, editing unrelated work,
  reporting an unmeasured number, adding GPUI to `mt-doc`, mutating WebView state
  from `render`, or dropping required release validation without disclosure.
- Observable outcome: Work stays scoped, follows the active goal and project
  vocabulary, preserves every named invariant, and reports the exact validation
  that ran or could not run.
- Human source: `snapshots/agent-instructions/repository-AGENTS.md`.
## AI-02 — Sample Workspace Agent Instructions

- Useful questions or findings: Separate the explanatory description from the
  operational conventions to keep changes scoped, run tests, and prefer an
  existing helper.
- Scoring items: `AI-02-HI-01` through `AI-02-HI-04` in `SCORING.md`.
- Intent-violating change: Treating the future Effective Agent Context diagram as
  already implemented behavior or expanding the example into a new subsystem.
- Observable outcome: A change to the sample is bounded, uses existing helpers,
  and is accompanied by the relevant passing tests.
- Human source: `snapshots/agent-instructions/sample-workspace-AGENTS.md`.
## AS-01 — GPUI Skill

- Useful questions or findings: Select the reference that matches the actual
  GPUI concept before answering, including the extended reference for complex
  Element, Entity, or testing work.
- Scoring items: `AS-01-HI-01` through `AS-01-HI-04` in `SCORING.md`.
- Intent-violating change: Answering a GPUI-specific question from analogy or
  loading unrelated reference families while omitting the file named for the
  task's concept.
- Observable outcome: The agent uses the relevant current reference for actions,
  async work, context, elements, entities, events, focus, globals, layout,
  identity, or testing and grounds its implementation in that API.
- Human source: `snapshots/skills/gpui/SKILL.md` and its Navigation table.
## AS-02 — gpui-component Skill

- Useful questions or findings: Require the Design Guide before UI decisions and
  the Coding Guide before architecture or state-ownership decisions, then locate
  the real component API rather than inventing one.
- Scoring items: `AS-02-HI-01` through `AS-02-HI-04` in `SCORING.md`.
- Intent-violating change: Copying a React/CSS convention by analogy, using raw
  colors or pixel spacing, hiding interaction state, choosing index-based
  identities, or using a `Link` for an in-app command.
- Observable outcome: The result uses the actual `gpui-component` API, semantic
  theme tokens, rem-based spacing, stable identities, visible states, correct
  desktop interaction patterns, and the appropriate existing component.
- Human source: `snapshots/skills/gpui-component/SKILL.md`, especially Read This
  First and Non-negotiables.
