"""Command-line interface for MarkTurbo development tooling."""

from __future__ import annotations

import argparse
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

from . import checks


ROOT = Path(__file__).resolve().parents[2]
UTILITY_COMMANDS = {
    "icons": "scripts.markturbo_tools.icons",
    "fixtures": "scripts.markturbo_tools.perf_fixtures",
    "probe": "scripts.markturbo_tools.probe",
    "capacity": "scripts.markturbo_tools.recovery_capacity",
}
ACCEPTANCE_GOALS = {
    "goal-02": "scripts.markturbo_tools.native.goal02",
    "goal-03": "scripts.markturbo_tools.native.goal03",
}
NATIVE_EXIT_CODES = frozenset({0, 1, 2})


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        prog="mt.py",
        description="MarkTurbo development, validation, and native acceptance tooling.",
    )
    subcommands = result.add_subparsers(dest="command", required=True)

    check = subcommands.add_parser("check", help="Run a named validation tier.")
    check.add_argument("tier", choices=tuple(checks.CHECKS))
    check.add_argument("--base", help="optional Git base revision for a CI diff check")
    check.add_argument("--head", help="optional Git head revision for a CI diff check")

    for command in UTILITY_COMMANDS:
        delegated = subcommands.add_parser(command, help=f"Run {command} tooling.")
        delegated.add_argument("args", nargs=argparse.REMAINDER)

    accept = subcommands.add_parser("accept", help="Run a Windows native acceptance harness.")
    accept.add_argument("goal", choices=tuple(ACCEPTANCE_GOALS))
    accept.add_argument("args", nargs=argparse.REMAINDER)
    return result


def forwarded(args: Sequence[str]) -> list[str]:
    return list(args[1:] if args[:1] == ["--"] else args)


def run_module(module: str, args: Sequence[str]) -> int:
    command = [sys.executable, "-m", module, *forwarded(args)]
    print("+", subprocess.list2cmdline(command), flush=True)
    return subprocess.run(command, cwd=ROOT, check=False).returncode


def native_exit_code(returncode: int) -> int:
    """Keep the documented native acceptance PASS/FAIL/BLOCKED contract."""

    if returncode in NATIVE_EXIT_CODES:
        return returncode
    print(f"error: native harness exited unexpectedly with {returncode}", file=sys.stderr)
    return 1


def main(argv: Sequence[str] | None = None) -> int:
    namespace = parser().parse_args(argv)
    if namespace.command == "check":
        try:
            checks.run_check(namespace.tier, base=namespace.base, head=namespace.head)
        except checks.CheckFailure as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0
    if namespace.command == "accept":
        return native_exit_code(run_module(ACCEPTANCE_GOALS[namespace.goal], namespace.args))
    return run_module(UTILITY_COMMANDS[namespace.command], namespace.args)
