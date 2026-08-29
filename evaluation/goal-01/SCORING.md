# Goal 01 v1 Scoring Registry

**Corpus version:** `goal-01-v1`

## Fixed Threshold

This registry atomizes every high-impact finding or question from the `Useful
questions or findings` field of `OWNER-ANNOTATIONS.md`. It fixes the denominator
at **60** independently scorable items and requires **45** hits:
`ceil(0.75 * 60) = 45`.

The item ID ranges are cross-referenced from their source annotations. The
remaining annotation fields remain grounding context and do not add hidden
items to this denominator.

## Scoring Rules

- Score each item binary: one hit or no hit.
- A hit requires equivalent grounded meaning: the result directly surfaces the
  item as a finding or materially consequential question and correctly grounds
  it in the evaluated artifact. Exact wording is not required.
- A generic statement earns no hit when it does not identify the item's distinct
  condition, boundary, or risk. A result can hit multiple items only when it
  independently surfaces each one.
- Do not award a hit to invented source anchors or assertions that contradict
  the artifact.
- The IDs, item boundaries, denominator, and required hit count are fixed before
  scoring results. They must not change after any result is seen; a needed change
  requires a new owner-approved corpus version.

## Items

### TP-01 - Upgrade GPUI Without Preview Regression

| ID | Independently scorable high-impact finding or question |
|---|---|
| `TP-01-HI-01` | Recursive layout must remain stack-safe after `stacker` became opt-in. |
| `TP-01-HI-02` | The `windows` upgrade is constrained by the `lb-wry` source boundary. |

### TP-02 - Keep Web Preview In One Window

| ID | Independently scorable high-impact finding or question |
|---|---|
| `TP-02-HI-01` | The preview needs a dedicated Windows STA worker. |
| `TP-02-HI-02` | The preview needs a private `WS_CHILD` host. |
| `TP-02-HI-03` | The active browser child window needs overlay-free chrome. |

### TP-03 - Diagnose Duplicate Git Crates

| ID | Independently scorable high-impact finding or question |
|---|---|
| `TP-03-HI-01` | The buried `multiple different versions` note is relevant evidence. |
| `TP-03-HI-02` | `git = "URL"` and `git = "URL", rev = "..."` are distinct Cargo sources even when they resolve to equivalent code. |

### TP-04 - Measure The Release Profile On A Quiet Host

| ID | Independently scorable high-impact finding or question |
|---|---|
| `TP-04-HI-01` | The pre-registered quiet-machine gate is required. |
| `TP-04-HI-02` | The comparison must use the A-B-B-A order. |
| `TP-04-HI-03` | Binary-size evidence and runtime evidence are distinct. |

### SP-01 - Guarantee User-Text Safety

| ID | Independently scorable high-impact finding or question |
|---|---|
| `SP-01-HI-01` | A normal action must not silently replace newer user-authored text. |
| `SP-01-HI-02` | An asynchronous result must not silently replace newer user-authored text. |
| `SP-01-HI-03` | An external change must not silently replace newer user-authored text. |
| `SP-01-HI-04` | An encoding conversion must not silently replace newer user-authored text. |
| `SP-01-HI-05` | A symbolic-link save must not silently replace newer user-authored text. |
| `SP-01-HI-06` | A recovery operation must not silently replace newer user-authored text. |

### SP-02 - Create First-Use Document Flow

| ID | Independently scorable high-impact finding or question |
|---|---|
| `SP-02-HI-01` | A new user must reach an editable Markdown buffer. |
| `SP-02-HI-02` | Reaching that buffer must not require another editor. |
| `SP-02-HI-03` | Reaching that buffer must not require a pre-existing file. |
| `SP-02-HI-04` | Reaching that buffer must not require terminal knowledge. |
| `SP-02-HI-05` | The unsaved buffer must reuse Goal 02's lifecycle path. |

### SP-03 - Protect Model Credentials And Request Privacy

| ID | Independently scorable high-impact finding or question |
|---|---|
| `SP-03-HI-01` | Every credential path must be identified. |
| `SP-03-HI-02` | Every endpoint-identity transition must be identified. |
| `SP-03-HI-03` | Every consent boundary must be identified. |
| `SP-03-HI-04` | Redirect risk must be identified. |
| `SP-03-HI-05` | Request-body exposure must be identified. |
| `SP-03-HI-06` | A key or private payload entering settings must be identified. |
| `SP-03-HI-07` | A key or private payload entering recovery must be identified. |
| `SP-03-HI-08` | A key or private payload entering logs must be identified. |
| `SP-03-HI-09` | A key or private payload entering process arguments must be identified. |
| `SP-03-HI-10` | A key or private payload entering errors must be identified. |
| `SP-03-HI-11` | A key or private payload entering screenshots must be identified. |
| `SP-03-HI-12` | A key or private payload entering fixtures must be identified. |

### SP-04 - Deliver Read-Only Review

| ID | Independently scorable high-impact finding or question |
|---|---|
| `SP-04-HI-01` | Distinguish the stated goal. |
| `SP-04-HI-02` | Distinguish context. |
| `SP-04-HI-03` | Distinguish constraints. |
| `SP-04-HI-04` | Distinguish the deliverable. |
| `SP-04-HI-05` | Distinguish success evidence. |
| `SP-04-HI-06` | Distinguish inferred assumptions. |
| `SP-04-HI-07` | Distinguish unresolved decisions. |
| `SP-04-HI-08` | Ask only questions whose answers could materially change the outcome. |

### AI-01 - Repository Agent Instructions

| ID | Independently scorable high-impact finding or question |
|---|---|
| `AI-01-HI-01` | Identify the required product source. |
| `AI-01-HI-02` | Identify the required goal source. |
| `AI-01-HI-03` | Identify the required architecture source. |
| `AI-01-HI-04` | Identify the required vocabulary source. |
| `AI-01-HI-05` | Identify the numbered-goal prerequisite rule. |
| `AI-01-HI-06` | Identify measurement requirements. |
| `AI-01-HI-07` | Identify the structural invariants that must survive a change. |

### AI-02 - Sample Workspace Agent Instructions

| ID | Independently scorable high-impact finding or question |
|---|---|
| `AI-02-HI-01` | Separate the explanatory description from the operational conventions. |
| `AI-02-HI-02` | Keep changes scoped. |
| `AI-02-HI-03` | Run tests. |
| `AI-02-HI-04` | Prefer an existing helper. |

### AS-01 - GPUI Skill

| ID | Independently scorable high-impact finding or question |
|---|---|
| `AS-01-HI-01` | Select the reference that matches the actual GPUI concept before answering. |
| `AS-01-HI-02` | Use the extended reference for complex Element work. |
| `AS-01-HI-03` | Use the extended reference for complex Entity work. |
| `AS-01-HI-04` | Use the extended reference for complex testing work. |

### AS-02 - gpui-component Skill

| ID | Independently scorable high-impact finding or question |
|---|---|
| `AS-02-HI-01` | Require the Design Guide before UI decisions. |
| `AS-02-HI-02` | Require the Coding Guide before architecture decisions. |
| `AS-02-HI-03` | Require the Coding Guide before state-ownership decisions. |
| `AS-02-HI-04` | Locate the real component API rather than inventing one. |
