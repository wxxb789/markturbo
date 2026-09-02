"""Unit tests for the Goal 03 native harness without launching a UI."""

from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import io
import json
import shutil
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal03 as HARNESS
from scripts.markturbo_tools.native import runtime

SCRIPT = Path(HARNESS.__file__)
PYWINAUTO_WAS_LOADED = "pywinauto" in sys.modules

BUILD_LAUNCH_SPEC = runtime.build_launch_spec
CASE_CLI = HARNESS.CASE_CLI
CASE_NEW_PASTE = HARNESS.CASE_NEW_PASTE
CASE_RECENTS = HARNESS.CASE_RECENTS
CASE_SAMPLE = HARNESS.CASE_SAMPLE
CASE_SAVE_CANCEL_OVERWRITE = HARNESS.CASE_SAVE_CANCEL_OVERWRITE
CASE_SAVE_CREATE = HARNESS.CASE_SAVE_CREATE
CASE_WELCOME = HARNESS.CASE_WELCOME
COMPLETE_EVIDENCE = HARNESS.complete_evidence
DOCUMENT_SENTINEL = HARNESS.DOCUMENT_SENTINEL
HARNESS_FAILURE = runtime.HarnessFailure
HAS_UNICODE_CLIPBOARD_TEXT = HARNESS.has_unicode_clipboard_text
NEW_EVIDENCE = HARNESS.new_evidence
NORMALIZE_EXPECTED_HASH = runtime.normalize_expected_hash
PARSE_ARGS = HARNESS.parse_args
RECENT_SETTINGS_DOCUMENT = HARNESS.recent_settings_document
REQUIRED_CASE_IDS = HARNESS.REQUIRED_CASE_IDS
SCAN_CASE_ARTIFACTS = HARNESS.scan_case_artifacts
SOURCE_CONTRACT_FAILURE = HARNESS.source_contract_failure
VALIDATE_EVIDENCE = HARNESS.validate_evidence
VALIDATE_FINGERPRINT = runtime.validate_fingerprint
GOAL_03_HARNESS = HARNESS.Goal03Harness
RUN = HARNESS.run

HASH = "a" * 64


def fingerprint(value: bytes) -> dict[str, int | str]:
    return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}


def sample_observation() -> dict[str, int | str | dict[str, int | str]]:
    manifest = b"README.md\0"
    content = b"README.md\0sample\n\0"
    content_hash = hashlib.sha256(content).hexdigest()
    return {
        "sample_file_count": 1,
        "sample_manifest": fingerprint(manifest),
        "sample_content": {"byte_count": len(b"sample\n"), "sha256": content_hash},
        "sample_version": content_hash[:24],
    }


def runtime_scan() -> dict[str, int | bool]:
    return {
        "files_scanned": 3,
        "app_logs_scanned": 1,
        "config_files_scanned": 1,
        "utf8_sentinel_absent": True,
        "utf16le_sentinel_absent": True,
    }


def process_context() -> dict[str, int | str]:
    return {"session_id": 1, "integrity_rid": 0x2000, "integrity": "medium"}


