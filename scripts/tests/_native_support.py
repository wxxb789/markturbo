"""Fixtures shared by the native harness unit tests; nothing here launches a UI."""

from __future__ import annotations

import hashlib
import struct
from collections.abc import Callable
from typing import Any

HASH = "a" * 64


def fingerprint(value: bytes) -> dict[str, int | str]:
    return {"byte_count": len(value), "sha256": hashlib.sha256(value).hexdigest()}


def process_context() -> dict[str, int | str]:
    return {"session_id": 7, "integrity_rid": 0x2000, "integrity": "medium"}


def environment() -> dict[str, Any]:
    return {
        "platform": "Windows 11",
        "windows_major": 10,
        "windows_minor": 0,
        "windows_build": 22631,
        "architecture": "x86_64",
        "native_machine_code": 0x8664,
        "python_pointer_bits": 64,
        "wts_state": "WTSActive",
        "active_console_session_id": 7,
        "harness_is_console_session": True,
        "input_desktop": "Default",
        "thread_desktop": "Default",
        "harness_process": process_context(),
    }


def runtime_scan(**extra: int) -> dict[str, int]:
    return {"files_scanned": 3 + sum(extra.values()), "app_logs_scanned": 1, "config_files_scanned": 1, **extra}


def case_observations(flow: str, **observations: Any) -> dict[str, Any]:
    """A passed case's common observations plus the case's own."""
    return {
        "flow": flow,
        "process_context": process_context(),
        "runtime_scan": runtime_scan(),
        **observations,
    }


def passed_evidence(
    evidence: dict[str, Any],
    observations: Callable[[str], dict[str, Any]],
    complete: Callable[[dict[str, Any], str], None],
) -> dict[str, Any]:
    """Fill a goal's new evidence as a hash-bound PASS of every case."""
    evidence["executable"].update(
        sha256=HASH,
        byte_count=1234,
        hash_verified=True,
        copied_sha256=HASH,
        copy_hash_verified=True,
        format="PE32+",
        machine="x86_64",
        machine_code=0x8664,
        optional_magic=0x20B,
    )
    evidence["environment"] = environment()
    for case in evidence["cases"]:
        case.update(status="PASS", duration_ms=1.25, observations=observations(case["id"]))
    complete(evidence, "PASS")
    return evidence


def minimal_pe(machine: int = 0x8664, magic: int = 0x20B) -> bytes:
    data = bytearray(512)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<H", data, 0x84, machine)
    struct.pack_into("<H", data, 0x86, 1)
    struct.pack_into("<H", data, 0x94, 0xF0)
    struct.pack_into("<H", data, 0x98, magic)
    data[0x188:0x190] = b".text\0\0\0"
    struct.pack_into("<I", data, 0x190, 123)
    struct.pack_into("<I", data, 0x198, 512)
    struct.pack_into("<I", data, 0x1AC, 0x60000020)
    return bytes(data)
