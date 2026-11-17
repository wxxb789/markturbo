"""The shared native evidence schema, artifact scan, credential identity and CLI."""

from __future__ import annotations

import argparse
import copy
import re
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import evidence, goal02, goal03, goal06, goal07, runtime

from ._native_support import (
    HASH,
    case_observations,
    fingerprint,
    passed_evidence,
    process_context,
    runtime_scan,
)

SENTINEL = "MT-TEST-PRIVATE-SENTINEL"


def check_case(case_id: str, observations: dict) -> None:
    if case_id == "one" and observations["editor"]["byte_count"] == 0:
        raise ValueError("empty editor")


CONTRACT = evidence.EvidenceContract(
    schema="markturbo.test-native-acceptance",
    case_flows={"one": "flow one", "two": "flow two"},
    observation_keys=frozenset({"editor", "count", "label", "process_contexts"}),
    fingerprint_keys=frozenset({"editor"}),
    required_observations={"one": frozenset({"editor"}), "two": frozenset()},
    validate_runtime_scan=lambda value: evidence.validate_runtime_scan(value, require_config=True),
    validate_passed_case=check_case,
    private_sentinels=(SENTINEL,),
    safe_strings=frozenset({"flow one", "flow two", "medium"}),
    string_patterns={"label": re.compile(r"[a-z]{3}")},
)


def observations(case_id: str) -> dict:
    flow = CONTRACT.case_flows[case_id]
    if case_id == "one":
        return case_observations(flow, editor=fingerprint(b"text"), count=2)
    return case_observations(flow)


def valid() -> dict:
    return passed_evidence(
        evidence.new_evidence(CONTRACT, HASH),
        observations,
        lambda value, status: runtime.complete_evidence(value, status, CONTRACT.case_ids),
    )


def complete(value: dict, status: str) -> None:
    runtime.complete_evidence(value, status, CONTRACT.case_ids)


