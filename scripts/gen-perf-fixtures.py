#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Regenerate the performance fixtures under fixtures/perf/.

The fixtures are committed, so a fresh clone can run `cargo test` without
running this first. Regenerate only when you want to change their shape — and
note that the performance thresholds in crates/mt-doc/tests/performance.rs were
calibrated against the current output, so changing the generator means
re-checking those numbers.

    uv run scripts/gen-perf-fixtures.py

Determinism matters: the seed is fixed so a regeneration produces identical
bytes, and a regenerated fixture therefore does not show up as a spurious diff.
"""

import pathlib
import random

# Fixed so regeneration is reproducible. The generator does not currently use
# randomness, but the seed is set anyway so adding variation later cannot make
# the fixtures churn.
random.seed(20260820)

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "fixtures" / "perf"


def make(lines: int) -> str:
    """Build a document of roughly `lines` lines.

    Deliberately block-heavy: headings, paragraphs, lists, tables, fences, and
    quotes. That mix is what exercises the parser's per-block cost, which is
    where markdown-rs is superlinear — a single huge paragraph with the same
    byte count would not test the thing that actually matters.
    """
    out: list[str] = []
    section = 0
    while len(out) < lines:
        section += 1
        out += [
            f"## Section {section}",
            "",
            f"Paragraph with **bold**, `code`, and a "
            f"[link](https://example.com/{section}).",
            "",
            "- bullet one",
            "- bullet two",
            "",
            "| col | val |",
            "|---|---|",
            f"| 中文 {section} | {section * 7} |",
            "",
            "```rust",
            f"fn item_{section}() -> usize {{ {section} }}",
            "```",
            "",
            "> A quoted line with unicode: 🎉 日本語",
            "",
        ]
    return "\n".join(out[:lines]) + "\n"


def make_diagram_heavy(count: int = 60) -> str:
    """Many distinct diagrams, so the render cache cannot mask per-block cost."""
    out = ["# Diagram-heavy document", ""]
    for i in range(count):
        out += [
            f"## Diagram {i}",
            "",
            "```mermaid",
            "graph TD;",
            f"  A{i}[Node {i}] --> B{i}[Next];",
            f"  B{i} --> C{i}[Done];",
            "```",
            "",
            "```d2",
            f"a{i} -> b{i}",
            "```",
            "",
            "$$",
            rf"\sum_{{k=1}}^{{{i + 1}}} k^2",
            "$$",
            "",
        ]
    return "\n".join(out) + "\n"


def write(path: pathlib.Path, text: str) -> None:
    # newline="\n" so a Windows run produces the same bytes as a Unix one;
    # .gitattributes marks fixtures binary for the same reason.
    path.write_text(text, encoding="utf-8", newline="\n")
    print(f"  {path.relative_to(ROOT)}  {path.stat().st_size // 1024} KB")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    print("Writing performance fixtures:")
    write(OUT / "large-10k.md", make(10_000))
    write(OUT / "huge-100k.md", make(100_000))
    write(OUT / "diagram-heavy.md", make_diagram_heavy())
    print("\nRe-check the thresholds in crates/mt-doc/tests/performance.rs.")


if __name__ == "__main__":
    main()
