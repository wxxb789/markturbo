"""Exercise Goal 02 destructive paths through a real Windows UI.

The harness is intentionally fail-closed. It emits PASS only after every
required case runs against the expected x64 release executable in an active,
unlocked Windows 11 session. Document text is never written to stdout or JSON;
observations contain only UTF-8 byte counts, SHA-256 digests, and timings.

Two cases, three launches. The guarded session runs in one app: a same-length
external rewrite (mtime restored) must be refused by menu Save until Keep mine
is confirmed; the rail's close control, revealed by a real pointer hover, must
raise the in-window guard card (Save / Don't save / Cancel), whose Cancel keeps
the text, then menu Save writes it; Alt+F4 and the native window close
(WM_CLOSE) must raise the guard, never close a dirty document unasked, and the
guard's Save writes the exact bytes, retires the recovery record and exits 0.
The recovery session needs a restart: an interrupted dirty buffer (CJK + emoji)
must leave a canonical recovery record within 10 000 ms; the relaunch, given no
document so the restored tab is the only one, offers it to Restore, and Ctrl+W's
guard Don't save then closes the tab without writing and retires the record.

Every phase has its own observation-key prefix and failure-code prefix
(`PHASE_*`), and each launch its own failure-code prefix (`LAUNCH_*`), so a
failed case names the step that failed.
"""

from __future__ import annotations

import argparse
import os
import re
import time
from collections.abc import Iterator
from contextlib import contextmanager
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
    safe_exception_name,
    sha256_file,
    validate_fingerprint,
    wait_until,
    write_durable,
)

SCHEMA = "markturbo.goal-02-native-acceptance"
CASE_GUARDED_SESSION = "guarded-session"
CASE_RECOVERY_SESSION = "recovery-session"
CASE_FLOWS = {
    CASE_GUARDED_SESSION: (
        "same-length rewrite -> menu Save refused with conflict -> Keep mine -> "
        "Overwrite; pointer rail close -> guard Cancel -> menu Save; "
        "SendInput Alt+F4 -> guard Cancel -> WM_CLOSE -> guard Save -> exit"
    ),
    CASE_RECOVERY_SESSION: (
        "edit -> recovery record -> TerminateProcess -> relaunch -> Restore; "
        "SendInput Ctrl+W -> guard Don't save -> recovery record retired"
    ),
}
REQUIRED_CASE_IDS = tuple(CASE_FLOWS)
SAFE_OBSERVATION_STRINGS = set(CASE_FLOWS.values()) | INTEGRITY_NAMES

# Phase failure-code prefixes; each phase's observation keys start with the
# same name in lower case.
PHASE_EXTERNAL = "EXTERNAL_MODIFICATION"
PHASE_TAB_CLOSE = "TAB_CLOSE"
PHASE_WINDOW_CLOSE = "WINDOW_CLOSE"
PHASE_CHECKPOINT = "CHECKPOINT"
PHASE_RESTORE = "RESTORE"
PHASE_KEYBOARD_CLOSE = "KEYBOARD_CLOSE"
# Launch failure-code prefixes; a phase wrapping a launch keeps them.
LAUNCH_GUARDED = "GUARDED_LAUNCH"
LAUNCH_CHECKPOINT = f"{PHASE_CHECKPOINT}_LAUNCH"
LAUNCH_RESTORE = f"{PHASE_RESTORE}_LAUNCH"

