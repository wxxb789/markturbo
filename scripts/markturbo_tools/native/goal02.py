"""Exercise Goal 02 destructive paths through a real Windows UI.

The harness is intentionally fail-closed. It emits PASS only after every
required case runs against the expected x64 release executable in an active,
unlocked Windows 11 session. Document text is never written to stdout or JSON;
observations contain only UTF-8 byte counts, SHA-256 digests, and timings.

It drives the nsh UI: the rail's close control, Ctrl+W, Alt+F4 and the native
window close (WM_CLOSE) each must raise the in-window guard card (Save /
Don't save / Cancel) for a dirty document, never close it unasked;
Save runs from the title menu; a same-length external rewrite must be refused
by Save until Keep mine is confirmed; an interrupted dirty buffer must leave a
canonical recovery record the next launch offers to Restore.
"""

from __future__ import annotations

import argparse
import os
import re
import time
from pathlib import Path
from typing import Any

from . import evidence
from .evidence import leak_codes, validate_exact_fingerprint
from .runtime import (
    INTEGRITY_NAMES,
    VK_MENU,
    VK_W,
    CONFLICT_HEADING,
    HarnessFailure,
    NativeHarness as BaseNativeHarness,
    NativeRunPlan,
    RunningApp,
    finite_nonnegative,
    fingerprint_bytes,
    key_input,
    load_pywinauto,
    main_native_acceptance,
    preflight,
    require_true,
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)

SCHEMA = "markturbo.goal-02-native-acceptance"
CASE_POINTER_CANCEL = "pointer-dirty-tab-close-cancel"
CASE_KEYBOARD_DISCARD = "sendinput-ctrl-w-discard"
CASE_WINDOW_SAVE = "wm-close-save-exact-bytes"
CASE_EXTERNAL_CONFLICT = "same-length-mtime-external-conflict"
CASE_RECOVERY = "cjk-emoji-interruption-recovery-retirement"
CASE_FLOWS = {
    CASE_POINTER_CANCEL: "pointer rail close -> guard Cancel -> menu Save",
    CASE_KEYBOARD_DISCARD: "SendInput Ctrl+W -> guard Don't save",
    CASE_WINDOW_SAVE: "SendInput Alt+F4 -> guard Cancel -> WM_CLOSE -> guard Save",
    CASE_EXTERNAL_CONFLICT: (
        "same-length rewrite -> menu Save refused with conflict -> Keep mine -> "
        "explicit Overwrite"
    ),
    CASE_RECOVERY: (
        "edit -> recovery record -> TerminateProcess -> restart -> Restore -> "
        "Ctrl+W -> guard Don't save -> restart without recovery offer"
    ),
}
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)
CHECKPOINT_SIGNAL = "content-free canonical recovery record"
STARTUP_SIGNAL = "document ready without recovery offer"
SAFE_OBSERVATION_STRINGS = (
    set(CASE_FLOWS.values())
    | {"TerminateProcess", CHECKPOINT_SIGNAL, STARTUP_SIGNAL}
    | INTEGRITY_NAMES
)
FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "editor",
        "source_before",
        "source_after_cancel_save",
        "discarded_editor",
        "source_after_discard",
        "editor_after_alt_f4_cancel",
        "source_after_alt_f4_cancel",
        "saved_source",
        "loaded_source",
        "external_source",
        "dirty_editor",
        "source_after_refused_save",
        "editor_after_conflict",
        "saved_after_explicit_overwrite",
        "edited_text",
        "restored_editor",
        "source_after_recovery_discard",
        "restart_editor",
    }
)
REQUIRED_OBSERVATIONS = {
    CASE_POINTER_CANCEL: frozenset(
        {"editor", "source_before", "source_after_cancel_save", "cancel_kept_tab"}
    ),
    CASE_KEYBOARD_DISCARD: frozenset(
        {"discarded_editor", "source_before", "source_after_discard", "discard_closed_tab"}
    ),
    CASE_WINDOW_SAVE: frozenset(
        {
            "editor",
            "alt_f4_cancel_kept_window",
            "editor_after_alt_f4_cancel",
            "source_after_alt_f4_cancel",
            "saved_source",
            "window_exited",
        }
    ),
    CASE_EXTERNAL_CONFLICT: frozenset(
        {
            "loaded_source",
            "external_source",
            "dirty_editor",
            "source_after_refused_save",
            "editor_after_conflict",
            "saved_after_explicit_overwrite",
            "same_length",
            "same_mtime_ns",
            "save_refused_with_conflict",
            "explicit_overwrite_used",
        }
    ),
    CASE_RECOVERY: frozenset(
        {
            "edited_text",
            "checkpoint_record_seen",
            "checkpoint_signal",
            "live_recovery_scan",
            "live_runtime_scan",
            "edit_to_signal_ms",
            "recovery_offer_seen",
            "restored_editor",
            "recovery_restored_exact",
            "restore_ms",
            "discard_restart_absent_ms",
            "source_after_recovery_discard",
            "restart_editor",
            "discarded_recovery_absent",
            "termination",
            "restart_count",
            "startup_signal",
            "startup_observed_before_restart_editor",
            "process_contexts",
        }
    ),
}
RECOVERY_SCAN_KEYS = frozenset(
    {
        "recovery_artifacts_scanned",
        "canonical_recovery_records_scanned",
        "recovery_leases_scanned",
    }
)
RECOVERY_LEASE_NAME = ".markturbo-recovery.lock"
PANIC_RE = re.compile(rb"\bpanic(?:ked)?\b")

