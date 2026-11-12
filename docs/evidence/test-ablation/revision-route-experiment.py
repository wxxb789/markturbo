from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "evaluation" / "goal-01"


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def run(command: str, fixture_root: Path) -> dict[str, object]:
    result = subprocess.run(
        [sys.executable, "scripts/mt.py", command, "verify-manifest", "--root", str(fixture_root)],
        cwd=ROOT,
        check=False,
        capture_output=True,
    )
    return {
        "exit": result.returncode,
        "stdout_sha256": digest(result.stdout),
        "stderr_sha256": digest(result.stderr),
        "stdout_bytes": len(result.stdout),
        "stderr_bytes": len(result.stderr),
    }


def mutate(name: str, corpus: Path) -> None:
    if name == "untouched":
        return
    target = corpus / "CORPUS.md"
    if name in {"changed_snapshot", "recomputed_manifest"}:
        old = target.read_bytes()
        target.write_bytes(old + b"\n")
        if name == "recomputed_manifest":
            manifest = corpus / "MANIFEST.sha256"
            manifest.write_bytes(
                manifest.read_bytes().replace(
                    digest(old).encode("ascii"), digest(target.read_bytes()).encode("ascii"), 1
                )
            )
        return
    if name == "extra_file":
        (corpus / "UNMANIFESTED.txt").write_bytes(b"extra\n")
        return
    raise AssertionError(name)


def main() -> int:
    matrix: dict[str, object] = {}
    with tempfile.TemporaryDirectory(prefix="revision-route-ablation-") as directory:
        temp = Path(directory)
        for name in ("untouched", "changed_snapshot", "recomputed_manifest", "extra_file"):
            fixture_root = temp / name
            corpus = fixture_root / "evaluation" / "goal-01"
            shutil.copytree(SOURCE, corpus)
            mutate(name, corpus)
            canonical = run("evaluation", fixture_root)
            alias = run("revision-evaluation", fixture_root)
            matrix[name] = {
                "canonical": canonical,
                "revision_alias": alias,
                "decision_match": canonical["exit"] == alias["exit"],
                "output_hashes_match": (
                    canonical["stdout_sha256"] == alias["stdout_sha256"]
                    and canonical["stderr_sha256"] == alias["stderr_sha256"]
                ),
            }
    output = ROOT / ".scratch" / "test-ablation" / "revision-route-baseline.json"
    output.write_text(json.dumps(matrix, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(matrix, indent=2, sort_keys=True))
    return 0 if all(
        row["decision_match"] and row["output_hashes_match"] for row in matrix.values()
    ) else 1


if __name__ == "__main__":
    raise SystemExit(main())
