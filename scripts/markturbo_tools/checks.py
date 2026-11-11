"""Explicit, platform-aware validation commands for ``scripts/mt.py``."""

from __future__ import annotations

import concurrent.futures
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from collections.abc import Iterable
from pathlib import Path

from . import privacy


ROOT = Path(__file__).resolve().parents[2]
CARGO_FALLBACK = Path.home() / ".cargo" / "bin" / (
    "cargo.exe" if sys.platform == "win32" else "cargo"
)


class CheckFailure(RuntimeError):
    """A required command could not start or returned a non-zero status."""


# This list is intentionally not discovery-driven. These tests gate ordinary
# product and tooling changes.
TOOLING_TESTS = (
    "scripts.tests.test_checks",
    "scripts.tests.test_privacy",
    "scripts.tests.test_cli",
    "scripts.tests.test_evaluation",
    "scripts.tests.test_revision_evaluation",
    "scripts.tests.test_icons",
    "scripts.tests.test_perf_fixtures",
    "scripts.tests.test_recovery_capacity",
    "scripts.tests.test_probe",
)

# Native harness unit tests describe a Windows automation path that does not run
# on PRs and still targets the retired GPUI surface. Run them only when changing
# the harness, not for every product change.
NATIVE_TOOLING_TESTS = (
    "scripts.tests.test_native_goal02_evidence",
    "scripts.tests.test_native_goal02_uia",
    "scripts.tests.test_native_goal02_runtime",
    "scripts.tests.test_native_goal02_execution",
    "scripts.tests.test_native_goal03",
    "scripts.tests.test_native_goal06",
    "scripts.tests.test_native_goal07",
    "scripts.tests.test_native_goal07_provider",
    "scripts.tests.test_native_source_contract",
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


def test_shards(test_ids: list[str], workers: int) -> list[list[str]]:
    """Partition sorted IDs deterministically, one shard per worker."""

    ordered = sorted(test_ids)
    count = max(1, min(workers, len(ordered)))
    return [ordered[index::count] for index in range(count)]


def run_tooling_suite(modules: Iterable[str]) -> None:
    """Run independent unittest modules in parallel shards.

    The suite is a set of pure, process-isolated tests: modules do not share
    mutable state and each test owns its temporary paths. Splitting the flattened
    IDs keeps one slow module from serializing the whole gate.
    """

    root = str(ROOT)
    if root not in sys.path:
        sys.path.insert(0, root)
    suite = unittest.defaultTestLoader.loadTestsFromNames(tuple(modules))

    def ids(item: unittest.TestSuite | unittest.TestCase) -> Iterable[str]:
        if isinstance(item, unittest.TestSuite):
            for child in item:
                yield from ids(child)
        else:
            yield item.id()

    test_ids = sorted(ids(suite))
    if not test_ids:
        return
    workers = min(8, os.cpu_count() or 1, len(test_ids))
    shards = test_shards(test_ids, workers)
    started = time.perf_counter()
    with tempfile.TemporaryDirectory(prefix="markturbo-tests-") as temporary:
        commands: list[tuple[str, ...]] = []
        for index, shard in enumerate(shards):
            path = Path(temporary) / f"shard-{index}.txt"
            path.write_text("\n".join(shard) + "\n", encoding="utf-8")
            commands.append(
                (sys.executable, "-m", "scripts.markturbo_tools.test_shard", str(path))
            )
        with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
            futures = [
                pool.submit(
                    subprocess.run,
                    command,
                    cwd=ROOT,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    check=False,
                )
                for command in commands
            ]
            results = [future.result() for future in futures]
    failures = [
        (index, result)
        for index, result in enumerate(results)
        if result.returncode != 0
    ]
    elapsed = time.perf_counter() - started
    if failures:
        for index, result in failures:
            print(f"shard {index} failed:", file=sys.stderr)
            if result.stdout:
                print(result.stdout, file=sys.stderr)
            if result.stderr:
                print(result.stderr, file=sys.stderr)
        raise CheckFailure(f"{len(failures)} of {len(shards)} tooling test shards failed")
    print(
        f"tooling tests: {len(test_ids)} passed in {elapsed:.1f}s "
        f"across {len(shards)} shards"
    )


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
    run_tooling_suite(TOOLING_TESTS)


def doc(*, base: str | None = None, head: str | None = None) -> None:
    """Validate the headless core without compiling the desktop app."""

    fast(base=base, head=head)
    run(cargo("fmt", "--all", "--", "--check"))
    run(cargo("test", "--profile", "ci", "-p", "mt-core", "--locked"))


def rust_checks(profile: str) -> None:
    run(cargo("fmt", "--all", "--", "--check"))
    run(cargo("clippy", "--profile", profile, "--workspace", "--all-targets", "--locked"))
    run(cargo("test", "--profile", profile, "--workspace", "--locked"))


def ci(*, base: str | None = None, head: str | None = None) -> None:
    fast(base=base, head=head)
    rust_checks("ci")


def native(*, base: str | None = None, head: str | None = None) -> None:
    """Native-harness unit tests only; not the native acceptance run."""

    check_diff(base=base, head=head)
    run_tooling_suite(NATIVE_TOOLING_TESTS)


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
    "native": native,
    "full": full,
}


def run_check(tier: str, *, base: str | None = None, head: str | None = None) -> None:
    """Run one named tier, preserving the optional CI revision range."""

    CHECKS[tier](base=base, head=head)
