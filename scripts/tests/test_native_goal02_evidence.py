"""Goal 02 native harness tests without launching a UI."""

from ._native_goal02_support import *


class EvidenceSchemaTests(unittest.TestCase):
    def test_accepts_complete_versioned_evidence(self) -> None:
        evidence = valid_evidence()

        VALIDATE_EVIDENCE(evidence)

        self.assertEqual(evidence["schema_version"], 1)
        self.assertEqual([case["id"] for case in evidence["cases"]], list(REQUIRED_CASE_IDS))

    def test_rejects_missing_required_case(self) -> None:
        evidence = valid_evidence()
        evidence["cases"].pop()
        COMPLETE_EVIDENCE(evidence, "PASS")

        with self.assertRaisesRegex(ValueError, "required case set is incomplete"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_duplicate_required_case(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][-1] = copy.deepcopy(evidence["cases"][0])
        COMPLETE_EVIDENCE(evidence, "PASS")

        with self.assertRaisesRegex(ValueError, "duplicate required case ids"):
            VALIDATE_EVIDENCE(evidence)

    def test_rejects_invalid_observation_hash(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor"]["sha256"] = "short"

        with self.assertRaisesRegex(ValueError, "invalid observation SHA-256"):
            VALIDATE_EVIDENCE(evidence)

    def test_failed_case_accepts_legacy_missing_or_safe_exception_type(self) -> None:
        evidence = valid_evidence()
        failed = evidence["cases"][0]
        failed["status"] = "FAIL"
        failed["reason_code"] = "SOURCE_EDITOR_UIA_TIMEOUT"
        failed.pop("failure_type")
        COMPLETE_EVIDENCE(evidence, "FAIL")

        VALIDATE_EVIDENCE(evidence)

        failed["failure_type"] = "HarnessFailure"
        VALIDATE_EVIDENCE(evidence)

        failed["failure_type"] = "UNIQUE_SECRET"
        with self.assertRaisesRegex(ValueError, "invalid case failure type"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_matching_verified_executable_hash(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["sha256"] = "b" * 64

        with self.assertRaisesRegex(ValueError, "does not match expected"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_boolean_executable_byte_count(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["byte_count"] = True

        with self.assertRaisesRegex(ValueError, "invalid executable byte count"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["status"] = "FAIL"
        evidence["executable"]["byte_count"] = True
        COMPLETE_EVIDENCE(evidence, "FAIL")
        with self.assertRaisesRegex(ValueError, "invalid executable byte count"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_environment_and_matching_process_context(self) -> None:
        evidence = valid_evidence()
        evidence["environment"].pop("harness_process")
        with self.assertRaisesRegex(ValueError, "missing process context"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["process_context"]["session_id"] = 2
        with self.assertRaisesRegex(ValueError, "differs from harness context"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_true_case_facts_and_runtime_scan(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][3]["observations"]["watcher_conflict_before_save"] = False
        with self.assertRaisesRegex(ValueError, "requires true watcher_conflict_before_save"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["runtime_scan"][
            "utf8_sentinel_absent"
        ] = False
        with self.assertRaisesRegex(ValueError, "requires true utf8_sentinel_absent"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_signal_over_budget_and_negative_duration(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][4]["observations"]["edit_to_signal_ms"] = 10_000.001
        with self.assertRaisesRegex(ValueError, "exceeds 10000ms"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = -0.1
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        evidence["cases"][2]["duration_ms"] = float("nan")
        with self.assertRaisesRegex(ValueError, "duration must be nonnegative"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_case_specific_hash_mismatch(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["source_after_cancel_save"] = {
            "byte_count": 4,
            "sha256": "b" * 64,
        }

        with self.assertRaisesRegex(ValueError, "Cancel did not preserve exact"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_windows_and_process_evidence(self) -> None:
        mutations = (
            ("windows_build", 21999, "Windows 11 build 22000"),
            ("native_machine_code", 0xAA64, "x64 OS and Python"),
            ("python_pointer_bits", 32, "x64 OS and Python"),
            ("wts_state", "WTSDisconnected", "WTSActive"),
            ("input_desktop", "Winlogon", "unlocked Default input desktop"),
        )
        for key, value, message in mutations:
            with self.subTest(key=key):
                evidence = valid_evidence()
                evidence["environment"][key] = value
                with self.assertRaisesRegex(ValueError, message):
                    VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_pe_evidence(self) -> None:
        evidence = valid_evidence()
        evidence["executable"]["machine_code"] = 0x14C

        with self.assertRaisesRegex(ValueError, "AMD64 PE32"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_rejects_contradictory_flow_mechanics(self) -> None:
        evidence = valid_evidence()
        evidence["cases"][1]["observations"]["flow"] = "WM_CLOSE -> Save"
        with self.assertRaisesRegex(ValueError, "flow mechanics"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["restart_count"] = 1
        with self.assertRaisesRegex(ValueError, "requires two restarts"):
            VALIDATE_EVIDENCE(evidence)

    def test_pass_requires_startup_barrier_and_live_canonical_record(self) -> None:
        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["startup_observed_before_restart_editor"] = False
        with self.assertRaisesRegex(ValueError, "startup_observed_before_restart_editor"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        live = evidence["cases"][4]["observations"]["live_recovery_scan"]
        live["canonical_record_count"] = 0
        live["canonical_records"] = []
        with self.assertRaisesRegex(ValueError, "requires a canonical record"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["canonical_recovery_records_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted canonical recovery records"):
            VALIDATE_EVIDENCE(evidence)

        evidence = valid_evidence()
        recovery = evidence["cases"][4]["observations"]
        recovery["live_runtime_scan"]["recovery_leases_scanned"] = 0
        with self.assertRaisesRegex(ValueError, "omitted recovery lease"):
            VALIDATE_EVIDENCE(evidence)


class PlatformAndExecutableTests(unittest.TestCase):
    def test_accepts_windows_11_x64_only(self) -> None:
        self.assertIsNone(PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 22631, "AMD64", 64))
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 19045, "AMD64", 64),
            "WINDOWS_11_REQUIRED",
        )
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Linux", 6, 0, "x86_64", 64),
            "WINDOWS_REQUIRED",
        )
        self.assertEqual(
            PLATFORM_PREFLIGHT_FAILURE("Windows", 10, 22631, "ARM64", 64),
            "WINDOWS_X64_REQUIRED",
        )

    def test_parses_amd64_pe32_plus(self) -> None:
        result = INSPECT_PE_BYTES(minimal_pe())

        self.assertEqual(result["machine"], "x86_64")
        self.assertEqual(result["format"], "PE32+")

    def test_rejects_non_x64_pe(self) -> None:
        with self.assertRaisesRegex(ValueError, "not AMD64 PE32"):
            INSPECT_PE_BYTES(minimal_pe(machine=0x14C, magic=0x10B))

    def test_hash_comparison_fails_closed(self) -> None:
        self.assertIsNone(EXECUTABLE_HASH_FAILURE(HASH, HASH))
        self.assertEqual(
            EXECUTABLE_HASH_FAILURE("b" * 64, HASH), "EXECUTABLE_HASH_MISMATCH"
        )
        self.assertEqual(EXECUTABLE_HASH_FAILURE("short", HASH), "EXECUTABLE_HASH_INVALID")

    def test_maps_windows_launch_error_codes_without_exception_text(self) -> None:
        self.assertEqual(
            LAUNCH_FAILURE_CODE(OSError(2147942402, "The system cannot find the file specified.")),
            "PROCESS_LAUNCH_FILE_NOT_FOUND",
        )
        self.assertEqual(LAUNCH_FAILURE_CODE(OSError(5, "access denied")), "PROCESS_LAUNCH_ACCESS_DENIED")
        self.assertEqual(LAUNCH_FAILURE_CODE(OSError(999, "secret")), "PROCESS_LAUNCH_FAILED")