class EnvelopeTests(unittest.TestCase):
    def assert_rejected(self, value: dict, message: str) -> None:
        with self.assertRaisesRegex(ValueError, message):
            evidence.validate_evidence(value, CONTRACT)

    def test_accepts_a_complete_hash_bound_pass_and_a_fresh_blocked_record(self) -> None:
        value = valid()
        evidence.validate_evidence(value, CONTRACT)
        self.assertEqual(value["schema_version"], evidence.SCHEMA_VERSION)

        fresh = evidence.new_evidence(CONTRACT, HASH)
        runtime.mark_remaining_cases(fresh, 0, "BLOCKED", "WTS_SESSION_NOT_ACTIVE")
        complete(fresh, "BLOCKED")
        evidence.validate_evidence(fresh, CONTRACT)

    def test_rejects_every_earlier_schema_version(self) -> None:
        for version in (1, 2, True, "3"):
            with self.subTest(version=version):
                value = valid()
                value["schema_version"] = version
                self.assert_rejected(value, "unsupported evidence schema")

    def test_every_object_has_exactly_its_keys(self) -> None:
        mutations = (
            (lambda value: value.update(extra=1), "top-level evidence keys"),
            (lambda value: value["executable"].update(path="x"), "executable keys"),
            (lambda value: value["environment"].update(user="x"), "environment keys"),
            (lambda value: value["cases"][0].update(note=None), "case keys"),
            (lambda value: value["summary"].update(extra=0), "case summary keys"),
            (
                lambda value: value["cases"][0]["observations"]["process_context"].update(pid=1),
                "process context keys",
            ),
            (
                lambda value: value["cases"][0]["observations"]["editor"].update(path="x"),
                "fingerprint keys",
            ),
            (
                lambda value: value["cases"][0]["observations"]["runtime_scan"].update(extra=1),
                "runtime scan keys",
            ),
        )
        for mutate, message in mutations:
            with self.subTest(message=message):
                value = valid()
                mutate(value)
                self.assert_rejected(value, message)

        blocked = evidence.new_evidence(CONTRACT, HASH)
        blocked["environment"] = {"platform": "Windows 11"}
        complete(blocked, "BLOCKED")
        self.assert_rejected(blocked, "environment keys")

    def test_pass_binds_the_verified_original_and_copied_executable(self) -> None:
        mutations = (
            ("sha256", "b" * 64, "does not match expected"),
            ("copied_sha256", "b" * 64, "does not match expected"),
            ("hash_verified", False, "verified executable hashes"),
            ("machine_code", 0x14C, "AMD64 PE32"),
            ("byte_count", 0, "nonempty executable"),
            ("byte_count", True, "invalid executable byte count"),
        )
        for key, replacement, message in mutations:
            with self.subTest(key=key, replacement=replacement):
                value = valid()
                value["executable"][key] = replacement
                self.assert_rejected(value, message)

    def test_case_set_must_be_complete_and_unique(self) -> None:
        value = valid()
        value["cases"].pop()
        complete(value, "PASS")
        self.assert_rejected(value, "required case set is incomplete")

        value = valid()
        value["cases"][1] = copy.deepcopy(value["cases"][0])
        complete(value, "PASS")
        self.assert_rejected(value, "required case set is incomplete")

    def test_case_status_reason_failure_type_and_duration_fail_closed(self) -> None:
        value = valid()
        failed = value["cases"][0]
        failed.update(status="FAIL", reason_code="SOURCE_EDITOR_UIA_TIMEOUT", observations={})
        complete(value, "FAIL")
        evidence.validate_evidence(value, CONTRACT)
        failed["failure_type"] = "HarnessFailure"
        evidence.validate_evidence(value, CONTRACT)

        for key, replacement, message in (
            ("failure_type", "UNIQUE_SECRET", "invalid case failure type"),
            ("reason_code", "free text", "invalid case reason code"),
            ("duration_ms", -0.1, "duration must be nonnegative"),
            ("duration_ms", float("nan"), "duration must be nonnegative"),
        ):
            with self.subTest(key=key, replacement=replacement):
                broken = copy.deepcopy(value)
                broken["cases"][0][key] = replacement
                self.assert_rejected(broken, message)

        not_run = valid()
        not_run["cases"][1].update(status="NOT_RUN", duration_ms=1.0, observations={})
        complete(not_run, "FAIL")
        self.assert_rejected(not_run, "NOT_RUN case cannot have a duration")

        blocked = valid()
        blocked["cases"][1].update(status="BLOCKED", failure_type="HarnessFailure")
        complete(blocked, "BLOCKED")
        self.assert_rejected(blocked, "only failed cases")

    def test_pass_requires_every_case_to_pass_and_a_matching_summary(self) -> None:
        value = valid()
        value["cases"][1].update(status="FAIL", reason_code="X", observations={})
        self.assert_rejected(value, "PASS requires every required case to pass")
        value["status"] = "FAIL"
        self.assert_rejected(value, "case summary does not match")
        complete(value, "FAIL")
        evidence.validate_evidence(value, CONTRACT)

    def test_pass_rejects_contradictory_windows_and_process_evidence(self) -> None:
        mutations = (
            ("windows_build", 21999, "Windows 11 build 22000"),
            ("native_machine_code", 0xAA64, "x64 OS and Python"),
            ("python_pointer_bits", 32, "x64 OS and Python"),
            ("wts_state", "WTSDisconnected", "WTSActive"),
            ("input_desktop", "Winlogon", "unlocked Default input desktop"),
        )
        for key, replacement, message in mutations:
            with self.subTest(key=key):
                value = valid()
                value["environment"][key] = replacement
                self.assert_rejected(value, message)

        value = valid()
        value["cases"][0]["observations"]["process_context"]["session_id"] = 8
        self.assert_rejected(value, "differs from harness context")

        value = valid()
        value["cases"][0]["observations"]["process_contexts"] = [process_context()] * 2
        evidence.validate_evidence(value, CONTRACT)
        value["cases"][0]["observations"]["process_contexts"][1] = {
            **process_context(),
            "integrity_rid": 0x3000,
        }
        self.assert_rejected(value, "differs from harness context")

    def test_observations_are_allowed_keys_with_scalar_or_validated_values(self) -> None:
        for key, replacement, message in (
            ("raw_text", 1, "unknown observation field"),
            ("count", "free text", "free-form observation strings"),
            ("count", {"nested": 1}, "invalid nested observation structure"),
            ("count", [1], "invalid nested observation structure"),
            ("editor", {"byte_count": 1, "sha256": "short"}, "invalid fingerprint SHA-256"),
            ("label", "ABCD", "free-form observation strings"),
        ):
            with self.subTest(key=key, replacement=replacement):
                value = valid()
                value["cases"][0]["observations"][key] = replacement
                self.assert_rejected(value, message)

        value = valid()
        value["cases"][0]["observations"]["label"] = "abc"
        evidence.validate_evidence(value, CONTRACT)

        blocked = evidence.new_evidence(CONTRACT, HASH)
        blocked["cases"][0].update(
            status="BLOCKED",
            reason_code="FOREGROUND_PERMISSION_DENIED",
            duration_ms=1.0,
            observations={key: 0 for key in evidence.FOREGROUND_DIAGNOSTIC_KEYS},
        )
        complete(blocked, "BLOCKED")
        evidence.validate_evidence(blocked, CONTRACT)

    def test_private_content_is_rejected_at_every_nesting_level(self) -> None:
        for mutate in (
            lambda value: value["cases"][0]["observations"].update(label=SENTINEL.lower()),
            lambda value: value["cases"][0].update(reason_code=SENTINEL),
            lambda value: value["cases"][0]["observations"].update(api_key="x"),
            lambda value: value["environment"].update({SENTINEL: 1}),
        ):
            value = valid()
            mutate(value)
            with self.assertRaisesRegex(ValueError, "private content|free-form"):
                evidence.validate_evidence(value, CONTRACT)

    def test_pass_requires_the_case_flow_facts_and_goal_check(self) -> None:
        value = valid()
        value["cases"][0]["observations"]["flow"] = "flow two"
        self.assert_rejected(value, "flow mechanics")

        value = valid()
        value["cases"][0]["observations"]["foreground_verified"] = False
        self.assert_rejected(value, "requires true foreground_verified")

        for key in ("editor", "runtime_scan", "flow"):
            with self.subTest(key=key):
                value = valid()
                del value["cases"][0]["observations"][key]
                self.assert_rejected(value, "passed case evidence is incomplete")

        value = valid()
        value["cases"][0]["observations"]["editor"] = fingerprint(b"")
        self.assert_rejected(value, "empty editor")

    def test_runtime_scan_counts_are_consistent(self) -> None:
        for scan, message in (
            ({**runtime_scan(), "files_scanned": True}, "invalid files_scanned"),
            ({**runtime_scan(), "app_logs_scanned": 0}, "must cover an application log"),
            ({**runtime_scan(), "config_files_scanned": 0}, "must cover a configuration file"),
            ({**runtime_scan(), "files_scanned": 2}, "file count is incomplete"),
        ):
            with self.subTest(message=message):
                value = valid()
                value["cases"][0]["observations"]["runtime_scan"] = scan
                self.assert_rejected(value, message)

    def test_goal_contracts_reject_their_earlier_schema_versions(self) -> None:
        for module in (goal02, goal03, goal06, goal07):
            with self.subTest(goal=module.SCHEMA):
                value = module.new_evidence(HASH)
                runtime.mark_remaining_cases(value, 0, "NOT_RUN", "EXECUTABLE_MISSING")
                runtime.complete_evidence(value, "FAIL", module.REQUIRED_CASE_IDS)
                module.validate_evidence(value)
                for version in (1, 2):
                    value["schema_version"] = version
                    with self.assertRaisesRegex(ValueError, "unsupported evidence schema"):
                        module.validate_evidence(value)


