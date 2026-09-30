"""Unit tests for the Goal 03 native harness without launching a UI."""

from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import io
import json
import shutil
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from scripts.markturbo_tools.native import goal03 as HARNESS
from scripts.markturbo_tools.native import runtime

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
VALIDATE_EVIDENCE = HARNESS.validate_evidence
VALIDATE_FINGERPRINT = runtime.validate_fingerprint
GOAL_03_HARNESS = HARNESS.Goal03Harness

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

    def test_native_run_plan_wires_each_required_case_to_its_scenario_in_order(self) -> None:
        harness = object.__new__(GOAL_03_HARNESS)
        plan = HARNESS.native_run_plan()

        scenarios = plan.scenarios(harness)

        self.assertEqual(plan.required_case_ids, REQUIRED_CASE_IDS)
        self.assertEqual(
            tuple(scenario.__func__ for scenario in scenarios),
            (
                GOAL_03_HARNESS.scenario_welcome,
                GOAL_03_HARNESS.scenario_new_paste,
                GOAL_03_HARNESS.scenario_save_create,
                GOAL_03_HARNESS.scenario_save_cancel_overwrite,
                GOAL_03_HARNESS.scenario_sample,
                GOAL_03_HARNESS.scenario_recents,
                GOAL_03_HARNESS.scenario_cli,
            ),
        )
        self.assertTrue(all(scenario.__self__ is harness for scenario in scenarios))

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


