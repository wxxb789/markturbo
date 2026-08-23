---
name: Broken-Example
model: opus
---

# Broken Example

This skill is deliberately non-conformant, so you can see what validation
reports. Open the **Harness** tab: it is flagged, and the inspector lists every
problem.

Three things are wrong:

1. `description` is missing — it is required by the specification.
2. `name` is not lowercase.
3. `model` is not a specification field (it is Claude Code specific).

The skill still appears in the list. A broken skill you cannot see is worse than
a broken skill you can fix.