def valid_evidence() -> dict:
    evidence = NEW_EVIDENCE(HASH)
    evidence["executable"].update(
        {
            "sha256": HASH,
            "byte_count": 1234,
            "hash_verified": True,
            "copied_sha256": HASH,
            "copy_hash_verified": True,
            "format": "PE32+",
            "machine": "x86_64",
            "machine_code": 0x8664,
            "optional_magic": 0x20B,
        }
    )
    evidence["environment"] = {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": process_context(),
    }
    original = fingerprint(b"original")
    saved = fingerprint(b"saved")
    observations = {
        CASE_WELCOME: {
            "welcome_visible": True,
            "dont_show_visible": True,
            "dont_show_memory_buffer": True,
        },
        CASE_NEW_PASTE: {
            "new_buffer_created": True,
            "paste_buffer_created": True,
            "new_unicode_editor": fingerprint("new \u4e2d\u6587 \U0001f680".encode()),
            "paste_unicode_editor": fingerprint("paste \u65e5\u672c\u8a9e \U0001f9ea".encode()),
        },
        CASE_SAVE_CREATE: {
            "save_as_created": True,
            "saved_destination": saved,
            "reopened_editor": saved,
        },
        CASE_SAVE_CANCEL_OVERWRITE: {
            "editor_before_cancellation": saved,
            "editor_after_save_as_cancel": saved,
            "editor_after_overwrite_cancel": saved,
            "source_before": original,
            "source_after_cancel": original,
            "save_as_cancel_destination_before": original,
            "save_as_cancel_destination_after": original,
            "saved_destination": saved,
            "save_as_cancelled": True,
            "save_as_cancel_focus_preserved": True,
            "overwrite_cancelled": True,
            "overwrite_cancel_focus_preserved": True,
            "overwrite_confirmed": True,
        },
        CASE_SAMPLE: {
            "sample_workspace_opened": True,
            **sample_observation(),
        },
        CASE_RECENTS: {
            "recent_restart_visible": True,
            "recent_count": 10,
            "stale_recent_disabled": True,
        },
        CASE_CLI: {
            "direct_file_bypassed_welcome": True,
            "direct_directory_bypassed_welcome": True,
        },
    }
    for case in evidence["cases"]:
        case["status"] = "PASS"
        case["duration_ms"] = 1.0
        case["observations"] = {
            **observations[case["id"]],
            "flow": HARNESS.CASE_FLOWS[case["id"]],
            "process_context": process_context(),
            "foreground_verified": True,
            "runtime_scan": runtime_scan(),
        }
    COMPLETE_EVIDENCE(evidence, "PASS")
    return evidence


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_hash_bound_evidence(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["status"], "PASS")
        self.assertEqual([case["id"] for case in evidence["cases"]], list(REQUIRED_CASE_IDS))

    def test_accepts_failed_evidence_with_earlier_passed_cases(self) -> None:
        evidence = valid_evidence()
        failed = evidence["cases"][3]
        failed.update(
            status="FAIL",
            reason_code="UI_TIMEOUT",
            failure_type="HarnessFailure",
            observations={},
        )
        for case in evidence["cases"][4:]:
            case.update(
                status="NOT_RUN",
                reason_code="SKIPPED_AFTER_FAILURE",
                failure_type=None,
                duration_ms=None,
                observations={},
            )
        COMPLETE_EVIDENCE(evidence, "FAIL")

        VALIDATE_EVIDENCE(evidence)

    def test_rejects_missing_case_or_duplicate_case(self) -> None:
        evidence = valid_evidence()
        evidence["cases"].pop()
        COMPLETE_EVIDENCE(evidence, "FAIL")
        with self.assertRaisesRegex(ValueError, "required case set is incomplete"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][-1] = copy.deepcopy(evidence["cases"][0])
        COMPLETE_EVIDENCE(evidence, "FAIL")
        with self.assertRaisesRegex(ValueError, "required case set is incomplete"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_matching_original_and_copied_hashes(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["copied_sha256"] = "b" * 64
        with self.assertRaisesRegex(ValueError, "copied executable hash"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["executable"]["hash_verified"] = False
        with self.assertRaisesRegex(ValueError, "verified executable hashes"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_boolean_counts_and_reuses_native_runtime_fingerprint_validation(self) -> None:
        self.assertIs(VALIDATE_FINGERPRINT, runtime.validate_fingerprint)

        with self.assertRaisesRegex(ValueError, "invalid fingerprint byte count"):
            VALIDATE_FINGERPRINT({"byte_count": True, "sha256": HASH})

        evidence = valid_evidence()
        evidence["executable"]["byte_count"] = True
        with self.assertRaisesRegex(ValueError, "nonempty executable"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"]["files_scanned"] = True
        with self.assertRaisesRegex(ValueError, "invalid files_scanned"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_exact_save_and_recent_evidence(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][2]["observations"]["reopened_editor"] = fingerprint(b"different")
        with self.assertRaisesRegex(ValueError, "direct reopen"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["save_as_cancel_destination_after"] = fingerprint(
            b"changed"
        )
        with self.assertRaisesRegex(ValueError, "Save As picker cancellation"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["save_as_cancel_focus_preserved"] = False
        with self.assertRaisesRegex(ValueError, "requires true save_as_cancel_focus_preserved"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["overwrite_cancel_focus_preserved"] = False
        with self.assertRaisesRegex(ValueError, "requires true overwrite_cancel_focus_preserved"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][5]["observations"]["recent_count"] = 11
        with self.assertRaisesRegex(ValueError, "exactly ten"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_the_complete_materialized_sample_inventory(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][4]["observations"].pop("sample_content")
        with self.assertRaisesRegex(ValueError, "passed case evidence is incomplete"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][4]["observations"]["sample_file_count"] = 0
        with self.assertRaisesRegex(ValueError, "sample file inventory must be nonempty"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][4]["observations"]["sample_version"] = "b" * 24
        with self.assertRaisesRegex(ValueError, "sample version does not match"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_text_and_unknown_observations(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["raw_text"] = DOCUMENT_SENTINEL
        with self.assertRaisesRegex(ValueError, "unknown observation field"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["flow"] = "secret document text"
        with self.assertRaisesRegex(ValueError, "free-form observation strings"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_untrusted_failure_and_reason_fields(self) -> None:
        evidence = valid_evidence()
        failed = evidence["cases"][0]
        failed.update(status="FAIL", reason_code="secret text", failure_type="RuntimeError")
        COMPLETE_EVIDENCE(evidence, "FAIL")
        with self.assertRaisesRegex(ValueError, "invalid case reason code"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        failed = evidence["cases"][0]
        failed.update(status="FAIL", reason_code="UI_TIMEOUT", failure_type="SecretFailure")
        COMPLETE_EVIDENCE(evidence, "FAIL")
        with self.assertRaisesRegex(ValueError, "invalid case failure type"):
            VALIDATE_EVIDENCE(evidence)


class ParserAndIsolationTests(unittest.TestCase):
    def test_recent_settings_seed_is_valid_input_without_document_content(self) -> None:
        documents = [Path(f"C:/work/recent-{index:02}.md") for index in range(11)]
        text = RECENT_SETTINGS_DOCUMENT(documents).decode("utf-8")
        parsed = tomllib.loads(text)

        self.assertTrue(parsed["show-welcome-on-startup"])
        self.assertEqual(len(parsed["recent-targets"]), 11)
        self.assertEqual(parsed["recent-targets"][1]["display-name"], "recent-01.md")
        self.assertNotIn(DOCUMENT_SENTINEL, text)

    def test_unicode_text_clipboard_allows_additional_application_formats(self) -> None:
        self.assertTrue(HAS_UNICODE_CLIPBOARD_TEXT({13, 49161, 49282}))
        self.assertFalse(HAS_UNICODE_CLIPBOARD_TEXT({49161, 49282}))

    def test_materialized_sample_inventory_requires_a_nonempty_self_consistent_tree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            template = root / "template"
            template.mkdir()
            (template / "README.md").write_bytes(b"sample\n")
            expected_content_hash = hashlib.sha256(b"README.md\0sample\n\0").hexdigest()
            data = root / "data"
            materialized = data / "sample" / expected_content_hash[:24]
            materialized.parent.mkdir(parents=True)
            shutil.copytree(template, materialized)

            evidence = HARNESS.materialized_sample_inventory(data)

            self.assertEqual(
                set(evidence),
                {"sample_file_count", "sample_manifest", "sample_content", "sample_version"},
            )
            self.assertNotIn("README.md", json.dumps(evidence))
            self.assertEqual(evidence["sample_content"]["sha256"], expected_content_hash)
            self.assertEqual(evidence["sample_version"], expected_content_hash[:24])
            (data / "sample" / ("a" * 24)).mkdir()
            with self.assertRaises(HARNESS_FAILURE) as raised:
                HARNESS.materialized_sample_inventory(data)
            self.assertEqual(raised.exception.code, "SAMPLE_MATERIALIZATION_INCOMPLETE")
            shutil.rmtree(data / "sample" / ("a" * 24))
            (materialized / "README.md").unlink()
            with self.assertRaises(HARNESS_FAILURE) as raised:
                HARNESS.materialized_sample_inventory(data)
            self.assertEqual(raised.exception.code, "SAMPLE_MATERIALIZATION_INCOMPLETE")

    def test_editor_replacement_uses_and_restores_the_text_clipboard(self) -> None:
        events = []

        class FakeWin32:
            def send_shortcut(self, hwnd, key):
                events.append(("shortcut", hwnd, key))

        class FakeHarness:
            ui_timeout = 1.0
            win32 = FakeWin32()

            def read_text_clipboard(self):
                events.append(("read",))
                return "previous text"

            def write_text_clipboard(self, value):
                events.append(("write", value))

            def focus_editor(self, app):
                events.append(("focus", app.hwnd))

            def wait_editor_fingerprint(self, app, expected, timeout, *, already_focused):
                events.append(("wait", timeout, already_focused))
                return expected, 0.0

        app = type("App", (), {"hwnd": 42})()
        result = GOAL_03_HARNESS.replace_editor(FakeHarness(), app, "emoji \U0001f680")

        self.assertEqual(result, runtime.fingerprint_text("emoji \U0001f680"))
        self.assertEqual(events[0], ("read",))
        self.assertEqual(events[1], ("write", "emoji \U0001f680"))
        self.assertIn(("shortcut", 42, runtime.VK_A), events)
        self.assertIn(("shortcut", 42, HARNESS.VK_V), events)
        self.assertEqual(events[-1], ("write", "previous text"))

    def test_source_editor_focus_check_reads_uia_without_refocusing(self) -> None:
        events = []

        class FakeControl:
            def has_keyboard_focus(self):
                events.append(("read_focus",))
                return True

        class FakeHarness:
            def control_by_id(self, hwnd, automation_id, control_type, mismatch_code):
                events.append(("lookup", hwnd, automation_id, control_type, mismatch_code))
                return FakeControl()

        app = type("App", (), {"hwnd": 42})()
        focused = GOAL_03_HARNESS.source_editor_has_focus(FakeHarness(), app)

        self.assertTrue(focused)
        self.assertEqual(events[-1], ("read_focus",))
        self.assertNotIn("click", [event[0] for event in events])

    def test_save_as_shortcut_requires_editor_focus_and_sends_six_key_events(self) -> None:
        events = []

        class FakeWin32:
            def require_foreground(self, hwnd, timeout):
                events.append(("foreground", hwnd, timeout))

            def send_inputs(self, inputs):
                events.append(("inputs", len(inputs)))

        class FakeHarness:
            ui_timeout = 3.0
            win32 = FakeWin32()

            def require_source_editor_focus(self, app, failure_code):
                events.append(("focus", app.hwnd, failure_code))

        app = type("App", (), {"hwnd": 42})()
        GOAL_03_HARNESS.request_save_as_shortcut(FakeHarness(), app)

        self.assertEqual(events[0], ("focus", 42, "SAVE_AS_SHORTCUT_EDITOR_NOT_FOCUSED"))
        self.assertEqual(events[1], ("foreground", 42, 3.0))
        self.assertEqual(events[2], ("inputs", 6))

    def test_close_app_posts_then_waits_for_native_teardown(self) -> None:
        events = []
        window = object()

        class FakeWin32:
            def post_close(self, hwnd):
                events.append(("post_close", hwnd, app.window))

        class FakeHarness:
            win32 = FakeWin32()

            def wait_process_exit(self, value):
                events.append(("wait", value.hwnd))

        app = type("App", (), {"window": window, "hwnd": 42})()
        GOAL_03_HARNESS.close_app(FakeHarness(), app)

        self.assertIs(app.window, window)
        self.assertEqual(
            events,
            [("post_close", 42, window), ("wait", 42)],
        )

    def test_parser_requires_hash_and_positive_timeout(self) -> None:
        args = PARSE_ARGS(["--expect-exe-sha256", HASH.upper(), "--ui-timeout", "1.5"])
        self.assertEqual(args.expect_exe_sha256, HASH)
        self.assertEqual(args.ui_timeout, 1.5)
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            PARSE_ARGS(["--expect-exe-sha256", HASH, "--ui-timeout", "0"])
        with self.assertRaises(argparse.ArgumentTypeError):
            NORMALIZE_EXPECTED_HASH("not-a-hash")

    def test_parser_exposes_debug_case_and_workdir_retention(self) -> None:
        case = REQUIRED_CASE_IDS[0]
        args = PARSE_ARGS(
            [
                "--expect-exe-sha256",
                HASH,
                "--case",
                case,
                "--keep-workdir-on-failure",
            ]
        )
        self.assertEqual(args.case, case)
        self.assertTrue(args.keep_workdir_on_failure)

    def test_single_case_run_never_claims_acceptance_pass(self) -> None:
        class FakeHarness:
            def __init__(self, *_args):
                pass

            def scenario_welcome(self):
                return {}

            scenario_new_paste = scenario_welcome
            scenario_save_create = scenario_welcome
            scenario_save_cancel_overwrite = scenario_welcome
            scenario_sample = scenario_welcome
            scenario_recents = scenario_welcome
            scenario_cli = scenario_welcome

            def cleanup(self):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exe = root / "markturbo.exe"
            exe.write_bytes(b"test executable")
            sample = root / "sample"
            sample.mkdir()
            (sample / "readme.md").write_text("sample", encoding="utf-8")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            args = PARSE_ARGS(
                [
                    "--exe",
                    str(exe),
                    "--expect-exe-sha256",
                    expected,
                    "--case",
                    REQUIRED_CASE_IDS[0],
                ]
            )
            with mock.patch.dict(
                RUN.__globals__,
                {
                    "source_contract_failure": lambda: None,
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal03Harness": FakeHarness,
                    "REPO": root,
                },
            ):
                returncode, evidence, code = RUN(args)

        self.assertEqual((returncode, evidence["status"], code), (1, "FAIL", "PARTIAL_CASE_RUN"))
        self.assertEqual(evidence["cases"][0]["status"], "PASS")
        self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"][1:]))

    def test_run_lifecycle_cleans_success_and_keeps_failure_or_partial_workdirs(self) -> None:
        roots: list[Path] = []

        class FakeHarness:
            def __init__(self, _exe, root, *_args):
                roots.append(root)

            def scenario_welcome(self):
                return {}

            scenario_new_paste = scenario_welcome
            scenario_save_create = scenario_welcome
            scenario_save_cancel_overwrite = scenario_welcome
            scenario_sample = scenario_welcome
            scenario_recents = scenario_welcome
            scenario_cli = scenario_welcome

            def cleanup(self):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exe = root / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            patch_run = {
                "source_contract_failure": lambda: None,
                "preflight": lambda *_args: (object(), object()),
                "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                "Goal03Harness": FakeHarness,
            }

            with mock.patch.dict(RUN.__globals__, patch_run):
                success_args = PARSE_ARGS(["--exe", str(exe), "--expect-exe-sha256", expected])
                success_code, _, _ = RUN(success_args)

                partial_args = PARSE_ARGS(
                    [
                        "--exe",
                        str(exe),
                        "--expect-exe-sha256",
                        expected,
                        "--case",
                        REQUIRED_CASE_IDS[0],
                        "--keep-workdir-on-failure",
                    ]
                )
                partial_code, _, partial_reason = RUN(partial_args)

            self.assertEqual(success_code, 0)
            self.assertFalse(roots[0].exists())
            self.assertEqual((partial_code, partial_reason), (1, "PARTIAL_CASE_RUN"))
            self.assertIsNotNone(partial_args.debug_workdir)
            self.assertTrue(partial_args.debug_workdir.exists())
            shutil.rmtree(partial_args.debug_workdir)

        roots.clear()

        class FailingHarness(FakeHarness):
            def scenario_welcome(self):
                raise HARNESS_FAILURE("TEST_CASE_FAILURE")

        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            args = PARSE_ARGS(
                [
                    "--exe",
                    str(exe),
                    "--expect-exe-sha256",
                    expected,
                    "--keep-workdir-on-failure",
                ]
            )
            with mock.patch.dict(
                RUN.__globals__,
                {
                    "source_contract_failure": lambda: None,
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal03Harness": FailingHarness,
                },
            ):
                returncode, _, code = RUN(args)

            self.assertEqual((returncode, code), (1, "TEST_CASE_FAILURE"))
            self.assertIsNotNone(args.debug_workdir)
            self.assertTrue(args.debug_workdir.exists())
            shutil.rmtree(args.debug_workdir)

    def test_constructs_isolated_no_argument_and_explicit_launches(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            exe = root / "bin" / "markturbo.exe"
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            env = {"PATH": "path", "OPENAI_API_KEY": "secret", "ANTHROPIC_API_KEY": "secret"}
            no_argument = BUILD_LAUNCH_SPEC(exe, None, data, config, workspace, stderr, env)
            explicit = BUILD_LAUNCH_SPEC(exe, workspace / "document.md", data, config, workspace, stderr, env)

        self.assertEqual(no_argument.args, (str(exe),))
        self.assertEqual(explicit.args, (str(exe), str(workspace / "document.md")))
        self.assertEqual(no_argument.env["MARKTURBO_DATA_DIR"], str(data))
        self.assertEqual(no_argument.env["MARKTURBO_CONFIG_DIR"], str(config))
        self.assertNotIn("OPENAI_API_KEY", no_argument.env)
        self.assertNotIn("ANTHROPIC_API_KEY", no_argument.env)

    def test_rejects_relative_isolation_paths(self) -> None:
        with self.assertRaisesRegex(ValueError, "launch paths must be absolute"):
            BUILD_LAUNCH_SPEC(Path("markturbo.exe"), None, Path("data"), Path("config"), Path("workspace"), Path("stderr.log"))


class RuntimeAndSourceContractTests(unittest.TestCase):
    def test_stale_recent_native_check_proves_visible_and_inert_behavior(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def scenario_recents", 1)[1].split(
            "def scenario_cli", 1
        )[0]

        self.assertIn("stale_name = documents[1].name", body)
        self.assertIn("control.element_info.name", body)
        self.assertIn('require_recent_status(restarted, "Missing"', body)
        self.assertIn("if stale.is_enabled():", body)
        self.assertIn('raise HarnessFailure("STALE_RECENT_ENABLED")', body)
        self.assertNotIn("self.click_control(stale", body)
        self.assertIn("self.editor_absent_while_running(restarted)", body)

    def test_save_as_cancellation_fingerprints_the_named_destination_and_preserves_focus(
        self,
    ) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def scenario_save_cancel_overwrite", 1)[1].split(
            "def scenario_sample", 1
        )[0]

        self.assertIn("write_durable(cancelled_destination", body)
        self.assertIn("cancel_save_picker(app, cancelled_destination)", body)
        self.assertIn("sha256_file(cancelled_destination)", body)
        self.assertIn("request_save_as_shortcut(app)", body)
        self.assertIn("require_source_editor_focus", body)
        self.assertIn("already_focused=True", body)
        self.assertIn('"save_as_cancel_focus_preserved": True', body)
        self.assertIn('"overwrite_cancel_focus_preserved": True', body)

    def test_recent_query_preserves_contract_failure_codes(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def recent_controls", 1)[1].split(
            "def require_recent_status", 1
        )[0]

        self.assertIn("except HarnessFailure:\n            raise", body)
        self.assertIn('raise HarnessFailure("RECENT_UIA_QUERY_FAILED"', body)

    def test_recent_query_excludes_status_and_remove_controls(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def recent_controls", 1)[1].split(
            "def require_recent_status", 1
        )[0]

        self.assertIn('"markturbo-welcome-recent-remove-"', body)
        self.assertIn('"markturbo-welcome-recent-status-"', body)

    def test_recent_generation_uses_isolated_settings_before_restart(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def scenario_recents", 1)[1].split(
            "def scenario_cli", 1
        )[0]

        self.assertIn("recent_settings_document(documents)", body)
        self.assertIn("app = self.launch_app(None", body)
        self.assertEqual(body.count("self.launch_app("), 2)

    def test_save_path_waits_for_the_native_dialog_to_release_the_main_window(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def select_save_path", 1)[1].split(
            "def cancel_save_picker", 1
        )[0]

        self.assertIn("SAVE_AS_FILE_DIALOG_CLOSE_TIMEOUT", body)
        self.assertIn("self.win32.require_foreground(app.hwnd, self.ui_timeout)", body)

    def test_welcome_paste_compares_the_clipboard_value_the_app_reads(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def scenario_new_paste", 1)[1].split(
            "def scenario_save_create", 1
        )[0]

        self.assertIn("clipboard_text = self.read_text_clipboard()", body)
        self.assertIn("clipboard_fingerprint = fingerprint_text(clipboard_text)", body)
        self.assertIn("paste_text != clipboard_fingerprint", body)

    def test_save_as_dialog_uses_rooted_uia_controls(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        body = source.split("def common_file_dialog", 1)[1].split(
            "def select_save_path", 1
        )[0]

        self.assertIn('"FileNameControlHost"', body)
        self.assertIn("combo.descendants()", body)
        self.assertIn('automation_id == "1001"', body)
        self.assertNotIn(".child_window(", body)

    def test_existing_save_path_accepts_the_native_confirmation_before_app_replace(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        native = source.split("def accept_native_overwrite_confirmation", 1)[1].split(
            "def select_save_path", 1
        )[0]
        select = source.split("def select_save_path", 1)[1].split(
            "def cancel_save_picker", 1
        )[0]

        self.assertIn('"CommandButton_6"', native)
        self.assertIn("owned_task_dialogs(app.process.pid, file_dialog_hwnd)", native)
        self.assertIn("self.accept_native_overwrite_confirmation(app, hwnd)", select)

    def test_source_contract_matches_current_rust_uia_and_startup_contract(self) -> None:
        self.assertIsNone(SOURCE_CONTRACT_FAILURE())

    def test_source_contract_does_not_accept_strings_from_rust_test_modules(self) -> None:
        workspace_contracts = [
            HARNESS.WELCOME_NEW_AUTOMATION_ID,
            HARNESS.WELCOME_PASTE_AUTOMATION_ID,
            HARNESS.WELCOME_OPEN_FILE_AUTOMATION_ID,
            HARNESS.WELCOME_OPEN_FOLDER_AUTOMATION_ID,
            HARNESS.WELCOME_OPEN_SAMPLE_AUTOMATION_ID,
            HARNESS.WELCOME_DONT_SHOW_AUTOMATION_ID,
            "initial.is_none() && show_welcome_on_startup",
            "fn dont_show_welcome_again",
            "fn on_paste_into_new",
            "cx.read_from_clipboard()",
            "fn open_bundled_sample",
            "fn record_recent_target",
            "fn prompt_save_as_overwrite",
            "PromptButton::ok(i18n::t(i18n::Key::Replace, cx))",
        ]
        document_contracts = [
            HARNESS.DOCUMENT_SAVE_AS_AUTOMATION_ID,
            "DocumentEvent::SaveAsRequested",
        ]

        with tempfile.TemporaryDirectory() as temporary:
            repo = Path(temporary)
            views = repo / "crates" / "mt-app" / "src" / "views"
            views.mkdir(parents=True)
            (views / "workspace.rs").write_text(
                "fn production() {}\n#[cfg(test)]\nmod tests {\n"
                + "\n".join(workspace_contracts)
                + "\n}\n",
                encoding="utf-8",
            )
            (views / "document.rs").write_text(
                "fn production() {}\n#[cfg(test)]\nmod tests {\n"
                + "\n".join(document_contracts)
                + "\n}\n",
                encoding="utf-8",
            )
            with mock.patch.dict(SOURCE_CONTRACT_FAILURE.__globals__, {"REPO": repo}):
                self.assertEqual(
                    SOURCE_CONTRACT_FAILURE(), "WELCOME_UIA_CONTRACT_MISSING"
                )

    def test_runtime_scan_rejects_document_text_in_data_or_config_but_not_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "workspace").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_text("startup", encoding="utf-8")
            (root / "config" / "settings.toml").write_text("show_welcome_on_startup = true", encoding="utf-8")
            (root / "workspace" / "saved.md").write_text(DOCUMENT_SENTINEL, encoding="utf-8")
            scan = SCAN_CASE_ARTIFACTS(root)
            self.assertTrue(scan["utf8_sentinel_absent"])

            (root / "data" / "leak.log").write_text(DOCUMENT_SENTINEL, encoding="utf-8")
            with self.assertRaises(HARNESS_FAILURE) as raised:
                SCAN_CASE_ARTIFACTS(root)
            self.assertEqual(raised.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")

    def test_runtime_scan_streams_the_entire_webview_profile_and_detects_boundary_leaks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            profile = root / "data" / "webview2" / "Default" / "Cache"
            profile.mkdir(parents=True)
            (root / "data" / "logs").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_bytes(b"startup")
            split = 1024 * 1024 - 3
            (profile / "cache.bin").write_bytes(b"x" * split + DOCUMENT_SENTINEL.encode("utf-8"))

            with self.assertRaises(HARNESS_FAILURE) as raised:
                SCAN_CASE_ARTIFACTS(root)

            self.assertEqual(raised.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")

    def test_loading_parser_does_not_import_pywinauto(self) -> None:
        if not PYWINAUTO_WAS_LOADED:
            self.assertNotIn("pywinauto", sys.modules)

    def test_native_source_requires_the_embedded_sample_and_restores_clipboard_after_any_failure(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn('bin_root / "sample"', source)
        self.assertNotIn("SAMPLE_FIXTURE_MISSING", source)

        scenario = source.split("def scenario_new_paste", 1)[1].split("def scenario_save_create", 1)[0]
        guarded_write = scenario.index("try:\n                    self.write_text_clipboard(PASTE_TEXT)")
        restore = scenario.index("finally:\n                    self.write_text_clipboard(clipboard_before)")
        self.assertLess(guarded_write, restore)


if __name__ == "__main__":
    unittest.main()
