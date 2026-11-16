---
name: hello-diagrams
description: A valid, fully-populated Agent Skill, shown here so the sample workspace has a correct one to list and review.
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

Open **Context** (click the file name at the top of the window) to find it
under **Skills**. Choose **Review** on this file: an Agent Skill is reviewed as
one package, and the consent card lists every file of the package on its own
row before anything is sent.

## Structure

```text
hello-diagrams/
├── SKILL.md          ← you are here
├── scripts/          ← executable helpers
└── references/       ← supporting documentation
```

Those subdirectories are conventions from the Agent Skills specification.
Discovery records the ones that actually exist.

## Steps

1. Read `references/guide.md` for the rules.
2. Run `scripts/render.sh` to produce output.
