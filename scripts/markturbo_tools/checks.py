"""Explicit, platform-aware validation commands for ``scripts/mt.py``."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
CARGO_FALLBACK = Path.home() / ".cargo" / "bin" / (
    "cargo.exe" if sys.platform == "win32" else "cargo"
)


class CheckFailure(RuntimeError):
    """A required command could not start or returned a non-zero status."""


# This list is intentionally not discovery-driven. Native acceptance harnesses
# have unit tests here but their real UI scenarios need explicit `mt.py accept`.
TOOLING_TESTS = (
    "scripts.tests.test_checks",
    "scripts.tests.test_cli",
    "scripts.tests.test_icons",
    "scripts.tests.test_workflows",
    "scripts.tests.test_perf_fixtures",
    "scripts.tests.test_recovery_capacity",
    "scripts.tests.test_native_goal02_evidence",
    "scripts.tests.test_native_goal02_uia",
    "scripts.tests.test_native_goal02_runtime",
    "scripts.tests.test_native_goal02_execution",
    "scripts.tests.test_native_goal03",
    "scripts.tests.test_probe",
)


def run(command: Iterable[str], *, cwd: Path = ROOT) -> None:
    args = list(command)
    print("+", subprocess.list2cmdline(args), flush=True)
    try:
        completed = subprocess.run(args, cwd=cwd, check=False)
    except OSError as error:
        raise CheckFailure(f"could not start {args[0]!r}: {error}") from error
    if completed.returncode:
        raise CheckFailure(f"command failed with exit code {completed.returncode}: {args[0]}")


def cargo(*args: str) -> tuple[str, ...]:
    executable = shutil.which("cargo")
    if executable is None and CARGO_FALLBACK.is_file():
        executable = str(CARGO_FALLBACK)
    if executable is None:
        raise CheckFailure("cargo was not found on PATH or under ~/.cargo/bin")
    return (executable, *args)


def run_tooling_tests() -> None:
    run((sys.executable, "-m", "unittest", *TOOLING_TESTS))


def diff_range(
    *, base: str | None = None, head: str | None = None, environment: dict[str, str] | None = None
) -> tuple[str, str] | None:
    """Resolve an explicit CLI range or the CI `BASE_SHA`/`HEAD_SHA` pair."""

    if base is None and head is None:
        environment = os.environ if environment is None else environment
        base = environment.get("BASE_SHA")
        head = environment.get("HEAD_SHA")
    if (base is None) != (head is None):
        raise CheckFailure("--base and --head, or BASE_SHA and HEAD_SHA, must be provided together")
    return None if base is None else (base, head)


def check_diff(*, base: str | None = None, head: str | None = None) -> None:
    """Check local worktree/index whitespace or one explicit CI revision range."""

    revision_range = diff_range(base=base, head=head)
    if revision_range is not None:
        run(("git", "diff", "--check", *revision_range))
        return
    run(("git", "diff", "--check"))
    run(("git", "diff", "--cached", "--check"))


def fast(*, base: str | None = None, head: str | None = None) -> None:
    check_diff(base=base, head=head)
    run_tooling_tests()


def ci(*, base: str | None = None, head: str | None = None) -> None:
    fast(base=base, head=head)
    run(cargo("fmt", "--all", "--", "--check"))
    run(cargo("clippy", "--workspace", "--all-targets", "--locked"))
    run(cargo("test", "--release", "--workspace", "--locked"))


def full(*, base: str | None = None, head: str | None = None) -> None:
    ci(base=base, head=head)
    run(cargo("build", "--release", "--locked", "-p", "mt-app", "--bin", "markturbo"))
    name = "markturbo.exe" if sys.platform == "win32" else "markturbo"
    binary = ROOT / "target" / "release" / name
    if not binary.is_file():
        raise CheckFailure(f"release build completed without {binary}")


CHECKS = {
    "fast": fast,
    "ci": ci,
    "full": full,
}


def run_check(tier: str, *, base: str | None = None, head: str | None = None) -> None:
    """Run one named tier, preserving the optional CI revision range."""

    CHECKS[tier](base=base, head=head)