# The rail's per-tab close control (`ui/rail.rs`): a Button with its own test
# id, clipped until the pointer widens the rail over a file row.
RAIL_FILE_ROW_AUTOMATION_ID = "rail-file-row"
TAB_CLOSE_AUTOMATION_ID = "rail-close"
# The conflict and recovery actions (`ui/notices.rs`), each a Button with its
# own id and its label as Name. The one recovered record is row 0. Keep mine's
# confirmation is an nsh system alert outside the UIA tree, answered with
# Enter (`runtime.py` `confirm_system_alert`).
KEEP_MINE_AUTOMATION_ID = "conflict-keep-mine"
RESTORE_AUTOMATION_ID = "recovery-restore-0"
RESTORE_NAME = "Restore"
VK_F4 = 0x73
DOCUMENT_SENTINEL = "MTG02-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
CANONICAL_RECOVERY_RECORD_RE = re.compile(r"^[0-9a-f]{64}\.mtrecovery$")

ORIGINAL_BYTES = b"native acceptance original"
EXTERNAL_BYTES = b"native acceptance external"
POINTER_EDIT = f"{DOCUMENT_SENTINEL}:pointer"
KEYBOARD_EDIT = f"{DOCUMENT_SENTINEL}:keyboard"
WINDOW_EDIT = f"{DOCUMENT_SENTINEL}:window"
CONFLICT_EDIT = f"{DOCUMENT_SENTINEL}:conflict"
RECOVERY_EDIT = f"{DOCUMENT_SENTINEL}:recovery"

def validate_runtime_scan(value: Any) -> None:
    evidence.validate_runtime_scan(value, require_config=False, extra_counts=RECOVERY_SCAN_KEYS)
    if (
        value["canonical_recovery_records_scanned"] + value["recovery_leases_scanned"]
        > value["recovery_artifacts_scanned"]
    ):
        raise ValueError("runtime scan recovery artifact counts are inconsistent")
    if value["files_scanned"] < (
        value["app_logs_scanned"]
        + value["config_files_scanned"]
        + value["recovery_artifacts_scanned"]
        + 1
    ):
        raise ValueError("runtime scan file count is incomplete")