class ArtifactScanTests(unittest.TestCase):
    CODES = {
        **evidence.leak_codes("DOCUMENT_SENTINEL", SENTINEL),
        **evidence.leak_codes("EPHEMERAL_CREDENTIAL", "credential-value"),
    }

    def case_root(self, temporary: str) -> Path:
        root = Path(temporary)
        (root / "data" / "logs").mkdir(parents=True)
        (root / "data" / "logs" / "app.log").write_bytes(b"started\n")
        (root / "config").mkdir()
        (root / "config" / "settings.toml").write_bytes(b"x = 1\n")
        (root / "workspace").mkdir()
        (root / "stderr.log").write_bytes(b"")
        return root

    def test_counts_data_config_and_stderr_but_not_the_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.case_root(temporary)
            (root / "workspace" / "document.md").write_text(SENTINEL, encoding="utf-8")
            self.assertEqual(
                evidence.scan_case_artifacts(root, self.CODES, require_config=True),
                {"files_scanned": 3, "app_logs_scanned": 1, "config_files_scanned": 1},
            )

    def test_each_private_pattern_fails_with_its_own_code(self) -> None:
        for pattern, code in self.CODES.items():
            for target in ("data/logs/app.log", "config/settings.toml", "stderr.log", "data/webview/cache"):
                with self.subTest(code=code, target=target), tempfile.TemporaryDirectory() as temporary:
                    root = self.case_root(temporary)
                    path = root / target
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(b"x" * (1024 * 1024 - 3) + pattern)
                    with self.assertRaises(runtime.HarnessFailure) as raised:
                        evidence.scan_case_artifacts(root, self.CODES)
                    self.assertEqual(raised.exception.code, code)

    def test_missing_or_unreadable_artifacts_fail_closed_without_text(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(runtime.HarnessFailure) as missing:
                evidence.scan_case_artifacts(root, self.CODES)
            self.assertEqual(missing.exception.code, "RUNTIME_ARTIFACTS_MISSING")

            root = self.case_root(temporary)
            (root / "data" / "logs" / "app.log").unlink()
            with self.assertRaises(runtime.HarnessFailure) as no_log:
                evidence.scan_case_artifacts(root, self.CODES)
            self.assertEqual(no_log.exception.code, "APP_LOG_MISSING")

        with tempfile.TemporaryDirectory() as temporary:
            root = self.case_root(temporary)
            (root / "config" / "settings.toml").unlink()
            self.assertEqual(
                evidence.scan_case_artifacts(root, self.CODES)["config_files_scanned"], 0
            )
            with self.assertRaises(runtime.HarnessFailure) as no_config:
                evidence.scan_case_artifacts(root, self.CODES, require_config=True)
            self.assertEqual(no_config.exception.code, "CONFIG_FILE_MISSING")
            (root / "stderr.log").unlink()
            with self.assertRaises(runtime.HarnessFailure) as incomplete:
                evidence.scan_case_artifacts(root, self.CODES)
            self.assertEqual(incomplete.exception.code, "RUNTIME_FILE_COUNT_INCOMPLETE")

        for target, code in (
            ("artifact_contains", "RUNTIME_ARTIFACT_SCAN_FAILED"),
            ("Path.rglob", "RUNTIME_ARTIFACT_ENUM_FAILED"),
        ):
            with self.subTest(code=code), tempfile.TemporaryDirectory() as temporary:
                root = self.case_root(temporary)
                patcher = (
                    mock.patch.object(evidence, "artifact_contains", side_effect=PermissionError(SENTINEL))
                    if target == "artifact_contains"
                    else mock.patch.object(Path, "rglob", side_effect=PermissionError(SENTINEL))
                )
                with patcher, self.assertRaises(runtime.HarnessFailure) as raised:
                    evidence.scan_case_artifacts(root, self.CODES)
                self.assertEqual(raised.exception.code, code)
                self.assertEqual(raised.exception.detail, "PermissionError")


class CredentialTargetTests(unittest.TestCase):
    def test_mirrors_the_rust_endpoint_identity_for_the_given_wire(self) -> None:
        self.assertEqual(
            evidence.credential_target("http://127.0.0.1:4141/v1/", "openai-responses"),
            "io.github.wxxb789.markturbo:model-credential:v2|wire=openai-responses|"
            "host=127.0.0.1|identity-sha256=76d1c8cb04c37287006720b64f66be02bb85723be756cc668cbc0acc13bb1eb4",
        )
        base = evidence.credential_target("http://127.0.0.1:4141/v1/", "openai-chat")
        self.assertTrue(base.startswith("io.github.wxxb789.markturbo:model-credential:v2|wire=openai-chat|"))
        for other in (
            evidence.credential_target("http://127.0.0.1:4141/v1/", "openai-responses"),
            evidence.credential_target("http://127.0.0.1:4142/v1/", "openai-chat"),
            evidence.credential_target("http://127.0.0.1:4141/other/", "openai-chat"),
        ):
            self.assertNotEqual(base, other)
        self.assertEqual(
            evidence.credential_target("HTTPS://Example.invalid/v1", "openai-chat"),
            evidence.credential_target("https://example.invalid:443/v1/", "openai-chat"),
        )

    def test_rejects_endpoints_without_an_identity(self) -> None:
        for endpoint in ("ftp://host/", "http://user:secret@host/", "http://host/?q=1", "http://host:99999/"):
            with self.subTest(endpoint=endpoint), self.assertRaises(ValueError):
                evidence.credential_target(endpoint, "openai-chat")


class ParserTests(unittest.TestCase):
    def test_every_goal_takes_the_shared_arguments(self) -> None:
        for module in (goal02, goal03, goal06, goal07):
            with self.subTest(goal=module.SCHEMA):
                goal = module.SCHEMA.removeprefix("markturbo.").removesuffix("-native-acceptance")
                args = module.parse_args(["--expect-exe-sha256", HASH.upper()])
                self.assertEqual(args.expect_exe_sha256, HASH)
                self.assertEqual(args.ui_timeout, 15.0)
                self.assertEqual(
                    args.evidence,
                    evidence.REPO
                    / "target"
                    / "native-acceptance"
                    / f"{goal}-native-acceptance-v{evidence.SCHEMA_VERSION}.json",
                )
                case = module.REQUIRED_CASE_IDS[-1]
                args = module.parse_args(
                    ["--expect-exe-sha256", HASH, "--case", case, "--keep-workdir-on-failure", "--ui-timeout", "1.5"]
                )
                self.assertEqual((args.case, args.keep_workdir_on_failure, args.ui_timeout), (case, True, 1.5))
                for argv in (
                    [],
                    ["--expect-exe-sha256", "not-a-hash"],
                    ["--expect-exe-sha256", HASH, "--ui-timeout", "0"],
                    ["--expect-exe-sha256", HASH, "--ui-timeout", "nan"],
                    ["--expect-exe-sha256", HASH, "--case", "unknown"],
                ):
                    with self.assertRaises(SystemExit), mock.patch("sys.stderr"):
                        module.parse_args(argv)
        with self.assertRaises(argparse.ArgumentTypeError):
            runtime.normalize_expected_hash("not-a-hash")


if __name__ == "__main__":
    unittest.main()
