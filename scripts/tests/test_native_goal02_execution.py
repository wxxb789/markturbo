"""Goal 02 native harness tests without launching a UI."""

from ._native_goal02_support import *


class PrivacyAndCliTests(unittest.TestCase):
    def test_loading_parser_does_not_import_pywinauto(self) -> None:
        if not PYWINAUTO_WAS_LOADED:
            self.assertNotIn("pywinauto", sys.modules)

    def test_editor_readback_does_not_use_clipboard_or_setvalue(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        forbidden_apis = (
            "Clipboard",
            "CF_UNICODETEXT",
            "GMEM_",
            "Global",
            "SetValue",
        )
        for forbidden in forbidden_apis:
            with self.subTest(forbidden=forbidden):
                self.assertNotIn(forbidden, source)
                self.assertIn(forbidden, f"legacy call: {forbidden}")

    def test_document_text_never_appears_in_serialized_observation(self) -> None:
        secret = "UNIQUE-SECRET-\u4fdd\u5b58-\U0001f680"
        evidence = valid_evidence()
        evidence["cases"][0]["observations"]["editor"] = FINGERPRINT_TEXT(
            secret
        ).evidence()
        serialized = json.dumps(evidence, ensure_ascii=False)

        self.assertNotIn(secret, serialized)
        self.assertNotIn("UNIQUE-SECRET", serialized)
        self.assertIn(hashlib.sha256(secret.encode("utf-8")).hexdigest(), serialized)

        evidence["cases"][0]["observations"]["raw_text"] = secret
        with self.assertRaisesRegex(ValueError, "unknown observation field"):
            VALIDATE_EVIDENCE(evidence)

    def test_exception_sanitizer_never_returns_exception_text(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"

        result = SAFE_EXCEPTION_NAME(RuntimeError(secret))

        self.assertEqual(result, "RuntimeError")
        self.assertNotIn(secret, result)

    def test_failure_type_prefers_safe_detail_and_rejects_unsafe_detail(self) -> None:
        for name in ("PermissionError", "TypeError", "COMError"):
            with self.subTest(name=name):
                self.assertEqual(
                    SAFE_FAILURE_TYPE(HARNESS_FAILURE("RUNTIME_ARTIFACT_SCAN_FAILED", name)),
                    name,
                )
        for detail in ("UNIQUE_SECRET", "secret text", "secret-text"):
            with self.subTest(detail=detail):
                self.assertEqual(
                    SAFE_FAILURE_TYPE(HARNESS_FAILURE("RUNTIME_ARTIFACT_SCAN_FAILED", detail)),
                    "HarnessFailure",
                )

    def test_constructs_isolated_absolute_launch_without_starting_process(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            exe = root / "bin" / "markturbo.exe"
            target = root / "workspace" / "case.md"
            data = root / "data"
            config = root / "config"
            workspace = root / "workspace"
            stderr = root / "stderr.log"
            with mock.patch("subprocess.Popen") as popen:
                spec = BUILD_LAUNCH_SPEC(
                    exe,
                    target,
                    data,
                    config,
                    workspace,
                    stderr,
                    {
                        "OPENAI_API_KEY": "must-not-propagate",
                        "ANTHROPIC_API_KEY": "must-not-propagate",
                        "PATH": "path",
                    },
                )

            popen.assert_not_called()
            self.assertEqual(spec.args, (str(exe), str(target)))
            self.assertEqual(spec.cwd, str(workspace))
            self.assertEqual(spec.env["MARKTURBO_DATA_DIR"], str(data))
            self.assertEqual(spec.env["MARKTURBO_CONFIG_DIR"], str(config))
            self.assertEqual(spec.env["RUST_LOG"], "debug")
            self.assertNotIn("OPENAI_API_KEY", spec.env)
            self.assertNotIn("ANTHROPIC_API_KEY", spec.env)

    def test_constructs_cli_without_starting_process(self) -> None:
        exe = Path("C:/release/markturbo.exe")
        evidence = Path("C:/evidence/goal-02.json")
        with mock.patch("subprocess.Popen") as popen:
            command = BUILD_CLI_COMMAND(exe, HASH, evidence)
            args = PARSE_ARGS(command[3:])

        popen.assert_not_called()
        self.assertEqual(command[1:3], ["-m", "scripts.markturbo_tools.native.goal02"])
        self.assertEqual(args.exe, exe)
        self.assertEqual(args.expect_exe_sha256, HASH)
        self.assertEqual(args.evidence, evidence)
        self.assertEqual(NORMALIZE_EXPECTED_HASH(HASH.upper()), HASH)

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

            def scenario_pointer_cancel(self):
                return {}

            scenario_keyboard_discard = scenario_pointer_cancel
            scenario_window_save = scenario_pointer_cancel
            scenario_external_conflict = scenario_pointer_cancel
            scenario_recovery = scenario_pointer_cancel

            def cleanup(self):
                pass

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
                    "--case",
                    REQUIRED_CASE_IDS[0],
                ]
            )
            with mock.patch.dict(
                RUN.__globals__,
                {
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal02Harness": FakeHarness,
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

            def scenario_pointer_cancel(self):
                return {}

            scenario_keyboard_discard = scenario_pointer_cancel
            scenario_window_save = scenario_pointer_cancel
            scenario_external_conflict = scenario_pointer_cancel
            scenario_recovery = scenario_pointer_cancel

            def cleanup(self):
                pass

        with tempfile.TemporaryDirectory() as temporary:
            exe = Path(temporary) / "markturbo.exe"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            patch_run = {
                "preflight": lambda *_args: (object(), object()),
                "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                "Goal02Harness": FakeHarness,
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
            def scenario_pointer_cancel(self):
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
                    "preflight": lambda *_args: (object(), object()),
                    "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    "Goal02Harness": FailingHarness,
                },
            ):
                returncode, _, code = RUN(args)

            self.assertEqual((returncode, code), (1, "TEST_CASE_FAILURE"))
            self.assertIsNotNone(args.debug_workdir)
            self.assertTrue(args.debug_workdir.exists())
            shutil.rmtree(args.debug_workdir)

    def test_isolation_copy_failure_is_fail_closed_and_keeps_requested_workdir(self) -> None:
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
            with (
                mock.patch.dict(
                    RUN.__globals__,
                    {
                        "preflight": lambda *_args: (object(), object()),
                        "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    },
                ),
                mock.patch.object(runtime.shutil, "copy2", side_effect=OSError("denied")),
            ):
                returncode, evidence, code = RUN(args)

            self.assertEqual((returncode, evidence["status"], code), (1, "FAIL", "ISOLATION_COPY_FAILED"))
            self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"]))
            self.assertIsNotNone(args.debug_workdir)
            self.assertTrue(args.debug_workdir.exists())
            shutil.rmtree(args.debug_workdir)

    def test_isolation_copy_failure_persists_fail_closed_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            exe = root / "markturbo.exe"
            evidence_path = root / "evidence.json"
            exe.write_bytes(b"test executable")
            expected = hashlib.sha256(exe.read_bytes()).hexdigest()
            with (
                mock.patch.dict(
                    RUN.__globals__,
                    {
                        "preflight": lambda *_args: (object(), object()),
                        "load_pywinauto": lambda: (object(), object(), object(), object(), object()),
                    },
                ),
                mock.patch.object(runtime.shutil, "copy2", side_effect=OSError("denied")),
            ):
                with contextlib.redirect_stderr(io.StringIO()):
                    returncode = HARNESS.main(
                        [
                            "--exe",
                            str(exe),
                            "--expect-exe-sha256",
                            expected,
                            "--evidence",
                            str(evidence_path),
                        ]
                    )

            evidence = json.loads(evidence_path.read_text(encoding="utf-8"))
            self.assertEqual(returncode, 1)
            self.assertEqual(evidence["status"], "FAIL")
            self.assertTrue(all(case["status"] == "NOT_RUN" for case in evidence["cases"]))
