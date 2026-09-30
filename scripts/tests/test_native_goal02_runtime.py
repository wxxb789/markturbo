"""Goal 02 native harness tests without launching a UI."""

from ._native_goal02_support import *

import sys
from types import SimpleNamespace


class OwnedKernel:
    """Kernel boundary fake: termination requests do not themselves prove exit."""

    JOB, PORT, PROCESS, THREAD = 101, 102, 103, 104

    def __init__(self, fail: str = "") -> None:
        self.fail = fail
        self.events: list[str] = []
        self.opened: set[int] = set()
        self.closed: list[int] = []
        self.parent_exited = False
        self.parent_code = 0
        self.descendant_alive = False
        self.completions = iter((6, 7, runtime.JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO))
        self.completion_timeouts: list[int] = []

    def _open(self, stage: str, handle: int) -> int:
        self.events.append(stage)
        if self.fail == stage:
            return 0
        self.opened.add(handle)
        return handle

    def CreateJobObjectW(self, *_args):
        return self._open("job", self.JOB)

    def SetInformationJobObject(self, job, kind, value, size):
        assert job == self.JOB
        stage = "limits" if kind == 9 else "associate"
        self.events.append(stage)
        if kind == 9:
            self.limit_flags = value._obj.BasicLimitInformation.LimitFlags
        else:
            assert kind == 7
            assert value._obj.CompletionPort == self.PORT
            assert value._obj.CompletionKey == runtime.JOB_OBJECT_COMPLETION_KEY
        return self.fail != stage

    def CreateIoCompletionPort(self, *_args):
        return self._open("port", self.PORT)

    def create_suspended_process(self, spec):
        self.events.append("create-suspended")
        if self.fail == "create-suspended":
            raise OSError(5, "synthetic launch failure")
        self.opened.update((self.PROCESS, self.THREAD))
        return self.PROCESS, self.THREAD, 73

    def AssignProcessToJobObject(self, job, process):
        assert (job, process) == (self.JOB, self.PROCESS)
        self.events.append("assign")
        return self.fail != "assign"

    def ResumeThread(self, thread):
        assert thread == self.THREAD
        self.events.append("resume")
        if self.fail == "resume":
            return 0xFFFFFFFF
        self.descendant_alive = True
        return 1

    def post_close(self, hwnd):
        self.events.append("close-window")
        if self.fail != "parent-close":
            self.parent_exited = True

    def WaitForSingleObject(self, handle, timeout):
        assert handle == self.PROCESS, "A job handle is not an exit notification"
        return 0 if self.parent_exited else runtime.WAIT_TIMEOUT

    def GetExitCodeProcess(self, process, code):
        assert process == self.PROCESS
        code._obj.value = self.parent_code
        return True

    def TerminateProcess(self, process, code):
        assert process == self.PROCESS
        self.events.append("terminate-parent")
        self.parent_exited = True
        self.parent_code = code
        return True

    def TerminateJobObject(self, job, code):
        assert job == self.JOB
        self.events.append("terminate-job")
        return self.fail != "terminate-job"

    def GetQueuedCompletionStatus(self, port, message, key, overlapped, timeout):
        assert port == self.PORT
        self.completion_timeouts.append(timeout)
        if self.fail == "completion":
            self.events.append("completion-timeout")
            return False
        message._obj.value = next(self.completions)
        key._obj.value = runtime.JOB_OBJECT_COMPLETION_KEY
        self.events.append(f"completion-{message._obj.value}")
        if message._obj.value == runtime.JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO:
            if not self.parent_exited:
                self.parent_code = 1
            self.parent_exited = True
            self.descendant_alive = False
        return True

    def CloseHandle(self, handle):
        assert handle in self.opened and handle not in self.closed
        self.closed.append(handle)
        if handle == self.JOB:
            self.descendant_alive = False  # Kill-on-close is only a backstop.
        return True


def owned_harness(root, harness_type=runtime.NativeHarness, kernel=None):
    kernel = kernel or OwnedKernel()
    win32 = object.__new__(runtime.Win32)
    win32.kernel32 = kernel
    win32.create_suspended_process = kernel.create_suspended_process
    win32.post_close = kernel.post_close
    win32.security_context = lambda _pid: runtime.SecurityContext(7, 0x2000, "medium")
    win32.require_foreground = mock.Mock()
    application = mock.Mock()
    application.connect.return_value = application
    application.top_window.return_value.handle = 73
    harness = harness_type(
        root / "app.exe", root, 1.0, win32, mock.Mock(return_value=application),
        None, None, None, None, runtime.SecurityContext(7, 0x2000, "medium"),
    )
    return harness, kernel


