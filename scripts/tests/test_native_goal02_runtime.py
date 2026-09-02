"""Goal 02 native harness tests without launching a UI."""

from ._native_goal02_support import *


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

    def test_cleanup_failure_is_a_product_failure(self) -> None:
        class BadProcess:
            def poll(self) -> None:
                return None

            def kill(self) -> None:
                raise OSError("cannot kill")

        harness = object.__new__(NATIVE_HARNESS)
        harness.processes = [BadProcess()]

        with self.assertRaises(HARNESS_FAILURE) as raised:
            harness.cleanup()

        self.assertEqual(raised.exception.code, "CLEANUP_REAP_FAILED")