def validate_live_recovery_scan(value: Any) -> None:
    evidence.require_exact_keys(
        value, frozenset({"canonical_record_count", "canonical_records"}), "live recovery scan"
    )
    count = value["canonical_record_count"]
    records = value["canonical_records"]
    if not isinstance(count, int) or isinstance(count, bool) or count <= 0:
        raise ValueError("live recovery scan requires a canonical record")
    if not isinstance(records, list) or len(records) != count:
        raise ValueError("live recovery record count does not match fingerprints")
    for record in records:
        if validate_exact_fingerprint(record)["byte_count"] <= 0:
            raise ValueError("canonical recovery record must be nonempty")


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    if case_id == CASE_POINTER_CANCEL:
        editor = validate_fingerprint(observations.get("editor"))
        saved = validate_fingerprint(observations.get("source_after_cancel_save"))
        validate_fingerprint(observations.get("source_before"))
        require_true(observations, "cancel_kept_tab")
        if editor != saved:
            raise ValueError("Cancel did not preserve exact editor bytes")
    elif case_id == CASE_KEYBOARD_DISCARD:
        before = validate_fingerprint(observations.get("source_before"))
        after = validate_fingerprint(observations.get("source_after_discard"))
        validate_fingerprint(observations.get("discarded_editor"))
        require_true(observations, "discard_closed_tab")
        if before != after:
            raise ValueError("Discard changed source bytes")
    elif case_id == CASE_WINDOW_SAVE:
        editor = validate_fingerprint(observations.get("editor"))
        kept = validate_fingerprint(observations.get("editor_after_alt_f4_cancel"))
        untouched = validate_fingerprint(observations.get("source_after_alt_f4_cancel"))
        saved = validate_fingerprint(observations.get("saved_source"))
        require_true(observations, "alt_f4_cancel_kept_window", "window_exited")
        if kept != editor or untouched != fingerprint_bytes(ORIGINAL_BYTES).evidence():
            raise ValueError("Alt+F4 Cancel changed the editor or the source")
        if editor != saved:
            raise ValueError("window Save did not preserve exact editor bytes")
    elif case_id == CASE_EXTERNAL_CONFLICT:
        loaded = validate_fingerprint(observations.get("loaded_source"))
        external = validate_fingerprint(observations.get("external_source"))
        dirty = validate_fingerprint(observations.get("dirty_editor"))
        refused = validate_fingerprint(observations.get("source_after_refused_save"))
        after_conflict = validate_fingerprint(observations.get("editor_after_conflict"))
        saved = validate_fingerprint(observations.get("saved_after_explicit_overwrite"))
        require_true(
            observations,
            "same_length",
            "same_mtime_ns",
            "save_refused_with_conflict",
            "explicit_overwrite_used",
        )
        if loaded["byte_count"] != external["byte_count"] or loaded["sha256"] == external["sha256"]:
            raise ValueError("external rewrite evidence is not same-length and different")
        if refused != external:
            raise ValueError("refused Save changed the externally rewritten source")
        if after_conflict != dirty:
            raise ValueError("the conflict replaced the dirty editor")
        if dirty != saved or external == saved:
            raise ValueError("explicit overwrite did not preserve exact dirty editor bytes")
    elif case_id == CASE_RECOVERY:
        edited = validate_fingerprint(observations.get("edited_text"))
        restored = validate_fingerprint(observations.get("restored_editor"))
        after_discard = validate_fingerprint(observations.get("source_after_recovery_discard"))
        restarted = validate_fingerprint(observations.get("restart_editor"))
        require_true(
            observations,
            "checkpoint_record_seen",
            "recovery_offer_seen",
            "recovery_restored_exact",
            "discarded_recovery_absent",
            "startup_observed_before_restart_editor",
        )
        if observations.get("checkpoint_signal") != CHECKPOINT_SIGNAL:
            raise ValueError("recovery checkpoint signal is not the canonical record")
        if observations.get("startup_signal") != STARTUP_SIGNAL:
            raise ValueError("recovery startup signal is not the absent offer")
        if observations.get("termination") != "TerminateProcess":
            raise ValueError("recovery interruption did not use TerminateProcess")
        if observations.get("restart_count") != 2 or len(observations["process_contexts"]) != 3:
            raise ValueError("recovery scenario requires two restarts")
        if observations["live_runtime_scan"]["canonical_recovery_records_scanned"] < observations[
            "live_recovery_scan"
        ]["canonical_record_count"]:
            raise ValueError("live runtime scan omitted canonical recovery records")
        if observations["live_runtime_scan"]["recovery_leases_scanned"] < 1:
            raise ValueError("live runtime scan omitted recovery lease")
        signal_ms = observations.get("edit_to_signal_ms")
        if not finite_nonnegative(signal_ms) or signal_ms > 10_000:
            raise ValueError("recovery success signal exceeds 10000ms")
        for key in ("restore_ms", "discard_restart_absent_ms"):
            value = observations.get(key)
            if not finite_nonnegative(value):
                raise ValueError("recovery timing must be nonnegative")
        if edited != restored:
            raise ValueError("recovery did not restore exact editor bytes")
        if after_discard != restarted or restarted == edited:
            raise ValueError("discarded recovery was present after restart")