def launch_owned(harness):
    return harness.launch_app(None, *harness.case_roots("owned"))


class ArtifactContainsTests(unittest.TestCase):
    def test_finds_a_pattern_split_across_the_read_chunk_boundary(self) -> None:
        pattern = b"cross-boundary"
        payload = b"x" * (1024 * 1024 - 5) + pattern
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "artifact.bin"
            path.write_bytes(payload)

            self.assertEqual(runtime.artifact_contains(path, (pattern,)), pattern)

    def test_pattern_tuple_order_wins_over_file_position(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "artifact.bin"
            path.write_bytes(b"second pattern appears before first")

            self.assertEqual(
                runtime.artifact_contains(path, (b"first", b"second")), b"first"
            )


class RuntimeEvidenceTests(unittest.TestCase):
    def test_log_marker_wait_reads_only_bytes_appended_after_the_offset(self) -> None:
        marker = b"DEBUG recovery checkpoint written\n"
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "markturbo.log"
            path.write_bytes(marker)

            def append_then_match(predicate, timeout, code, interval):
                self.assertEqual((timeout, code, interval), (1.0, "TIMEOUT", 0.025))
                self.assertFalse(predicate())
                with path.open("ab") as handle:
                    handle.write(b"DEBUG recovery checkpoint ")
                self.assertFalse(predicate())
                with path.open("ab") as handle:
                    handle.write(b"written\n")
                self.assertTrue(predicate())

            with mock.patch.object(HARNESS, "wait_until", side_effect=append_then_match):
                HARNESS.wait_for_log_marker(
                    path,
                    len(marker),
                    CHECKPOINT_SUCCESS_PRESENT,
                    1.0,
                    "TIMEOUT",
                )

    def test_checkpoint_log_parser_requires_written_marker_after_offset(self) -> None:
        marker = b"DEBUG recovery checkpoint written\n"
        self.assertTrue(CHECKPOINT_SUCCESS_PRESENT(marker))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(b"recovery checkpoint failed; durable=false\n"))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(b"recovery checkpoint written but failed\n"))
        self.assertFalse(CHECKPOINT_SUCCESS_PRESENT(marker + b"other\n", len(marker)))

    def test_startup_log_parser_requires_finished_marker_after_offset(self) -> None:
        marker = b"DEBUG recovery startup finished\n"
        self.assertTrue(RECOVERY_STARTUP_FINISHED_PRESENT(marker))
        self.assertFalse(RECOVERY_STARTUP_FINISHED_PRESENT(b"recovery startup began\n"))
        self.assertFalse(RECOVERY_STARTUP_FINISHED_PRESENT(marker + b"other\n", len(marker)))

    def test_live_recovery_scan_requires_canonical_encrypted_record(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            data = Path(temporary) / "data"
            recovery = data / "recovery"
            recovery.mkdir(parents=True)
            (recovery / "junk.mtrecovery").write_bytes(b"ignored")
            with self.assertRaises(HARNESS_FAILURE) as missing:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(missing.exception.code, "CANONICAL_RECOVERY_RECORD_MISSING")

            canonical = recovery / (("a" * 64) + ".mtrecovery")
            canonical.write_bytes(b"")
            with self.assertRaises(HARNESS_FAILURE) as empty:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(empty.exception.code, "CANONICAL_RECOVERY_RECORD_EMPTY")

            canonical.write_bytes(b"encrypted-record")
            result = LIVE_RECOVERY_SCAN(data)
            self.assertEqual(result["canonical_record_count"], 1)
            self.assertEqual(len(result["canonical_records"]), 1)

            canonical.write_bytes(DOCUMENT_SENTINEL.encode("utf-8"))
            with self.assertRaises(HARNESS_FAILURE) as leaked:
                LIVE_RECOVERY_SCAN(data)
            self.assertEqual(leaked.exception.code, "UTF8_DOCUMENT_SENTINEL_LEAKED")

    def test_runtime_scan_covers_stderr_app_logs_and_recovery_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")
            (recovery / (("a" * 64) + ".mtrecovery")).write_bytes(b"ciphertext")
            (recovery / ".markturbo-recovery.lock").write_bytes(b"lease")

            result = RUNTIME_ARTIFACT_SCAN(data, stderr)

            self.assertEqual(result["files_scanned"], 4)
            self.assertEqual(result["app_logs_scanned"], 1)
            self.assertEqual(result["recovery_artifacts_scanned"], 2)
            self.assertEqual(result["canonical_recovery_records_scanned"], 1)
            self.assertEqual(result["recovery_leases_scanned"], 1)

    def test_runtime_scan_counts_records_and_leases_separately(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")

            (recovery / ".markturbo-recovery.lock").write_bytes(b"lease")
            lease_only = RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(lease_only["canonical_recovery_records_scanned"], 0)
            self.assertEqual(lease_only["recovery_leases_scanned"], 1)

            (recovery / ".markturbo-recovery.lock").unlink()
            (recovery / (("a" * 64) + ".mtrecovery")).write_bytes(b"ciphertext")
            record_only = RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(record_only["canonical_recovery_records_scanned"], 1)
            self.assertEqual(record_only["recovery_leases_scanned"], 0)

    def test_recovery_scans_fail_closed_on_permission_error(self) -> None:
        secret = "UNIQUE-DOCUMENT-CONTENT"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            (logs / "markturbo-1.log").write_bytes(b"startup ok\n")
            record = recovery / (("a" * 64) + ".mtrecovery")
            record.write_bytes(b"ciphertext")
            original_read_bytes = Path.read_bytes

            def denied(path: Path) -> bytes:
                if path == record:
                    raise PermissionError(secret)
                return original_read_bytes(path)

            with mock.patch.object(Path, "read_bytes", new=denied):
                with self.assertRaises(HARNESS_FAILURE) as live:
                    LIVE_RECOVERY_SCAN(data)
            self.assertEqual(live.exception.code, "LIVE_RECOVERY_RECORD_SCAN_FAILED")
            self.assertEqual(live.exception.detail, "PermissionError")

            with mock.patch.object(Path, "read_bytes", new=denied):
                with self.assertRaises(HARNESS_FAILURE) as runtime:
                    RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(runtime.exception.code, "RUNTIME_ARTIFACT_SCAN_FAILED")
            self.assertEqual(runtime.exception.detail, "PermissionError")
            self.assertNotIn(secret, runtime.exception.detail)

    def test_runtime_scan_rejects_utf8_utf16_panic_and_refcell(self) -> None:
        payloads = (
            (DOCUMENT_SENTINEL.encode("utf-8"), "UTF8_DOCUMENT_SENTINEL_LEAKED"),
            (DOCUMENT_SENTINEL.encode("utf-16-le"), "UTF16LE_DOCUMENT_SENTINEL_LEAKED"),
            (b"thread panicked at source", "PANIC_LOGGED"),
            (b"RefCell already borrowed", "REFCELL_BORROW_PANIC_LOGGED"),
        )
        for payload, code in payloads:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                data = root / "data"
                logs = data / "logs"
                logs.mkdir(parents=True)
                stderr = root / "stderr.log"
                stderr.write_bytes(b"")
                (logs / "markturbo-1.log").write_bytes(payload)

                with self.assertRaises(HARNESS_FAILURE) as raised:
                    RUNTIME_ARTIFACT_SCAN(data, stderr)
                self.assertEqual(raised.exception.code, code)

    def test_runtime_scan_requires_app_log_and_scans_recovery_payload(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / "data"
            stderr = root / "stderr.log"
            stderr.write_bytes(b"")
            with self.assertRaises(HARNESS_FAILURE) as missing:
                RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(missing.exception.code, "APP_LOG_MISSING")

            logs = data / "logs"
            recovery = data / "recovery"
            logs.mkdir(parents=True)
            recovery.mkdir()
            (logs / "markturbo-1.log").write_bytes(b"startup ok")
            (recovery / "record.mtrecovery").write_bytes(
                DOCUMENT_SENTINEL.encode("utf-16-le")
            )
            with self.assertRaises(HARNESS_FAILURE) as leaked:
                RUNTIME_ARTIFACT_SCAN(data, stderr)
            self.assertEqual(leaked.exception.code, "UTF16LE_DOCUMENT_SENTINEL_LEAKED")

    def test_cleanup_failure_still_cleans_other_owned_jobs(self) -> None:
        bad = mock.Mock()
        bad.cleanup.side_effect = HARNESS_FAILURE("PROCESS_JOB_QUIESCENCE_TIMEOUT")
        other = mock.Mock()
        harness = object.__new__(NATIVE_HARNESS)
        harness.processes = [bad, other]

        with self.assertRaisesRegex(HARNESS_FAILURE, "PROCESS_JOB_QUIESCENCE_TIMEOUT"):
            harness.cleanup()

        other.cleanup.assert_called_once_with()


class OwnedProcessTests(unittest.TestCase):
    def test_launch_subscribes_and_assigns_before_resuming(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory))
            app = launch_owned(harness)
            self.assertEqual(kernel.events, [
                "job", "limits", "port", "associate", "create-suspended", "assign", "resume",
            ])
            self.assertEqual(kernel.limit_flags, runtime.JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)
            self.assertEqual(kernel.closed, [kernel.THREAD])
            self.assertIsNone(app.process.poll())
            harness.reap(app)
            self.assertFalse(kernel.descendant_alive)
            self.assertEqual(set(kernel.closed), kernel.opened)

    def test_exited_parent_does_not_skip_live_descendant_and_cleanup_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory))
            app = launch_owned(harness)
            kernel.parent_exited = True
            self.assertEqual(app.process.poll(), 0)
            self.assertTrue(kernel.descendant_alive)
            harness.reap(app)
            events = list(kernel.events)
            harness.reap(app)
            harness.cleanup()
            self.assertEqual(kernel.events, events)
            self.assertNotIn("close-window", events)
            self.assertEqual(events[-4:], [
                "terminate-job", "completion-6", "completion-7", "completion-4",
            ])
            self.assertFalse(kernel.descendant_alive)
            self.assertEqual(app.process.returncode, 0)
            self.assertCountEqual(kernel.closed, kernel.opened)
            self.assertTrue(all(0 < value <= 5000 for value in kernel.completion_timeouts))

    def test_parent_exit_code_and_intentional_crash_semantics_are_preserved(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory))
            app = launch_owned(harness)
            harness.win32.terminate_process = lambda _pid: kernel.TerminateProcess(
                kernel.PROCESS, 0xDEAD
            )
            harness.terminate(app)
            self.assertEqual(app.process.returncode, 0xDEAD)
            with self.assertRaisesRegex(runtime.HarnessFailure, "PROCESS_EXIT_NONZERO"):
                harness.wait_process_exit(app)
            harness.reap(app)
            self.assertEqual(app.process.returncode, 0xDEAD)
            self.assertFalse(kernel.descendant_alive)

    def test_hung_parent_is_terminated_only_through_its_job(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory), kernel=OwnedKernel("parent-close"))
            app = launch_owned(harness)
            harness.reap(app)
            self.assertEqual(app.process.returncode, 1)
            self.assertNotIn("terminate-parent", kernel.events)
            self.assertIn("completion-4", kernel.events)
            self.assertCountEqual(kernel.closed, kernel.opened)

    def test_every_startup_failure_closes_acquired_handles_before_ui(self) -> None:
        stages = (
            ("job", "PROCESS_JOB_CREATE_FAILED"),
            ("limits", "PROCESS_JOB_LIMIT_FAILED"),
            ("port", "PROCESS_JOB_PORT_CREATE_FAILED"),
            ("associate", "PROCESS_JOB_PORT_ASSOCIATE_FAILED"),
            ("create-suspended", "PROCESS_LAUNCH_ACCESS_DENIED"),
            ("assign", "PROCESS_JOB_ASSIGN_FAILED"),
            ("resume", "PROCESS_RESUME_FAILED"),
        )
        for stage, code in stages:
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                harness, kernel = owned_harness(Path(directory), kernel=OwnedKernel(stage))
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    launch_owned(harness)
                harness.cleanup()
                self.assertCountEqual(kernel.closed, kernel.opened)
                harness.application_class.assert_not_called()
                if stage == "assign":
                    self.assertNotIn("resume", kernel.events)
                    self.assertIn("terminate-parent", kernel.events)
                if stage == "resume":
                    self.assertIn("completion-4", kernel.events)

    def test_uia_startup_failure_retains_job_for_teardown(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory))
            harness.win32.require_foreground.side_effect = runtime.HarnessBlocked(
                "FOREGROUND_PERMISSION_DENIED"
            )
            with self.assertRaisesRegex(runtime.HarnessBlocked, "FOREGROUND_PERMISSION_DENIED"):
                launch_owned(harness)
            kernel.parent_exited = True
            harness.cleanup()
            self.assertFalse(kernel.descendant_alive)
            self.assertIn("completion-4", kernel.events)
            self.assertCountEqual(kernel.closed, kernel.opened)

    def test_quiescence_errors_close_handles_and_remain_fail_closed_on_retry(self) -> None:
        for stage, code in (
            ("completion", "PROCESS_JOB_QUIESCENCE_TIMEOUT"),
            ("terminate-job", "PROCESS_JOB_TERMINATE_FAILED"),
        ):
            with self.subTest(stage=stage), tempfile.TemporaryDirectory() as directory:
                harness, kernel = owned_harness(Path(directory), kernel=OwnedKernel(stage))
                launch_owned(harness)
                with mock.patch.object(runtime.ctypes, "get_last_error", return_value=258, create=True):
                    with self.assertRaisesRegex(runtime.HarnessFailure, code):
                        harness.cleanup()
                self.assertCountEqual(kernel.closed, kernel.opened)
                events = list(kernel.events)
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    harness.cleanup()
                self.assertEqual(kernel.events, events)
                with self.assertRaisesRegex(runtime.HarnessFailure, code):
                    harness.processes[0].poll()

    def test_interrupted_quiescence_closes_handles_without_allowing_later_success(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            harness, kernel = owned_harness(Path(directory))
            launch_owned(harness)
            kernel.GetQueuedCompletionStatus = mock.Mock(side_effect=KeyboardInterrupt)
            other = mock.Mock()
            harness.processes.append(other)
            with self.assertRaises(KeyboardInterrupt):
                harness.cleanup()
            self.assertCountEqual(kernel.closed, kernel.opened)
            other.cleanup.assert_called_once_with()
            with self.assertRaisesRegex(runtime.HarnessFailure, "CLEANUP_REAP_FAILED"):
                harness.cleanup()

    def test_suspended_native_creation_uses_only_explicit_standard_handles(self) -> None:
        api = mock.Mock()
        api.DUPLICATE_SAME_ACCESS = 2
        api.GetCurrentProcess.return_value = 70
        api.DuplicateHandle.side_effect = [81, 82]
        api.CreateProcess.return_value = (91, 92, 93, 94)
        crt = mock.Mock()
        crt.get_osfhandle.side_effect = [61, 62]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            spec = runtime.LaunchSpec(("app.exe", "argument with spaces"), str(root), {"KEY": "value"}, root / "stderr.log")
            win32 = object.__new__(runtime.Win32)
            with (
                mock.patch.dict(sys.modules, {"_winapi": api, "msvcrt": crt}),
                mock.patch.object(runtime.subprocess, "STARTUPINFO", SimpleNamespace, create=True),
                mock.patch.object(runtime.subprocess, "STARTF_USESTDHANDLES", 256, create=True),
            ):
                self.assertEqual(win32.create_suspended_process(spec), (91, 92, 93))
        args = api.CreateProcess.call_args.args
        self.assertEqual(args[:8], (
            "app.exe", 'app.exe "argument with spaces"', None, None, True,
            runtime.CREATE_SUSPENDED, spec.env, spec.cwd,
        ))
        self.assertEqual(args[8].lpAttributeList, {"handle_list": [81, 82]})
        self.assertEqual((args[8].hStdInput, args[8].hStdOutput, args[8].hStdError), (81, 81, 82))
        self.assertEqual(api.CloseHandle.call_args_list, [mock.call(81), mock.call(82)])

    def test_native_creation_failure_closes_inheritable_standard_handles(self) -> None:
        for duplicate_failure in (False, True):
            with self.subTest(duplicate_failure=duplicate_failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                api = mock.Mock(DUPLICATE_SAME_ACCESS=2)
                api.DuplicateHandle.side_effect = [
                    81, OSError("duplicate failure") if duplicate_failure else 82,
                ]
                api.CreateProcess.side_effect = OSError("creation failure")
                win32 = object.__new__(runtime.Win32)
                spec = runtime.LaunchSpec(("absent.exe",), str(root), {}, root / "stderr.log")
                with (
                    mock.patch.dict(sys.modules, {"_winapi": api, "msvcrt": mock.Mock()}),
                    mock.patch.object(runtime.subprocess, "STARTUPINFO", SimpleNamespace, create=True),
                    mock.patch.object(runtime.subprocess, "STARTF_USESTDHANDLES", 256, create=True),
                    self.assertRaises(OSError),
                ):
                    win32.create_suspended_process(spec)
                expected = [mock.call(81)] if duplicate_failure else [mock.call(81), mock.call(82)]
                self.assertEqual(api.CloseHandle.call_args_list, expected)
