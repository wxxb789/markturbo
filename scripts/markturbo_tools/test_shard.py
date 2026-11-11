"""Run one pre-partitioned shard of unittest IDs.

The parent validation runner writes one sorted test ID per line and starts one
process per shard. Keeping sharding in the parent makes the work distribution
deterministic and lets every worker import only the modules its tests need.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path


def main(argv: list[str]) -> int:
    if len(argv) != 1:
        print("usage: python -m scripts.markturbo_tools.test_shard <ids-file>", file=sys.stderr)
        return 2
    ids = [
        line.strip()
        for line in Path(argv[0]).read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    if not ids:
        return 0
    suite = unittest.defaultTestLoader.loadTestsFromNames(ids)
    result = unittest.TextTestRunner(verbosity=1).run(suite)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
