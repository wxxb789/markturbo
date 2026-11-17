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
    "evaluation": "scripts.markturbo_tools.evaluation",
    "release": "scripts.markturbo_tools.release",
    "revision-evaluation": "scripts.markturbo_tools.revision_evaluation",
    "visual": "scripts.markturbo_tools.native.visual",
}
ACCEPTANCE_GOALS = {
    "goal-02": "scripts.markturbo_tools.native.goal02",
    "goal-03": "scripts.markturbo_tools.native.goal03",
    "goal-06": "scripts.markturbo_tools.native.goal06",
    "goal-07": "scripts.markturbo_tools.native.goal07",
}
# `accept all` runs the cheapest, most stable goals first, so a late BLOCKED
# result discards as little evidence as possible.
ACCEPT_ALL_ORDER = ("goal-06", "goal-02", "goal-07", "goal-03")
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

    accept = subcommands.add_parser(
        "accept",
        help="Run a Windows native acceptance harness, or every goal in one session with 'all'.",
    )
    accept.add_argument("goal", choices=(*ACCEPTANCE_GOALS, "all"))
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


def accept_all(args: Sequence[str]) -> int:
    """Run every native acceptance goal in one process and one shared session."""
    from .native import evidence, goal02, goal03, goal06, goal07, runtime

    modules = {"goal-02": goal02, "goal-03": goal03, "goal-06": goal06, "goal-07": goal07}
    accept = argparse.ArgumentParser(
        prog="mt.py accept all",
        description=(
            "Run goals " + ", ".join(ACCEPT_ALL_ORDER) + " in that order: one preflight, "
            "one executable hash and copy, one clipboard save; one evidence file per goal."
        ),
    )
    accept.add_argument("--exe", type=Path, default=evidence.DEFAULT_EXE)
    accept.add_argument(
        "--expect-exe-sha256", type=runtime.normalize_expected_hash, required=True
    )
    accept.add_argument(
        "--evidence-dir",
        type=Path,
        default=ROOT / "target" / "native-acceptance",
        help="directory for goal-0N-native-acceptance.json (default: target/native-acceptance)",
    )
    accept.add_argument("--ui-timeout", type=evidence.ui_timeout_seconds, default=15.0)
    accept.add_argument(
        "--keep-going",
        action="store_true",
        help="after a case or goal FAIL, still run the later ones (BLOCKED stops)",
    )
    options = accept.parse_args(forwarded(args))
    runs = []
    for goal in ACCEPT_ALL_ORDER:
        module = modules[goal]
        goal_args = module.parse_args(
            [
                "--exe",
                str(options.exe),
                "--expect-exe-sha256",
                options.expect_exe_sha256,
                "--evidence",
                str(options.evidence_dir / f"{goal}-native-acceptance.json"),
                "--ui-timeout",
                repr(options.ui_timeout),
            ]
        )
        runs.append((goal, goal_args, module.native_run_plan()))
    return runtime.main_native_acceptance_suite(runs, keep_going=options.keep_going)


def main(argv: Sequence[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    if len(raw_argv) == 2 and raw_argv[0] in UTILITY_COMMANDS and raw_argv[1] in {"-h", "--help"}:
        return run_module(UTILITY_COMMANDS[raw_argv[0]], ("--help",))
    namespace = parser().parse_args(argv)
    if namespace.command == "check":
        try:
            checks.run_check(namespace.tier, base=namespace.base, head=namespace.head)
        except checks.CheckFailure as error:
            print(f"error: {error}", file=sys.stderr)
            return 1
        return 0
    if namespace.command == "accept":
        if namespace.goal == "all":
            return native_exit_code(accept_all(namespace.args))
        return native_exit_code(run_module(ACCEPTANCE_GOALS[namespace.goal], namespace.args))
    return run_module(UTILITY_COMMANDS[namespace.command], namespace.args)