CONTRACT = evidence.EvidenceContract(
    schema=SCHEMA,
    case_flows=CASE_FLOWS,
    observation_keys=frozenset().union(*REQUIRED_OBSERVATIONS.values()),
    fingerprint_keys=FINGERPRINT_OBSERVATION_KEYS,
    required_observations=REQUIRED_OBSERVATIONS,
    validate_runtime_scan=validate_runtime_scan,
    validate_passed_case=validate_passed_case,
    private_sentinels=(DOCUMENT_SENTINEL,),
    safe_strings=SAFE_OBSERVATION_STRINGS,
    structured={
        "live_recovery_scan": validate_live_recovery_scan,
        "live_runtime_scan": validate_runtime_scan,
    },
)


def new_evidence(expected_hash: str) -> dict[str, Any]:
    return evidence.new_evidence(CONTRACT, expected_hash)


def validate_evidence(value: dict[str, Any]) -> None:
    evidence.validate_evidence(value, CONTRACT)


def canonical_recovery_records(data_root: Path) -> list[Path]:
    recovery_root = data_root / "recovery"
    try:
        return sorted(
            path
            for path in recovery_root.glob("*.mtrecovery")
            if path.is_file() and CANONICAL_RECOVERY_RECORD_RE.fullmatch(path.name)
        )
    except OSError:
        return []


def scan_live_recovery_records(data_root: Path) -> dict[str, Any]:
    records = canonical_recovery_records(data_root)
    if not records:
        raise HarnessFailure("CANONICAL_RECOVERY_RECORD_MISSING")
    utf8 = DOCUMENT_SENTINEL.encode("utf-8")
    utf16 = DOCUMENT_SENTINEL.encode("utf-16-le")
    fingerprints = []
    for path in records:
        try:
            value = path.read_bytes()
        except OSError as error:
            raise HarnessFailure(
                "LIVE_RECOVERY_RECORD_SCAN_FAILED", safe_exception_name(error)
            ) from None
        if not value:
            raise HarnessFailure("CANONICAL_RECOVERY_RECORD_EMPTY")
        if utf8 in value:
            raise HarnessFailure("UTF8_DOCUMENT_SENTINEL_LEAKED")
        if utf16 in value:
            raise HarnessFailure("UTF16LE_DOCUMENT_SENTINEL_LEAKED")
        fingerprints.append(fingerprint_bytes(value).evidence())
    return {"canonical_record_count": len(records), "canonical_records": fingerprints}


def scan_runtime_artifacts(data_root: Path, stderr_path: Path) -> dict[str, Any]:
    """The shared private-content scan, plus panics and recovery artifacts.

    Logs, recovery artifacts and stderr must hold no panic; the recovery
    directory's canonical records and lease are counted.
    """
    scan = evidence.scan_case_artifacts(
        data_root.parent, leak_codes("DOCUMENT_SENTINEL", DOCUMENT_SENTINEL)
    )
    try:
        logs = [path for path in (data_root / "logs").rglob("*") if path.is_file()]
        recovery = [path for path in (data_root / "recovery").rglob("*") if path.is_file()]
    except OSError as error:
        raise HarnessFailure("RUNTIME_ARTIFACT_ENUM_FAILED", safe_exception_name(error)) from None
    for path in (*logs, *recovery, stderr_path):
        try:
            lowered = path.read_bytes().lower()
        except OSError as error:
            raise HarnessFailure(
                "RUNTIME_ARTIFACT_SCAN_FAILED", safe_exception_name(error)
            ) from None
        if b"refcell already borrowed" in lowered:
            raise HarnessFailure("REFCELL_BORROW_PANIC_LOGGED")
        if PANIC_RE.search(lowered):
            raise HarnessFailure("PANIC_LOGGED")
    return {
        **scan,
        "recovery_artifacts_scanned": len(recovery),
        "canonical_recovery_records_scanned": sum(
            bool(CANONICAL_RECOVERY_RECORD_RE.fullmatch(path.name)) for path in recovery
        ),
        "recovery_leases_scanned": sum(path.name == RECOVERY_LEASE_NAME for path in recovery),
    }


