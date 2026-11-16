# Agent instructions

This file exists so you can see how markturbo treats agent artifacts. Choose
**Review** on it: the result's **Lens** reads **Agent Instructions**, not
"Prompt", because markturbo knows the file by its name.

That recognition is deliberate. `AGENTS.md`, `CLAUDE.md`, `SKILL.md`, and
`.cursor/rules/*` are the source code of human–agent collaboration, and a
workspace for them should know what they are.

## Which instructions apply

Click the file name at the top of the window to open **Context**. It resolves
the chain of instruction files that apply to the open file, the way Codex
reads them:

```text
Effective Agent Context

global AGENTS.md
        ↓
repo/AGENTS.md
        ↓
src/AGENTS.md
        ↓
current file
```

Later files override earlier ones. Instruction files Codex does not apply are
listed separately as **Not applied by Codex**. Tick **Include in review** on a
source to send it with the next Review; nothing is included until you do.

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
