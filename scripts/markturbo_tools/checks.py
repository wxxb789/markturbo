"""Explicit, platform-aware validation commands for ``scripts/mt.py``."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path

from . import privacy


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
    "scripts.tests.test_privacy",
    "scripts.tests.test_cli",
    "scripts.tests.test_evaluation",
    "scripts.tests.test_icons",
    "scripts.tests.test_workflows",
    "scripts.tests.test_perf_fixtures",
    "scripts.tests.test_recovery_capacity",
    "scripts.tests.test_native_goal02_evidence",
    "scripts.tests.test_native_goal02_uia",
    "scripts.tests.test_native_goal02_runtime",
    "scripts.tests.test_native_goal02_execution",
    "scripts.tests.test_native_goal03",
    "scripts.tests.test_native_goal06",
    "scripts.tests.test_probe",
)


def run(command: Iterable[str], *, cwd: Path = ROOT, capture_stdout: bool = False) -> str:
    args = list(command)
    print("+", subprocess.list2cmdline(args), flush=True)
    try:
        completed = subprocess.run(
            args,
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE if capture_stdout else None,
            encoding="utf-8",
        )
    except OSError as error:
        raise CheckFailure(f"could not start {args[0]!r}: {error}") from error
    if completed.returncode:
        raise CheckFailure(f"command failed with exit code {completed.returncode}: {args[0]}")
    return completed.stdout or ""


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


def doc(*, base: str | None = None, head: str | None = None) -> None:
    """Validate the headless document engine without compiling the desktop app."""

    fast(base=base, head=head)
    run(cargo("fmt", "--all", "--", "--check"))
    run(cargo("test", "--profile", "ci", "-p", "mt-doc", "--locked"))


def rust_checks(profile: str) -> None:
    run(cargo("fmt", "--all", "--", "--check"))
    run(cargo("clippy", "--profile", profile, "--workspace", "--all-targets", "--locked"))
    run(cargo("test", "--profile", profile, "--workspace", "--locked"))


def ci(*, base: str | None = None, head: str | None = None) -> None:
    fast(base=base, head=head)
    rust_checks("ci")


def build_release_binary() -> Path:
    """Use Cargo's artifact path, including custom target directories/triples."""

    output = run(
        cargo(
            "build", "--release", "--locked", "-p", "mt-app", "--bin", "markturbo",
            "--message-format=json-render-diagnostics",
        ),
        capture_stdout=True,
    )
    binaries: set[Path] = set()
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            raise CheckFailure("cargo build returned invalid artifact output") from None
        if (
            isinstance(message, dict)
            and message.get("reason") == "compiler-artifact"
            and message.get("target", {}).get("name") == "markturbo"
            and "bin" in message.get("target", {}).get("kind", [])
            and message.get("executable")
        ):
            binaries.add(Path(message["executable"]))
    if len(binaries) != 1:
        raise CheckFailure("cargo build did not report exactly one markturbo executable")
    binary = binaries.pop()
    if not binary.is_file():
        raise CheckFailure(f"release build completed without {binary}")
    return binary


def full(*, base: str | None = None, head: str | None = None) -> None:
    fast(base=base, head=head)
    rust_checks("release")
    binary = build_release_binary()
    try:
        privacy.scan(ROOT, binary)
    except privacy.PrivacyScanError as error:
        raise CheckFailure(str(error)) from None


CHECKS = {
    "fast": fast,
    "doc": doc,
    "ci": ci,
    "full": full,
}


def run_check(tier: str, *, base: str | None = None, head: str | None = None) -> None:
    """Run one named tier, preserving the optional CI revision range."""

    CHECKS[tier](base=base, head=head)