class SourceContractFixtureTests(unittest.TestCase):
    WORKSPACE_SOURCE = '''#[cfg(test)]
use crate::test_support::WorkspaceFixture;

fn on_paste_into_new() { cx.read_from_clipboard(); }
fn prompt_save_as_overwrite() {
    PromptButton::ok(i18n::t(i18n::Key::Replace, cx));
}

#[cfg(test)]
mod tests {
    const SOURCE_MARKER_DECOYS: &[&str] = &[
        "fn on_paste_into_new",
        "cx.read_from_clipboard()",
        "fn prompt_save_as_overwrite",
        "PromptButton::ok(i18n::t(i18n::Key::Replace, cx))",
    ];
}
'''
    WELCOME_SOURCE = '''#[cfg(test)]
use crate::test_support::WelcomeFixture;

const WELCOME_AUTOMATION_IDS: &[&str] = &[
    "markturbo-welcome-new",
    "markturbo-welcome-paste",
    "markturbo-welcome-open-file",
    "markturbo-welcome-open-folder",
    "markturbo-welcome-open-sample",
    "markturbo-welcome-dont-show-again",
];

fn show_welcome(initial: Option<()>, show_welcome_on_startup: bool) {
    if initial.is_none() && show_welcome_on_startup {}
}

fn dont_show_welcome_again() {}
fn open_bundled_sample() {}
fn record_recent_target() {}

#[cfg(test)]
mod tests {
    const SOURCE_MARKER_DECOYS: &[&str] = &[
        "markturbo-welcome-new",
        "markturbo-welcome-paste",
        "markturbo-welcome-open-file",
        "markturbo-welcome-open-folder",
        "markturbo-welcome-open-sample",
        "markturbo-welcome-dont-show-again",
        "initial.is_none() && show_welcome_on_startup",
        "fn dont_show_welcome_again",
        "fn open_bundled_sample",
        "fn record_recent_target",
    ];
}
'''
    DOCUMENT_SOURCE = '''#[cfg(test)]
use crate::test_support::DocumentFixture;

const SAVE_AS_AUTOMATION_ID: &str = "markturbo-document-save-as";
fn dispatch_save_as() { DocumentEvent::SaveAsRequested; }

#[cfg(test)]
mod tests {
    const SOURCE_MARKER_DECOYS: &[&str] = &[
        "markturbo-document-save-as",
        "DocumentEvent::SaveAsRequested",
    ];
}
'''

    @staticmethod
    def write_repository(
        root: Path, workspace: str, welcome: str | None, document: str
    ) -> None:
        workspace_path = root / "crates" / "mt-app" / "src" / "views" / "workspace.rs"
        welcome_path = workspace_path.parent / "workspace" / "welcome.rs"
        document_path = root / "crates" / "mt-app" / "src" / "views" / "document.rs"
        workspace_path.parent.mkdir(parents=True, exist_ok=True)
        welcome_path.parent.mkdir(parents=True, exist_ok=True)
        document_path.parent.mkdir(parents=True, exist_ok=True)
        workspace_path.write_text(workspace, encoding="utf-8")
        if welcome is not None:
            welcome_path.write_text(welcome, encoding="utf-8")
        document_path.write_text(document, encoding="utf-8")

    @classmethod
    def source_failure(
        cls, root: Path, workspace: str, document: str, *, welcome: str | None = None
    ) -> str | None:
        cls.write_repository(
            root,
            workspace,
            cls.WELCOME_SOURCE if welcome is None else welcome,
            document,
        )
        with mock.patch.object(HARNESS, "REPO", root):
            return HARNESS.source_contract_failure()

    def test_complete_temporary_repository_satisfies_source_contract(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            self.assertIsNone(
                self.source_failure(
                    Path(temporary), self.WORKSPACE_SOURCE, self.DOCUMENT_SOURCE
                )
            )

    def test_real_repository_satisfies_source_contract(self) -> None:
        self.assertIsNone(HARNESS.source_contract_failure())

    def test_alternate_test_modules_cannot_supply_production_contracts(self) -> None:
        cases = (
            ("workspace", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("welcome", "WELCOME_UIA_CONTRACT_MISSING"),
            ("document", "SAVE_AS_UIA_CONTRACT_MISSING"),
        )
        for source_name, expected in cases:
            with self.subTest(source=source_name), tempfile.TemporaryDirectory() as temporary:
                sources = {
                    "workspace": self.WORKSPACE_SOURCE,
                    "welcome": self.WELCOME_SOURCE,
                    "document": self.DOCUMENT_SOURCE,
                }
                sources[source_name] = (
                    "#[cfg(test)] mod checks {\n"
                    + sources[source_name]
                    + "\n}\nfn production_after_tests() {}\n"
                )
                self.assertEqual(
                    self.source_failure(
                        Path(temporary), sources["workspace"], sources["document"],
                        welcome=sources["welcome"],
                    ),
                    expected,
                )

    def test_unsafe_projection_returns_existing_source_contract_codes(self) -> None:
        for source_name, expected in (
            ("workspace", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("welcome", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("document", "SAVE_AS_SOURCE_CONTRACT_MISSING"),
        ):
            with self.subTest(source=source_name), tempfile.TemporaryDirectory() as temporary:
                sources = {
                    "workspace": self.WORKSPACE_SOURCE,
                    "welcome": self.WELCOME_SOURCE,
                    "document": self.DOCUMENT_SOURCE,
                }
                sources[source_name] += "\n#[cfg(test)] mod checks {\n"
                self.assertEqual(
                    self.source_failure(
                        Path(temporary), sources["workspace"], sources["document"],
                        welcome=sources["welcome"],
                    ),
                    expected,
                )

    def test_missing_production_markers_fail_with_their_contract_codes(self) -> None:
        welcome_markers = (
            ("markturbo-welcome-new", "WELCOME_UIA_CONTRACT_MISSING"),
            ("markturbo-welcome-paste", "WELCOME_UIA_CONTRACT_MISSING"),
            ("markturbo-welcome-open-file", "WELCOME_UIA_CONTRACT_MISSING"),
            ("markturbo-welcome-open-folder", "WELCOME_UIA_CONTRACT_MISSING"),
            ("markturbo-welcome-open-sample", "WELCOME_UIA_CONTRACT_MISSING"),
            ("markturbo-welcome-dont-show-again", "WELCOME_UIA_CONTRACT_MISSING"),
            (
                "initial.is_none() && show_welcome_on_startup",
                "NO_ARGUMENT_WELCOME_CONTRACT_MISSING",
            ),
            ("fn dont_show_welcome_again", "DONT_SHOW_WELCOME_CONTRACT_MISSING"),
            ("fn open_bundled_sample", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("fn record_recent_target", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
        )
        for marker, expected_code in welcome_markers:
            with self.subTest(marker=marker), tempfile.TemporaryDirectory() as temporary:
                welcome = self.WELCOME_SOURCE.replace(marker, "", 1)
                self.assertEqual(
                    self.source_failure(
                        Path(temporary), self.WORKSPACE_SOURCE, self.DOCUMENT_SOURCE,
                        welcome=welcome,
                    ),
                    expected_code,
                )

        workspace_markers = (
            ("fn on_paste_into_new", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("cx.read_from_clipboard()", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            ("fn prompt_save_as_overwrite", "FIRST_USE_SOURCE_CONTRACT_MISSING"),
            (
                "PromptButton::ok(i18n::t(i18n::Key::Replace, cx))",
                "FIRST_USE_SOURCE_CONTRACT_MISSING",
            ),
        )
        for marker, expected_code in workspace_markers:
            with self.subTest(marker=marker), tempfile.TemporaryDirectory() as temporary:
                workspace = self.WORKSPACE_SOURCE.replace(marker, "", 1)
                self.assertEqual(
                    self.source_failure(
                        Path(temporary), workspace, self.DOCUMENT_SOURCE
                    ),
                    expected_code,
                )

        document_markers = (
            ("markturbo-document-save-as", "SAVE_AS_UIA_CONTRACT_MISSING"),
            ("DocumentEvent::SaveAsRequested", "SAVE_AS_SOURCE_CONTRACT_MISSING"),
        )
        for marker, expected_code in document_markers:
            with self.subTest(marker=marker), tempfile.TemporaryDirectory() as temporary:
                document = self.DOCUMENT_SOURCE.replace(marker, "", 1)
                self.assertEqual(
                    self.source_failure(
                        Path(temporary), self.WORKSPACE_SOURCE, document
                    ),
                    expected_code,
                )

    def test_production_comment_cannot_replace_removed_first_use_action(self) -> None:
        workspace = self.WORKSPACE_SOURCE.replace(
            "cx.read_from_clipboard();",
            "// Removed action marker: cx.read_from_clipboard().",
            1,
        )
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(
                self.source_failure(
                    Path(temporary), workspace, self.DOCUMENT_SOURCE
                ),
                "FIRST_USE_SOURCE_CONTRACT_MISSING",
            )

    def test_welcome_comment_cannot_replace_a_production_action(self) -> None:
        welcome = self.WELCOME_SOURCE.replace(
            "fn open_bundled_sample() {}",
            "// Removed action marker: fn open_bundled_sample() {}",
            1,
        )
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(
                self.source_failure(
                    Path(temporary), self.WORKSPACE_SOURCE, self.DOCUMENT_SOURCE,
                    welcome=welcome,
                ),
                "FIRST_USE_SOURCE_CONTRACT_MISSING",
            )

    def test_workspace_literal_cannot_replace_missing_welcome_control(self) -> None:
        workspace = (
            'const WRONG_OWNER: &str = "markturbo-welcome-new";\n'
            + self.WORKSPACE_SOURCE
        )
        welcome = self.WELCOME_SOURCE.replace('"markturbo-welcome-new"', "", 1)
        with tempfile.TemporaryDirectory() as temporary:
            self.assertEqual(
                self.source_failure(
                    Path(temporary), workspace, self.DOCUMENT_SOURCE, welcome=welcome
                ),
                "WELCOME_UIA_CONTRACT_MISSING",
            )

    def test_missing_welcome_module_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.write_repository(root, self.WORKSPACE_SOURCE, None, self.DOCUMENT_SOURCE)
            with mock.patch.object(HARNESS, "REPO", root):
                self.assertEqual(
                    HARNESS.source_contract_failure(),
                    "FIRST_USE_SOURCE_CONTRACT_MISSING",
                )


class RecentControlBehaviorTests(unittest.TestCase):
    @staticmethod
    def harness(elements: list[object], query_error: BaseException | None = None) -> object:
        class ElementArray:
            def __init__(self, values: list[object]) -> None:
                self.values = values
                self.Length = len(values)

            def GetElement(self, index: int) -> object:
                return self.values[index]

        class RootElement:
            def FindAll(self, _scope: object, _condition: object) -> ElementArray:
                if query_error is not None:
                    raise query_error
                return ElementArray(elements)

        class FakeIUIA:
            @staticmethod
            def CreateTrueCondition() -> object:
                return object()

        class FakeUIA:
            iuia = FakeIUIA()
            tree_scope = {"descendants": object()}
            known_control_types = {"Button": 1}

        class ElementInfo:
            def __init__(self, element: object) -> None:
                self.automation_id = element.CurrentAutomationId
                self.control_type = "Button"

        elements_by_id = {element.CurrentAutomationId: element for element in elements}

        class Control:
            def __init__(self, element_info: ElementInfo) -> None:
                self.element_info = element_info
                self.element = elements_by_id[element_info.automation_id]

            def is_visible(self) -> bool:
                return self.element.visible

        harness = object.__new__(HARNESS.Goal03Harness)
        harness.iuia_class = FakeUIA
        harness.fresh_uia_root = lambda _hwnd: type(
            "Root", (), {"element": RootElement()}
        )()
        harness.uia_wrapper_class = Control
        harness.uia_element_info_class = ElementInfo
        return harness

    @staticmethod
    def element(automation_id: str, control_type: int, *, visible: bool = True) -> object:
        return type(
            "Element",
            (),
            {
                "CurrentAutomationId": automation_id,
                "CurrentControlType": control_type,
                "visible": visible,
            },
        )()

    def test_recent_query_returns_only_recent_entry_buttons(self) -> None:
        elements = [
            self.element("markturbo-welcome-recent-alpha", 1),
            self.element("markturbo-welcome-recent-remove-alpha", 1),
            self.element("markturbo-welcome-recent-status-alpha", 2),
            self.element("unrelated-control", 1),
        ]
        harness = self.harness(elements)

        controls = harness.recent_controls(type("App", (), {"hwnd": 42})())

        self.assertEqual(
            [control.element_info.automation_id for control in controls],
            ["markturbo-welcome-recent-alpha"],
        )

    def test_recent_query_preserves_uia_contract_failures(self) -> None:
        failure = HARNESS_FAILURE("RECENT_UIA_CONTRACT_MISMATCH")
        harness = self.harness([], failure)

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.recent_controls(type("App", (), {"hwnd": 42})())

        self.assertIs(raised.exception, failure)


class RuntimeArtifactPrivacyTests(unittest.TestCase):
    def test_artifact_read_errors_keep_the_goal_runtime_scan_code(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_bytes(b"startup")
            (root / "config" / "settings.toml").write_bytes(b"settings")
            with (
                mock.patch.object(
                    HARNESS,
                    "artifact_contains",
                    side_effect=PermissionError("document content"),
                ),
                self.assertRaises(HARNESS_FAILURE) as raised,
            ):
                SCAN_CASE_ARTIFACTS(root)

        self.assertEqual(raised.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
        self.assertEqual(raised.exception.detail, "PermissionError")

    def test_runtime_scan_rejects_document_text_in_data_or_config_but_not_workspace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "data" / "logs").mkdir(parents=True)
            (root / "config").mkdir()
            (root / "workspace").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_text("startup", encoding="utf-8")
            (root / "config" / "settings.toml").write_text(
                "show_welcome_on_startup = true", encoding="utf-8"
            )
            (root / "workspace" / "saved.md").write_text(DOCUMENT_SENTINEL, encoding="utf-8")
            scan = SCAN_CASE_ARTIFACTS(root)
            self.assertTrue(scan["utf8_sentinel_absent"])

            (root / "data" / "leak.log").write_text(DOCUMENT_SENTINEL, encoding="utf-8")
            with self.assertRaises(HARNESS_FAILURE) as raised:
                SCAN_CASE_ARTIFACTS(root)
            self.assertEqual(raised.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")

    def test_runtime_scan_streams_webview_profile_and_detects_boundary_leaks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            profile = root / "data" / "webview2" / "Default" / "Cache"
            profile.mkdir(parents=True)
            (root / "data" / "logs").mkdir()
            (root / "data" / "logs" / "markturbo.log").write_bytes(b"startup")
            split = 1024 * 1024 - 3
            (profile / "cache.bin").write_bytes(
                b"x" * split + DOCUMENT_SENTINEL.encode("utf-8")
            )

            with self.assertRaises(HARNESS_FAILURE) as raised:
                SCAN_CASE_ARTIFACTS(root)

            self.assertEqual(raised.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")


if __name__ == "__main__":
    unittest.main()
