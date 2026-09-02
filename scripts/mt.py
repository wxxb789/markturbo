#!/usr/bin/env python3
"""The single entry point for MarkTurbo development tooling."""

from __future__ import annotations

import os
import sys

os.environ.setdefault("PYTHONDONTWRITEBYTECODE", "1")
sys.dont_write_bytecode = True

from markturbo_tools.cli import main


if __name__ == "__main__":
    raise SystemExit(main())
