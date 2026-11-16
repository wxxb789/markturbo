---
name: Broken-Example
model: opus
---

# Broken Example

This skill is deliberately non-conformant. Open **Context** (click the file
name at the top of the window): it is still listed under **Skills**, under the
name its frontmatter gives it.

Three things are wrong:

1. `description` is missing — it is required by the specification.
2. `name` is not lowercase.
3. `model` is not a specification field (it is Claude Code specific).

The skill still appears in the list. A broken skill you cannot see is worse than
a broken skill you can fix.
