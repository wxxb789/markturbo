# Agent instructions

This file exists so you can see how markturbo treats agent artifacts. Open it
and look at the toolbar: it is labelled **Agent Instructions**, not "Markdown".

That labelling is deliberate. `AGENTS.md`, `CLAUDE.md`, `SKILL.md`, and
`.cursor/rules/*` are the source code of human–agent collaboration, and a
workspace for them should know what they are.

## Why this matters later

Recognition is the foundation for a feature markturbo does not have yet:

```text
Effective Agent Context

repo/AGENTS.md
        ↓
src/AGENTS.md
        ↓
current file
```

Resolving which instructions actually apply to a given file requires knowing
which files are instructions in the first place. v0.1 stops at recognition —
the architecture just doesn't prevent the rest.

## Example content

Below is the sort of thing a real `AGENTS.md` holds, so the file is not empty.

### Build

```sh
cargo build --release
```

### Conventions

- Keep changes scoped to what was asked.
- Run the tests before claiming something works.
- Prefer the existing helper over a new one.
