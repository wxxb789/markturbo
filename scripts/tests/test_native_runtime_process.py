"""The shared native runtime's platform, process and artifact boundaries, against fakes."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from typing import Any
from unittest import mock

from scripts.markturbo_tools.native import runtime

from ._native_support import HASH, minimal_pe


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


class HarnessCleanupTests(unittest.TestCase):
    def test_cleanup_failure_still_cleans_other_owned_jobs(self) -> None:
        bad = mock.Mock()
        bad.cleanup.side_effect = runtime.HarnessFailure("PROCESS_JOB_QUIESCENCE_TIMEOUT")
        other = mock.Mock()
        harness = object.__new__(runtime.NativeHarness)
        harness.processes = [bad, other]

        with self.assertRaisesRegex(runtime.HarnessFailure, "PROCESS_JOB_QUIESCENCE_TIMEOUT"):
            harness.cleanup()

        other.cleanup.assert_called_once_with()


class PlatformAndExecutableTests(unittest.TestCase):
    def test_accepts_windows_11_x64_only(self) -> None:
        self.assertIsNone(runtime.platform_preflight_failure("Windows", 10, 22631, "AMD64", 64))
        self.assertEqual(
            runtime.platform_preflight_failure("Windows", 10, 19045, "AMD64", 64),
            "WINDOWS_11_REQUIRED",
        )
        self.assertEqual(
            runtime.platform_preflight_failure("Linux", 6, 0, "x86_64", 64),
            "WINDOWS_REQUIRED",
        )
        self.assertEqual(
            runtime.platform_preflight_failure("Windows", 10, 22631, "ARM64", 64),
            "WINDOWS_X64_REQUIRED",
        )

    def test_parses_amd64_pe32_plus(self) -> None:
        result = runtime.inspect_pe_bytes(minimal_pe())

        self.assertEqual(result["machine"], "x86_64")
        self.assertEqual(result["format"], "PE32+")

    def test_records_content_free_pe_section_sizes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "test.exe"
            path.write_bytes(minimal_pe())

            self.assertEqual(
                runtime.inspect_pe_sections(path),
                [
                    {
                        "name": ".text",
                        "virtual_size": 123,
                        "raw_size": 512,
                        "characteristics": 0x60000020,
                    }
                ],
            )

    def test_rejects_non_x64_pe(self) -> None:
        with self.assertRaisesRegex(ValueError, "not AMD64 PE32"):
            runtime.inspect_pe_bytes(minimal_pe(machine=0x14C, magic=0x10B))

    def test_hash_comparison_fails_closed(self) -> None:
        self.assertIsNone(runtime.executable_hash_failure(HASH, HASH))
        self.assertEqual(
            runtime.executable_hash_failure("b" * 64, HASH), "EXECUTABLE_HASH_MISMATCH"
        )
        self.assertEqual(runtime.executable_hash_failure("short", HASH), "EXECUTABLE_HASH_INVALID")

    def test_maps_windows_launch_error_codes_without_exception_text(self) -> None:
        self.assertEqual(
            runtime.launch_failure_code(OSError(2147942402, "The system cannot find the file specified.")),
            "PROCESS_LAUNCH_FILE_NOT_FOUND",
        )
        self.assertEqual(runtime.launch_failure_code(OSError(5, "access denied")), "PROCESS_LAUNCH_ACCESS_DENIED")
        self.assertEqual(runtime.launch_failure_code(OSError(999, "secret")), "PROCESS_LAUNCH_FAILED")


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


class ClipboardOpenRetryTests(unittest.TestCase):
    """Another process holding the clipboard for a moment is contention, not an
    unavailable clipboard: the open is retried until a deadline."""

    def fake_clock(self) -> tuple[list[float], Any, Any]:
        now = [0.0]
        return now, (lambda: now[0]), (lambda seconds: now.__setitem__(0, now[0] + seconds))

    def test_a_briefly_held_clipboard_opens_on_a_retry(self) -> None:
        attempts = iter([False, False, True])
        _, clock, sleep = self.fake_clock()
        self.assertEqual(
            runtime.open_clipboard_with_retry(
                lambda: next(attempts), lambda: True, clock=clock, sleep=sleep
            ),
            3,
        )

    def test_a_clipboard_held_past_the_deadline_blocks_with_content_free_diagnostics(self) -> None:
        now, clock, sleep = self.fake_clock()
        calls = []

        def held() -> bool:
            calls.append(now[0])
            return False

        with self.assertRaises(runtime.HarnessBlocked) as raised:
            runtime.open_clipboard_with_retry(held, lambda: True, clock=clock, sleep=sleep)

        self.assertEqual(raised.exception.code, "CLIPBOARD_UNAVAILABLE")
        self.assertEqual(
            raised.exception.diagnostics,
            {"clipboard_open_attempts": len(calls), "clipboard_owner_window_present": True},
        )
        # The whole ~3 s window is retried, not a single attempt.
        self.assertEqual(runtime.CLIPBOARD_OPEN_TIMEOUT_SECONDS, 3.0)
        self.assertGreaterEqual(now[0], runtime.CLIPBOARD_OPEN_TIMEOUT_SECONDS)
        self.assertGreater(len(calls), 100)


class ExecutionStateTests(unittest.TestCase):
    def test_the_run_keeps_the_display_on_and_releases_it(self) -> None:
        calls: list[int] = []
        win32 = object.__new__(runtime.Win32)
        win32.kernel32 = SimpleNamespace(
            SetThreadExecutionState=lambda flags: calls.append(flags) or runtime.ES_CONTINUOUS
        )

        win32.keep_display_awake()
        win32.allow_display_sleep()

        self.assertEqual(
            calls,
            [
                runtime.ES_CONTINUOUS | runtime.ES_DISPLAY_REQUIRED | runtime.ES_SYSTEM_REQUIRED,
                runtime.ES_CONTINUOUS,
            ],
        )

    def test_a_refused_execution_state_blocks(self) -> None:
        win32 = object.__new__(runtime.Win32)
        win32.kernel32 = SimpleNamespace(SetThreadExecutionState=lambda _flags: 0)

        with self.assertRaises(runtime.HarnessBlocked) as raised:
            win32.keep_display_awake()
        self.assertEqual(raised.exception.code, "EXECUTION_STATE_REQUEST_FAILED")
