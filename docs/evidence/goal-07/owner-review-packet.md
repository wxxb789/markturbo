# Goal 07 Owner Review Packet

Date: 2026-09-20

This packet summarizes the 12 private Goal 07 captures for an explicit project-owner decision. It is not approval evidence by itself.

Proposed common decision:

- Eligibility: all 12 artifacts are revision-eligible.
- Change decision: accept the single change in each artifact.
- Question coverage: the answered material question is represented by that change.
- Intent judgment: the accepted output preserves the artifact's intent.
- Intent-violating change IDs: none of the proposed changes match the prohibited change described in `OWNER-ANNOTATIONS.md`.
- Clarity claim: mark `TP-01` as clearer because the answered question becomes an explicit acceptance rule; leave the other cases false to make only the minimum required claim.

## AI-01

Path: `evaluation/goal-01/snapshots/agent-instructions/repository-AGENTS.md`

Question: When a validation command cannot run, must the report name both that exact command and the concrete reason?

Answer: Yes. Name each unavailable command and its concrete reason, and never describe an unrun check as passing.

Proposed appended text:

```markdown
## Clarified unavailable-validation reporting

Each unavailable validation must be reported by exact command and concrete reason. An unrun check must never be described as passing.
```

## AI-02

Path: `evaluation/goal-01/snapshots/agent-instructions/sample-workspace-AGENTS.md`

Question: Does the Effective Agent Context diagram authorize implementing inheritance resolution in this sample now?

Answer: No. Keep the diagram explicitly future-facing; v0.1 owns recognition only.

Proposed appended text:

```markdown
## Clarified diagram status

The Effective Agent Context diagram is explanatory future context only. It does not authorize implementing inheritance resolution or a new subsystem in this sample.
```

## AS-01

Path: `evaluation/goal-01/snapshots/skills/gpui`

Question: For complex Element, Entity, or testing work, should an Extended Reference replace the matching Navigation reference?

Answer: No. Read the matching Navigation reference first, then load only the Extended Reference needed for deeper API, pattern, or test detail.

Proposed appended text:

```markdown
## Reference order

Start with the Navigation reference that matches the concrete GPUI concept. Load an Extended Reference only when the task requires its deeper API, pattern, or test detail.
```

## AS-02

Path: `evaluation/goal-01/snapshots/skills/gpui-component`

Question: Can the entrypoint summary and Non-negotiables substitute for reading the applicable guide and current component API?

Answer: No. Read the applicable guide file and inspect the current component API before implementation.

Proposed appended text:

```markdown
## Clarified authority order

Read the applicable guide file and inspect the current component API before implementation. The summaries and Non-negotiables on this page do not substitute for either.
```

## SP-01

Path: `evaluation/goal-01/snapshots/goals/02-guarantee-user-text-safety.md`

Question: May matching file length and modification time alone authorize an overwrite to avoid extra I/O?

Answer: No. A matching stamp may avoid an eager read, but it cannot authorize overwrite; verify content before the destructive save decision.

Proposed appended text:

```markdown
## Clarified conflict check

A matching file length and modification time may avoid an eager read, but it cannot authorize an overwrite. Verify content before any destructive save decision while preserving the documented recovery cadence.
```

## SP-02

Path: `evaluation/goal-01/snapshots/goals/03-create-first-use-document-flow.md`

Question: Must New or Paste work before the user selects a workspace?

Answer: Yes. New and Paste must create an editable unsaved Markdown buffer before any workspace is selected and reuse Goal 02 lifecycle handling.

Proposed appended text:

```markdown
## Clarified first-use prerequisite

New and Paste must create an editable unsaved Markdown buffer before any workspace is selected, and that buffer must use Goal 02's existing dirty-close and recovery path.
```

## SP-03

Path: `evaluation/goal-01/snapshots/goals/05a-protect-model-credentials-and-request-privacy.md`

Question: Can changing base_url to a custom host carry an ambient vendor API key to that host automatically?

Answer: No. A custom host requires separate explicit authorization bound to its normalized endpoint identity before the vendor key can be sent.

Proposed appended text:

```markdown
## Clarified custom-host credential boundary

Changing base_url to a custom host must never carry an ambient vendor key with it. Sending that credential requires a separate explicit authorization bound to the custom endpoint identity.
```

## SP-04

Path: `evaluation/goal-01/snapshots/goals/06-deliver-read-only-intent-review.md`

Question: Should Review return boilerplate questions merely to fill the five-question allowance when no material decision is unresolved?

Answer: No. Return no question when none could materially change the outcome; the five-question rule is a maximum, not a quota.

Proposed appended text:

```markdown
## Clarified question materiality

If no clarification question could materially change the outcome, return no question rather than filling the five-question limit with boilerplate. The limit is a maximum, not a quota.
```

## TP-01

Path: `evaluation/goal-01/task-prompts/01-upgrade-gpui-without-regression.md`

Question: Should the prompt make the compile-versus-preview acceptance boundary explicit in one normative statement?

Answer: Yes. State that compilation alone is insufficient unless the expected GPUI feature graph and Windows WebView acceptance also pass.

Proposed appended text:

```markdown
## Clarified acceptance boundary

Compilation alone is not success. The expected GPUI feature graph, release-profile workspace tests, Python harness, Clippy, and Windows WebView acceptance must all pass without preview regression.
```

## TP-02

Path: `evaluation/goal-01/task-prompts/02-keep-web-preview-in-one-window.md`

Question: Should native visual integration around the active WebView be stated as an explicit revision requirement?

Answer: Yes. Require the WebView to remain inside the single GPUI window and appear integrated with native, overlay-free chrome.

Proposed appended text:

```markdown
## Clarified native integration

The active WebView must remain inside the single GPUI top-level window and appear integrated with native, overlay-free chrome; overlays must neither create a companion window nor blank the preview.
```

## TP-03

Path: `evaluation/goal-01/task-prompts/03-diagnose-duplicate-git-crates.md`

Question: Should the proposed fix explicitly remove mixed git source selectors and leave the resolved revision to Cargo.lock?

Answer: Yes. Align every shared git dependency on the same URL and source selector, and let Cargo.lock record the resolved revision.

Proposed appended text:

```markdown
## Clarified source-selector fix

Treat git = "URL" and git = "URL", rev = "..." as distinct Cargo sources. Align every shared git dependency on the same URL and source selector, and let Cargo.lock record the resolved revision.
```

## TP-04

Path: `evaluation/goal-01/task-prompts/04-measure-release-profile-on-quiet-host.md`

Question: How should the revision handle a smaller binary when the quiet-host runtime gate does not pass?

Answer: Keep the current release profile, record size separately, and defer the runtime decision until a quiet host completes the A-B-B-A comparison.

Proposed appended text:

```markdown
## Clarified decision rule

Record binary-size and runtime evidence separately. A smaller binary does not justify adopting opt-level = "s" unless the pre-registered quietness gate passes and the A-B-B-A runtime comparison supports the change.
```

## Owner Confirmation

To approve the proposed common decision, reply with:

`Approve all 12 Goal 07 proposals as revision-eligible; accept each change; confirm each accepted output preserves intent and each answered question is represented; confirm no proposed change is intent-violating; mark only TP-01 as clearer due to the answered question.`

Otherwise list any artifact IDs whose eligibility, change decision, intent judgment, question coverage, or clarity judgment should differ.
