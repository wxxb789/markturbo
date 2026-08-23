---
name: hello-diagrams
description: A valid, fully-populated Agent Skill, shown here so the Skills inspector has something correct to display.
license: Apache-2.0
allowed-tools: Read Write
compatibility: Any agent implementing the Agent Skills specification.
metadata:
  version: "1.0"
  author: markturbo
---

# Hello Diagrams

This is a real Agent Skill: a **directory** whose entry document is `SKILL.md`,
not just a file with a particular name.

Open the **Harness** tab in the sidebar to see its parsed metadata, the discovery
root it was found under, and its supporting directories.

## Structure

```text
hello-diagrams/
├── SKILL.md          ← you are here
├── scripts/          ← executable helpers
└── references/       ← supporting documentation
```

Those subdirectories are conventions from the Agent Skills specification. The
inspector detects the ones that actually exist.

## Steps

1. Read `references/guide.md` for the rules.
2. Run `scripts/render.sh` to produce output.