class Goal02Harness(BaseNativeHarness):
    def wait_checkpoint_record(self, data_root: Path, started: float) -> float:
        """Wait for the dirty buffer's canonical record within the loss window."""
        remaining = 10.0 - (time.perf_counter() - started)
        if remaining <= 0:
            raise HarnessFailure("RECOVERY_RECORD_EXCEEDED_10_SECONDS")
        wait_until(
            lambda: canonical_recovery_records(data_root),
            remaining,
            "RECOVERY_RECORD_EXCEEDED_10_SECONDS",
            interval=0.025,
        )
        elapsed_ms = (time.perf_counter() - started) * 1000
        if elapsed_ms > 10_000:
            raise HarnessFailure("RECOVERY_RECORD_EXCEEDED_10_SECONDS")
        return elapsed_ms

    def close_tab_by_pointer(self, app: RunningApp) -> None:
        """Widen the rail under the pointer, then click the tab's close control."""
        row = self.find_control(
            app,
            RAIL_FILE_ROW_AUTOMATION_ID,
            "Button",
            "RAIL_FILE_ROW_UIA_TIMEOUT",
            "RAIL_FILE_ROW_UIA_CONTRACT_MISMATCH",
        )
        try:
            rectangle = row.rectangle()
        except Exception as error:
            raise HarnessFailure("RAIL_FILE_ROW_UIA_QUERY_FAILED", safe_exception_name(error)) from None
        self.win32.move_cursor(
            (rectangle.left + rectangle.right) // 2, (rectangle.top + rectangle.bottom) // 2
        )
        close = self.find_control(
            app,
            TAB_CLOSE_AUTOMATION_ID,
            "Button",
            "TAB_CLOSE_UIA_TIMEOUT",
            "TAB_CLOSE_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(close, "TAB_CLOSE_POINTER_CLICK_FAILED")

    def send_alt_f4(self, app: RunningApp) -> None:
        """Alt+F4 through SendInput to the foreground main window."""
        self.win32.require_foreground(app.hwnd, self.ui_timeout)
        self.win32.send_inputs(
            [
                key_input(VK_MENU, False),
                key_input(VK_F4, False),
                key_input(VK_F4, True),
                key_input(VK_MENU, True),
            ]
        )

    def require_editor_closed(self, app: RunningApp, code: str, timeout: float) -> None:
        wait_until(lambda: self.editor_absent_while_running(app), timeout, code)

    def scenario_pointer_cancel(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_POINTER_CANCEL)
        document = (workspace / "pointer.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source_before = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            editor = self.replace_editor(app, POINTER_EDIT)
            self.close_tab_by_pointer(app)
            self.click_lifecycle_decision(app, "Cancel")
            if app.process.poll() is not None:
                raise HarnessFailure("CANCEL_CLOSED_PROCESS")
            if self.editor_fingerprint(app) != editor:
                raise HarnessFailure("CANCEL_CHANGED_EDITOR_BYTES")
            if sha256_file(document) != source_before:
                raise HarnessFailure("CANCEL_CHANGED_SOURCE")
            self.save_via_menu(app)
            saved = self.wait_file(document, editor, "CANCEL_FOLLOWUP_SAVE_TIMEOUT")
            observations = {
                "editor": editor.evidence(),
                "source_before": source_before.evidence(),
                "source_after_cancel_save": saved.evidence(),
                "cancel_kept_tab": True,
                "flow": CASE_FLOWS[CASE_POINTER_CANCEL],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_keyboard_discard(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_KEYBOARD_DISCARD)
        document = (workspace / "keyboard.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source_before = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            discarded = self.replace_editor(app, KEYBOARD_EDIT)
            self.focus_editor(app)
            self.win32.send_shortcut(app.hwnd, VK_W)
            self.click_lifecycle_decision(app, "Discard")
            self.require_editor_closed(app, "DISCARD_TAB_CLOSE_TIMEOUT", self.ui_timeout)
            source_after = sha256_file(document)
            if source_after != source_before:
                raise HarnessFailure("DISCARD_CHANGED_SOURCE")
            self.close_app(app)
            observations = {
                "discarded_editor": discarded.evidence(),
                "source_before": source_before.evidence(),
                "source_after_discard": source_after.evidence(),
                "discard_closed_tab": True,
                "flow": CASE_FLOWS[CASE_KEYBOARD_DISCARD],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_window_save(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_WINDOW_SAVE)
        document = (workspace / "window.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            editor = self.replace_editor(app, WINDOW_EDIT)
            # Alt+F4 must raise the guard, not close the window; Cancel keeps it.
            self.send_alt_f4(app)
            self.click_lifecycle_decision(app, "Cancel")
            if app.process.poll() is not None:
                raise HarnessFailure("ALT_F4_CANCEL_CLOSED_PROCESS")
            kept = self.editor_fingerprint(app)
            if kept != editor:
                raise HarnessFailure("ALT_F4_CANCEL_CHANGED_EDITOR_BYTES")
            untouched = sha256_file(document)
            if untouched != fingerprint_bytes(ORIGINAL_BYTES):
                raise HarnessFailure("ALT_F4_CANCEL_CHANGED_SOURCE")
            # The platform close must raise the guard, not close the window.
            self.win32.post_close(app.hwnd)
            self.click_lifecycle_decision(app, "Save")
            self.wait_process_exit(app, max(self.ui_timeout, 20.0))
            saved = sha256_file(document)
            if saved != editor:
                raise HarnessFailure("WINDOW_SAVE_BYTES_MISMATCH")
            observations = {
                "editor": editor.evidence(),
                "alt_f4_cancel_kept_window": True,
                "editor_after_alt_f4_cancel": kept.evidence(),
                "source_after_alt_f4_cancel": untouched.evidence(),
                "saved_source": saved.evidence(),
                "flow": CASE_FLOWS[CASE_WINDOW_SAVE],
                "window_exited": True,
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_external_conflict(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_EXTERNAL_CONFLICT)
        document = (workspace / "conflict.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        loaded = sha256_file(document)
        app = self.launch(document, data, config, workspace, stderr)
        try:
            dirty = self.replace_editor(app, CONFLICT_EDIT)
            before = document.stat()
            if len(ORIGINAL_BYTES) != len(EXTERNAL_BYTES):
                raise HarnessFailure("FIXTURE_LENGTH_MISMATCH")
            write_durable(document, EXTERNAL_BYTES)
            os.utime(document, ns=(before.st_atime_ns, before.st_mtime_ns))
            after = document.stat()
            if after.st_size != before.st_size or after.st_mtime_ns != before.st_mtime_ns:
                raise HarnessFailure("EXACT_MTIME_RESTORE_UNAVAILABLE")
            external = sha256_file(document)
            if external != fingerprint_bytes(EXTERNAL_BYTES):
                raise HarnessFailure("EXTERNAL_REWRITE_NOT_OBSERVED")

            # Save must detect the same-length rewrite and refuse to write.
            self.save_via_menu(app)
            self.wait_name(app, CONFLICT_HEADING, "Text", "SAVE_CONFLICT_UIA_TIMEOUT")
            refused = sha256_file(document)
            if refused != external:
                raise HarnessFailure("SAVE_OVERWROTE_EXTERNAL_REWRITE")
            after_conflict = self.editor_fingerprint(app)
            if after_conflict != dirty:
                raise HarnessFailure("CONFLICT_CHANGED_EDITOR")
            keep_mine = self.find_control(
                app,
                KEEP_MINE_AUTOMATION_ID,
                "Button",
                "CONFLICT_KEEP_MINE_UIA_TIMEOUT",
                "CONFLICT_KEEP_MINE_UIA_CONTRACT_MISMATCH",
            )
            self.click_control(keep_mine, "CONFLICT_KEEP_MINE_CLICK_FAILED")
            # Keep mine opens the Overwrite alert, which UIA cannot see; Enter
            # answers it with Overwrite, proven by the written file.
            self.confirm_system_alert(
                app,
                lambda: sha256_file(document) == external,
                "KEEP_MINE_WROTE_BEFORE_CONFIRMATION",
            )
            saved = self.wait_file(document, dirty, "EXPLICIT_OVERWRITE_SAVE_TIMEOUT")
            self.close_app(app)
            observations = {
                "loaded_source": loaded.evidence(),
                "external_source": external.evidence(),
                "dirty_editor": dirty.evidence(),
                "source_after_refused_save": refused.evidence(),
                "editor_after_conflict": after_conflict.evidence(),
                "saved_after_explicit_overwrite": saved.evidence(),
                "same_length": True,
                "same_mtime_ns": True,
                "save_refused_with_conflict": True,
                "explicit_overwrite_used": True,
                "flow": CASE_FLOWS[CASE_EXTERNAL_CONFLICT],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations

    def scenario_recovery(self) -> dict[str, Any]:
        data, config, workspace, stderr = self.case_roots(CASE_RECOVERY)
        document = (workspace / "recovery.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source = sha256_file(document)
        long_timeout = max(self.ui_timeout, 20.0)

        first = self.launch(document, data, config, workspace, stderr)
        try:
            if canonical_recovery_records(data):
                raise HarnessFailure("RECOVERY_RECORD_BEFORE_EDIT")
            input_started = time.perf_counter()
            edited = self.replace_editor(first, RECOVERY_EDIT)
            signal_ms = self.wait_checkpoint_record(data, input_started)
            live_recovery_scan = scan_live_recovery_records(data)
            self.terminate(first)
        finally:
            self.reap(first)

        live_runtime_scan = scan_runtime_artifacts(data, stderr)

        second = self.launch(document, data, config, workspace, stderr)
        try:
            restore_started = time.perf_counter()
            restore = self.find_control(
                second,
                RESTORE_AUTOMATION_ID,
                "Button",
                "RECOVERY_RESTORE_UIA_TIMEOUT",
                "RECOVERY_RESTORE_UIA_CONTRACT_MISMATCH",
                long_timeout,
            )
            self.click_control(restore, "RECOVERY_RESTORE_CLICK_FAILED")
            restored, _ = self.wait_editor_fingerprint(second, edited, long_timeout)
            restore_ms = (time.perf_counter() - restore_started) * 1000
            self.focus_editor(second)
            self.win32.send_shortcut(second.hwnd, VK_W)
            self.click_lifecycle_decision(second, "Discard")
            self.require_editor_closed(second, "RECOVERY_DISCARD_CLOSE_TIMEOUT", long_timeout)
            if sha256_file(document) != source:
                raise HarnessFailure("RECOVERY_DISCARD_CHANGED_SOURCE")
            self.close_app(second)
        finally:
            self.reap(second)

        third = self.launch(document, data, config, workspace, stderr)
        try:
            restart_started = time.perf_counter()
            # The store is read before the first frame: once the document is
            # on screen, a remaining record would already be offered.
            if self.name_present(third, RESTORE_NAME, "Button"):
                raise HarnessFailure("DISCARDED_RECOVERY_OFFERED_AGAIN")
            if canonical_recovery_records(data):
                raise HarnessFailure("DISCARDED_RECOVERY_RECORD_RETAINED")
            restarted, _ = self.wait_editor_fingerprint(third, source, long_timeout)
            absent_ms = (time.perf_counter() - restart_started) * 1000
            observations = {
                "edited_text": edited.evidence(),
                "checkpoint_record_seen": True,
                "checkpoint_signal": CHECKPOINT_SIGNAL,
                "live_runtime_scan": live_runtime_scan,
                "live_recovery_scan": live_recovery_scan,
                "edit_to_signal_ms": round(signal_ms, 3),
                "recovery_offer_seen": True,
                "restored_editor": restored.evidence(),
                "recovery_restored_exact": restored == edited,
                "restore_ms": round(restore_ms, 3),
                "source_after_recovery_discard": sha256_file(document).evidence(),
                "restart_editor": restarted.evidence(),
                "discarded_recovery_absent": restarted == source,
                "discard_restart_absent_ms": round(absent_ms, 3),
                "termination": "TerminateProcess",
                "restart_count": 2,
                "startup_signal": STARTUP_SIGNAL,
                "startup_observed_before_restart_editor": True,
                "flow": CASE_FLOWS[CASE_RECOVERY],
                "process_contexts": [
                    first.security_context.evidence(),
                    second.security_context.evidence(),
                    third.security_context.evidence(),
                ],
                "foreground_verified": True,
            }
        finally:
            self.reap(third)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        return observations


def native_run_plan() -> NativeRunPlan:
    return NativeRunPlan(
        required_case_ids=REQUIRED_CASE_IDS,
        workdir_prefix="markturbo-goal-02-native-",
        new_evidence=new_evidence,
        validate_evidence=validate_evidence,
        preflight=preflight,
        ui_types_loader=load_pywinauto,
        harness_factory=Goal02Harness,
        scenarios=lambda harness: (
            harness.scenario_pointer_cancel,
            harness.scenario_keyboard_discard,
            harness.scenario_window_save,
            harness.scenario_external_conflict,
            harness.scenario_recovery,
        ),
    )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = evidence.build_parser(__doc__.splitlines()[0], "goal-02", REQUIRED_CASE_IDS)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    return main_native_acceptance(args, native_run_plan())


if __name__ == "__main__":
    raise SystemExit(main())