FINGERPRINT_OBSERVATION_KEYS = frozenset(
    {
        "external_modification_loaded_source",
        "external_modification_rewritten_source",
        "external_modification_dirty_editor",
        "external_modification_source_after_refused_save",
        "external_modification_editor_after_refusal",
        "external_modification_overwritten_source",
        "tab_close_editor",
        "tab_close_source_after_cancel",
        "tab_close_saved_source",
        "window_close_editor",
        "window_close_editor_after_cancel",
        "window_close_source_after_cancel",
        "window_close_saved_source",
        "checkpoint_editor",
        "restore_editor",
        "keyboard_close_source_before",
        "keyboard_close_source_after",
    }
)
REQUIRED_OBSERVATIONS = {
    CASE_GUARDED_SESSION: frozenset(
        {
            "external_modification_loaded_source",
            "external_modification_rewritten_source",
            "external_modification_dirty_editor",
            "external_modification_source_after_refused_save",
            "external_modification_editor_after_refusal",
            "external_modification_overwritten_source",
            "external_modification_conflict_ms",
            "tab_close_editor",
            "tab_close_source_after_cancel",
            "tab_close_saved_source",
            "window_close_editor",
            "window_close_editor_after_cancel",
            "window_close_source_after_cancel",
            "window_close_saved_source",
            "process_context",
        }
    ),
    CASE_RECOVERY_SESSION: frozenset(
        {
            "checkpoint_editor",
            "checkpoint_ms",
            "checkpoint_live_records",
            "checkpoint_runtime_scan",
            "restore_editor",
            "restore_ms",
            "keyboard_close_source_before",
            "keyboard_close_source_after",
            "keyboard_close_ms",
            "keyboard_close_retire_ms",
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
# mt-core `recovery.rs` names its transaction, temporary and retirement
# artifacts with this prefix; the lease (`.markturbo-recovery.lock`) has none.
RECOVERY_ARTIFACT_PREFIX = ".markturbo-recovery-"
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
VK_F4 = 0x73
DOCUMENT_SENTINEL = "MTG02-NATIVE-SENTINEL-\u4fdd\u5b58-\U0001f680"
CANONICAL_RECOVERY_RECORD_RE = re.compile(r"^[0-9a-f]{64}\.mtrecovery$")
RECOVERY_LOSS_WINDOW_MS = 10_000

ORIGINAL_BYTES = b"native acceptance original"
EXTERNAL_BYTES = b"native acceptance external"
CONFLICT_EDIT = f"{DOCUMENT_SENTINEL}:conflict"
POINTER_EDIT = f"{DOCUMENT_SENTINEL}:pointer"
WINDOW_EDIT = f"{DOCUMENT_SENTINEL}:window"
RECOVERY_EDIT = f"{DOCUMENT_SENTINEL}:recovery"


@contextmanager
def phase(prefix: str) -> Iterator[None]:
    """Prefix every failure raised inside one phase with the phase's name."""
    try:
        yield
    except HarnessFailure as error:
        if error.code.startswith(f"{prefix}_"):
            raise
        raise HarnessFailure(f"{prefix}_{error.code}", error.detail) from None


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


def _fingerprints(observations: dict[str, Any], prefix: str, *names: str) -> list[dict[str, Any]]:
    return [validate_fingerprint(observations.get(f"{prefix}_{name}")) for name in names]


def _require_timing(observations: dict[str, Any], key: str) -> float:
    value = observations.get(key)
    if not finite_nonnegative(value):
        raise ValueError(f"{key} must be a nonnegative timing")
    return value


def validate_guarded_session(observations: dict[str, Any]) -> None:
    loaded, rewritten, dirty, refused, after_refusal, overwritten = _fingerprints(
        observations,
        "external_modification",
        "loaded_source",
        "rewritten_source",
        "dirty_editor",
        "source_after_refused_save",
        "editor_after_refusal",
        "overwritten_source",
    )
    if loaded["byte_count"] != rewritten["byte_count"] or loaded["sha256"] == rewritten["sha256"]:
        raise ValueError("external rewrite evidence is not same-length and different")
    _require_timing(observations, "external_modification_conflict_ms")
    if refused != rewritten:
        raise ValueError("refused Save changed the externally rewritten source")
    if after_refusal != dirty:
        raise ValueError("the conflict replaced the dirty editor")
    if dirty != overwritten or rewritten == overwritten:
        raise ValueError("explicit overwrite did not preserve exact dirty editor bytes")

    editor, kept_source, saved = _fingerprints(
        observations, "tab_close", "editor", "source_after_cancel", "saved_source"
    )
    if kept_source != overwritten or editor == kept_source:
        raise ValueError("tab close Cancel changed the source")
    if editor != saved:
        raise ValueError("Cancel did not preserve exact editor bytes for Save")

    window, kept, untouched, window_saved = _fingerprints(
        observations,
        "window_close",
        "editor",
        "editor_after_cancel",
        "source_after_cancel",
        "saved_source",
    )
    if kept != window or untouched != saved or window == untouched:
        raise ValueError("Alt+F4 Cancel changed the editor or the source")
    if window != window_saved:
        raise ValueError("window Save did not preserve exact editor bytes")
    if observations["runtime_scan"]["canonical_recovery_records_scanned"] != 0:
        raise ValueError("saved document kept its recovery record")


def validate_recovery_session(observations: dict[str, Any]) -> None:
    (edited,) = _fingerprints(observations, "checkpoint", "editor")
    signal_ms = _require_timing(observations, "checkpoint_ms")
    if signal_ms > RECOVERY_LOSS_WINDOW_MS:
        raise ValueError("recovery checkpoint exceeds 10000ms")
    live_scan = observations["checkpoint_runtime_scan"]
    if live_scan["canonical_recovery_records_scanned"] < observations[
        "checkpoint_live_records"
    ]["canonical_record_count"]:
        raise ValueError("live runtime scan omitted canonical recovery records")
    if live_scan["recovery_leases_scanned"] < 1:
        raise ValueError("live runtime scan omitted recovery lease")

    (restored,) = _fingerprints(observations, "restore", "editor")
    _require_timing(observations, "restore_ms")
    if restored != edited:
        raise ValueError("recovery did not restore exact editor bytes")

    before, after = _fingerprints(
        observations, "keyboard_close", "source_before", "source_after"
    )
    _require_timing(observations, "keyboard_close_ms")
    _require_timing(observations, "keyboard_close_retire_ms")
    if before != after or restored == before:
        raise ValueError("keyboard close Don't save changed the source")
    if observations["runtime_scan"]["canonical_recovery_records_scanned"] != 0:
        raise ValueError("discarded recovery record was retained")
    if len(observations["process_contexts"]) != 2:
        raise ValueError("recovery session requires one interrupted and one relaunched process")


def validate_passed_case(case_id: str, observations: dict[str, Any]) -> None:
    if case_id == CASE_GUARDED_SESSION:
        validate_guarded_session(observations)
    elif case_id == CASE_RECOVERY_SESSION:
        validate_recovery_session(observations)


CONTRACT = evidence.EvidenceContract(
    schema=SCHEMA,
    case_flows=CASE_FLOWS,
    observation_keys=frozenset().union(*REQUIRED_OBSERVATIONS.values()),
    fingerprint_keys=FINGERPRINT_OBSERVATION_KEYS,
    required_observations=REQUIRED_OBSERVATIONS,
    validate_runtime_scan=validate_runtime_scan,
    validate_passed_case=validate_passed_case,
    private_sentinels=(DOCUMENT_SENTINEL,),
    safe_strings=frozenset(SAFE_OBSERVATION_STRINGS),
    structured={
        "checkpoint_live_records": validate_live_recovery_scan,
        "checkpoint_runtime_scan": validate_runtime_scan,
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


def retained_recovery_artifacts(data_root: Path) -> list[Path]:
    """Recovery records and their transaction or retirement artifacts.

    After a deliberate Don't save none may remain: the next launch offers only
    what these hold. The lease is the running app's and is not counted.
    """
    try:
        return sorted(
            path
            for path in (data_root / "recovery").iterdir()
            if CANONICAL_RECOVERY_RECORD_RE.fullmatch(path.name)
            or path.name.startswith(RECOVERY_ARTIFACT_PREFIX)
        )
    except FileNotFoundError:
        return []
    except OSError as error:
        raise HarnessFailure(
            "RECOVERY_ARTIFACT_ENUM_FAILED", safe_exception_name(error)
        ) from None


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


def elapsed_ms(started: float) -> float:
    return round((time.perf_counter() - started) * 1000, 3)


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
        elapsed = (time.perf_counter() - started) * 1000
        if elapsed > RECOVERY_LOSS_WINDOW_MS:
            raise HarnessFailure("RECOVERY_RECORD_EXCEEDED_10_SECONDS")
        return elapsed

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
            "RAIL_CLOSE_UIA_TIMEOUT",
            "RAIL_CLOSE_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(close, "RAIL_CLOSE_POINTER_CLICK_FAILED")

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

    def require_running(self, app: RunningApp, code: str) -> None:
        if app.process.poll() is not None:
            raise HarnessFailure(code)

    def external_modification(self, app: RunningApp, document: Path) -> dict[str, Any]:
        """A same-length rewrite with the loaded mtime is refused, then overwritten."""
        loaded = sha256_file(document)
        dirty = self.replace_editor(app, CONFLICT_EDIT)
        before = document.stat()
        if len(ORIGINAL_BYTES) != len(EXTERNAL_BYTES):
            raise HarnessFailure("FIXTURE_LENGTH_MISMATCH")
        write_durable(document, EXTERNAL_BYTES)
        os.utime(document, ns=(before.st_atime_ns, before.st_mtime_ns))
        after = document.stat()
        if after.st_size != before.st_size or after.st_mtime_ns != before.st_mtime_ns:
            raise HarnessFailure("EXACT_MTIME_RESTORE_UNAVAILABLE")
        rewritten = sha256_file(document)
        if rewritten != fingerprint_bytes(EXTERNAL_BYTES):
            raise HarnessFailure("REWRITE_NOT_OBSERVED")

        # Save must detect the same-length rewrite and refuse to write.
        save_started = time.perf_counter()
        self.save_via_menu(app)
        self.wait_name(app, CONFLICT_HEADING, "Text", "SAVE_CONFLICT_UIA_TIMEOUT")
        conflict_ms = elapsed_ms(save_started)
        refused = sha256_file(document)
        if refused != rewritten:
            raise HarnessFailure("SAVE_OVERWROTE_EXTERNAL_REWRITE")
        after_refusal = self.editor_fingerprint(app)
        if after_refusal != dirty:
            raise HarnessFailure("CONFLICT_CHANGED_EDITOR")
        keep_mine = self.find_control(
            app,
            KEEP_MINE_AUTOMATION_ID,
            "Button",
            "KEEP_MINE_UIA_TIMEOUT",
            "KEEP_MINE_UIA_CONTRACT_MISMATCH",
        )
        self.click_control(keep_mine, "KEEP_MINE_CLICK_FAILED")
        # Keep mine opens the Overwrite alert, which UIA cannot see; Enter
        # answers it with Overwrite, proven by the written file.
        self.confirm_system_alert(
            app,
            lambda: sha256_file(document) == rewritten,
            "KEEP_MINE_WROTE_BEFORE_CONFIRMATION",
        )
        overwritten = self.wait_file(document, dirty, "OVERWRITE_SAVE_TIMEOUT")
        # The next phases start from a clean, conflict-free document.
        wait_until(
            lambda: not self.name_present(app, CONFLICT_HEADING, "Text")
            and not self.document_dirty(app),
            self.ui_timeout,
            "CONFLICT_NOT_RESOLVED",
        )
        return {
            "external_modification_loaded_source": loaded.evidence(),
            "external_modification_rewritten_source": rewritten.evidence(),
            "external_modification_dirty_editor": dirty.evidence(),
            "external_modification_source_after_refused_save": refused.evidence(),
            "external_modification_editor_after_refusal": after_refusal.evidence(),
            "external_modification_overwritten_source": overwritten.evidence(),
            "external_modification_conflict_ms": conflict_ms,
        }

    def tab_close(self, app: RunningApp, document: Path) -> dict[str, Any]:
        """The hover-revealed rail close raises the guard; Cancel keeps the text."""
        editor = self.replace_editor(app, POINTER_EDIT)
        source = sha256_file(document)
        self.close_tab_by_pointer(app)
        self.click_lifecycle_decision(app, "Cancel")
        self.require_running(app, "CANCEL_CLOSED_PROCESS")
        if self.editor_fingerprint(app) != editor:
            raise HarnessFailure("CANCEL_CHANGED_EDITOR_BYTES")
        if not self.document_dirty(app):
            raise HarnessFailure("CANCEL_CLEARED_DIRTY_MARK")
        kept_source = sha256_file(document)
        if kept_source != source:
            raise HarnessFailure("CANCEL_CHANGED_SOURCE")
        self.save_via_menu(app)
        saved = self.wait_file(document, editor, "FOLLOWUP_SAVE_TIMEOUT")
        return {
            "tab_close_editor": editor.evidence(),
            "tab_close_source_after_cancel": kept_source.evidence(),
            "tab_close_saved_source": saved.evidence(),
        }

    def window_close(self, app: RunningApp, document: Path) -> dict[str, Any]:
        """Alt+F4 and WM_CLOSE raise the guard; its Save writes and exits.

        `wait_process_exit` fails with PROCESS_EXIT_NONZERO unless the code is 0.
        """
        editor = self.replace_editor(app, WINDOW_EDIT)
        source = sha256_file(document)
        self.send_alt_f4(app)
        self.click_lifecycle_decision(app, "Cancel")
        self.require_running(app, "ALT_F4_CANCEL_CLOSED_PROCESS")
        kept = self.editor_fingerprint(app)
        if kept != editor:
            raise HarnessFailure("ALT_F4_CANCEL_CHANGED_EDITOR_BYTES")
        untouched = sha256_file(document)
        if untouched != source:
            raise HarnessFailure("ALT_F4_CANCEL_CHANGED_SOURCE")
        # The platform close must raise the guard, not close the window.
        self.win32.post_close(app.hwnd)
        self.click_lifecycle_decision(app, "Save")
        self.wait_process_exit(app, max(self.ui_timeout, 20.0))
        saved = sha256_file(document)
        if saved != editor:
            raise HarnessFailure("SAVE_BYTES_MISMATCH")
        return {
            "window_close_editor": editor.evidence(),
            "window_close_editor_after_cancel": kept.evidence(),
            "window_close_source_after_cancel": untouched.evidence(),
            "window_close_saved_source": saved.evidence(),
        }

    def scenario_guarded_session(self) -> dict[str, Any]:
        """One launch: external modification, then tab close, then window close.

        The conflict runs first so its stamp is the one the app loaded; each
        later phase starts from the clean document the previous one saved, and
        window close ends the process.
        """
        data, config, workspace, stderr = self.case_roots(CASE_GUARDED_SESSION)
        document = (workspace / "guarded.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        app: RunningApp | None = None
        try:
            with phase(LAUNCH_GUARDED):
                app = self.launch(document, data, config, workspace, stderr)
            with phase(PHASE_EXTERNAL):
                observations = self.external_modification(app, document)
            with phase(PHASE_TAB_CLOSE):
                observations |= self.tab_close(app, document)
            with phase(PHASE_WINDOW_CLOSE):
                observations |= self.window_close(app, document)
            observations |= {
                "flow": CASE_FLOWS[CASE_GUARDED_SESSION],
                "process_context": app.security_context.evidence(),
                "foreground_verified": True,
            }
        finally:
            if app is not None:
                self.reap(app)
        observations["runtime_scan"] = scan_runtime_artifacts(data, stderr)
        # Every Save retired the document's record; none may outlive the exit.
        if observations["runtime_scan"]["canonical_recovery_records_scanned"]:
            raise HarnessFailure(f"{PHASE_WINDOW_CLOSE}_RECOVERY_RECORD_RETAINED_AFTER_SAVE")
        return observations

    def scenario_recovery_session(self) -> dict[str, Any]:
        """Two launches: the interruption needs a restart; Ctrl+W closes the restored tab.

        The relaunch opens no document: an argv document would open as its own
        clean tab beside the restored one (`AppState::add_tab_with` does not
        deduplicate paths), and closing the restored tab would leave that tab's
        editor on screen.
        """
        data, config, workspace, stderr = self.case_roots(CASE_RECOVERY_SESSION)
        document = (workspace / "recovery.md").resolve()
        write_durable(document, ORIGINAL_BYTES)
        source = sha256_file(document)
        long_timeout = max(self.ui_timeout, 20.0)

        with phase(PHASE_CHECKPOINT):
            with phase(LAUNCH_CHECKPOINT):
                first = self.launch(document, data, config, workspace, stderr)
            try:
                if canonical_recovery_records(data):
                    raise HarnessFailure("RECORD_BEFORE_EDIT")
                input_started = time.perf_counter()
                edited = self.replace_editor(first, RECOVERY_EDIT)
                checkpoint_ms = round(self.wait_checkpoint_record(data, input_started), 3)
                live_records = scan_live_recovery_records(data)
                self.terminate(first)
            finally:
                self.reap(first)
            live_scan = scan_runtime_artifacts(data, stderr)

        second: RunningApp | None = None
        try:
            with phase(PHASE_RESTORE):
                with phase(LAUNCH_RESTORE):
                    second = self.launch_app(None, data, config, workspace, stderr)
                restore_started = time.perf_counter()
                restore = self.find_control(
                    second,
                    RESTORE_AUTOMATION_ID,
                    "Button",
                    "UIA_TIMEOUT",
                    "UIA_CONTRACT_MISMATCH",
                    long_timeout,
                )
                self.click_control(restore, "CLICK_FAILED")
                self.activate_source_layout(second)
                restored, _ = self.wait_editor_fingerprint(second, edited, long_timeout)
                restore_ms = elapsed_ms(restore_started)
            with phase(PHASE_KEYBOARD_CLOSE):
                close_started = time.perf_counter()
                self.focus_editor(second)
                self.win32.send_shortcut(second.hwnd, VK_W)
                self.click_lifecycle_decision(second, "Discard")
                discard_started = time.perf_counter()
                wait_until(
                    lambda: self.editor_absent_while_running(second),
                    long_timeout,
                    "TAB_CLOSE_TIMEOUT",
                )
                close_ms = elapsed_ms(close_started)
                wait_until(
                    lambda: not retained_recovery_artifacts(data),
                    self.ui_timeout,
                    "RECOVERY_RECORD_RETAINED",
                )
                retire_ms = elapsed_ms(discard_started)
                after = sha256_file(document)
                if after != source:
                    raise HarnessFailure("DISCARD_CHANGED_SOURCE")
                self.close_app(second)
                if retained_recovery_artifacts(data):
                    raise HarnessFailure("RECOVERY_RECORD_RETAINED_AFTER_EXIT")
            observations = {
                "checkpoint_editor": edited.evidence(),
                "checkpoint_ms": checkpoint_ms,
                "checkpoint_live_records": live_records,
                "checkpoint_runtime_scan": live_scan,
                "restore_editor": restored.evidence(),
                "restore_ms": restore_ms,
                "keyboard_close_source_before": source.evidence(),
                "keyboard_close_source_after": after.evidence(),
                "keyboard_close_ms": close_ms,
                "keyboard_close_retire_ms": retire_ms,
                "flow": CASE_FLOWS[CASE_RECOVERY_SESSION],
                "process_contexts": [
                    first.security_context.evidence(),
                    second.security_context.evidence(),
                ],
                "foreground_verified": True,
            }
        finally:
            if second is not None:
                self.reap(second)
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
            harness.scenario_guarded_session,
            harness.scenario_recovery_session,
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
