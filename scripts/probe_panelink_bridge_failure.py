#!/usr/bin/env python3
"""Prove a managed provider bridge failure leaves its owned PTY and binding safe.

The fixture uses pinned OpenCode 2.0.32 in a fresh HOME and loopback-only
sandbox. It stops only the exact OpenCode child directly owned by its private
Panelink `serve` process, observes the bridge fail closed, resumes that same
child, and verifies that recovery remains unavailable without a new launch.
No prompt or model call is submitted.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts import probe_panelink_manual_recovery as recovery
    from scripts import probe_panelink_provider_bridge as bridge
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    import probe_panelink_manual_recovery as recovery
    import probe_panelink_provider_bridge as bridge


MAX_SECONDS = 60.0
LEASE_WAIT_SECONDS = 8.0
EXPECTED_SHA256 = bootstrap.EXPECTED_SHA256
EXPECTED_VERSION = bootstrap.EXPECTED_VERSION
_PROCESS_ENUM_STATUS = "not_called"


def _process_path(pid: int) -> Path | None:
    """Read the exact image path through macOS libproc, without process args."""
    if sys.platform != "darwin" or pid <= 1:
        return None
    try:
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        function = libproc.proc_pidpath
        function.argtypes = (ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32)
        function.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        length = function(pid, buffer, len(buffer))
        if length <= 0 or length >= len(buffer):
            return None
        return Path(os.fsdecode(buffer.raw[:length])).resolve(strict=True)
    except (OSError, AttributeError, ValueError):
        return None


def _direct_children(parent_pid: int) -> list[int] | None:
    """Return direct child PIDs from the macOS libproc child list."""
    global _PROCESS_ENUM_STATUS
    _PROCESS_ENUM_STATUS = "not_called"
    if sys.platform != "darwin" or parent_pid <= 1:
        _PROCESS_ENUM_STATUS = "platform_unavailable"
        return None
    try:
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        function = libproc.proc_listchildpids
        function.argtypes = (ctypes.c_int, ctypes.c_void_p, ctypes.c_int)
        function.restype = ctypes.c_int
        buffer = ctypes.create_string_buffer(4096)
        length = function(parent_pid, buffer, len(buffer))
        _PROCESS_ENUM_STATUS = "child_list_empty" if length == 0 else "child_list_returned"
        if length > 0 and length % ctypes.sizeof(ctypes.c_int):
            # Some deployed libproc versions return the number of PIDs for
            # proc_listchildpids, while proc_listpids returns bytes. The
            # subsequent exact parent, executable, and PID checks still gate
            # any signal.
            if length > len(buffer) // ctypes.sizeof(ctypes.c_int):
                _PROCESS_ENUM_STATUS = "child_list_invalid_count"
                return None
            count = length
            values = (ctypes.c_int * count).from_buffer(buffer)
            children = [int(values[index]) for index in range(count)]
            if not all(pid > 1 for pid in children):
                _PROCESS_ENUM_STATUS = "child_list_invalid_pid"
                return None
            _PROCESS_ENUM_STATUS = "child_list_count_parsed"
            return children
        if length == 0:
            list_pids = libproc.proc_listpids
            list_pids.argtypes = (ctypes.c_uint32, ctypes.c_uint32,
                                  ctypes.c_void_p, ctypes.c_int)
            list_pids.restype = ctypes.c_int
            length = list_pids(6, parent_pid, buffer, len(buffer))
            _PROCESS_ENUM_STATUS = "parent_filter_empty" if length == 0 else "parent_filter_returned"
        if length == 0:
            all_pids = libproc.proc_listallpids
            all_pids.argtypes = (ctypes.c_void_p, ctypes.c_int)
            all_pids.restype = ctypes.c_int
            buffer = ctypes.create_string_buffer(256 * 1024)
            length = all_pids(buffer, len(buffer))
            _PROCESS_ENUM_STATUS = "all_processes_empty" if length == 0 else "all_processes_returned"
            if length < 0 or length > len(buffer) // ctypes.sizeof(ctypes.c_int):
                return None
            count = length  # proc_listallpids returns a PID count.
            process_ids = (ctypes.c_int * count).from_buffer(buffer) if count else []
            pidinfo = libproc.proc_pidinfo
            pidinfo.argtypes = (ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                                ctypes.c_void_p, ctypes.c_int)
            pidinfo.restype = ctypes.c_int
            class BsdParent(ctypes.Structure):
                _fields_ = [("flags", ctypes.c_uint32), ("status", ctypes.c_uint32),
                            ("xstatus", ctypes.c_uint32), ("pid", ctypes.c_uint32),
                            ("ppid", ctypes.c_uint32)]
            found: list[int] = []
            for index in range(count):
                pid = int(process_ids[index])
                if pid <= 1:
                    continue
                info_buffer = ctypes.create_string_buffer(136)
                received = pidinfo(pid, 3, 0, info_buffer, len(info_buffer))
                info = BsdParent.from_buffer(info_buffer)
                if received >= ctypes.sizeof(info) and info.pid == pid and info.ppid == parent_pid:
                    found.append(pid)
            _PROCESS_ENUM_STATUS = "all_processes_parsed"
            return found
        if length < 0 or length > len(buffer) or length % ctypes.sizeof(ctypes.c_int):
            _PROCESS_ENUM_STATUS = "child_list_invalid_length"
            return None
        count = length // ctypes.sizeof(ctypes.c_int)
        values = (ctypes.c_int * count).from_buffer(buffer) if count else []
        children = [int(values[index]) for index in range(count)]
        if not all(pid > 1 for pid in children):
            _PROCESS_ENUM_STATUS = "child_list_invalid_pid"
            return None
        _PROCESS_ENUM_STATUS = "child_list_parsed"
        return children
    except (OSError, AttributeError, ValueError, BufferError):
        _PROCESS_ENUM_STATUS = "process_api_failed"
        return None


def _process_state(pid: int, expected_parent: int) -> tuple[str, bool] | None:
    """Return stopped/running class only after exact PID and parent checks."""
    class ProcBsdInfo(ctypes.Structure):
        _fields_ = [
            ("flags", ctypes.c_uint32), ("status", ctypes.c_uint32),
            ("exit_status", ctypes.c_uint32), ("pid", ctypes.c_uint32),
            ("ppid", ctypes.c_uint32), ("uid", ctypes.c_uint32),
            ("gid", ctypes.c_uint32), ("ruid", ctypes.c_uint32),
            ("rgid", ctypes.c_uint32), ("svuid", ctypes.c_uint32),
            ("svgid", ctypes.c_uint32), ("reserved", ctypes.c_uint32),
            ("comm", ctypes.c_char * 16), ("name", ctypes.c_char * 32),
            ("nfiles", ctypes.c_uint32), ("pgid", ctypes.c_uint32),
            ("pjobc", ctypes.c_uint32), ("ttydev", ctypes.c_uint32),
            ("tpgid", ctypes.c_uint32), ("nice", ctypes.c_int32),
            ("start_sec", ctypes.c_uint64), ("start_usec", ctypes.c_uint64),
        ]

    if sys.platform != "darwin" or pid <= 1 or expected_parent <= 1:
        return None
    try:
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        function = libproc.proc_pidinfo
        function.argtypes = (ctypes.c_int, ctypes.c_int, ctypes.c_uint64,
                             ctypes.c_void_p, ctypes.c_int)
        function.restype = ctypes.c_int
        info = ProcBsdInfo()
        length = function(pid, 3, 0, ctypes.byref(info), ctypes.sizeof(info))
    except (OSError, AttributeError, ValueError):
        return None
    if length != ctypes.sizeof(info):
        return None
    if info.pid != pid or info.ppid != expected_parent or info.status == 5:
        return None
    return ("stopped" if info.status == 4 else "running", True)


def _project_root(context: dict[str, Any]) -> Path:
    path = context.get("project")
    if not isinstance(path, Path) or not path.is_dir():
        raise ValueError("fixture project unavailable")
    return path


def _source_record_matches(directory: Path, project: Path, target: str,
                           revision: int, source_id: str) -> bool:
    record = bridge.read_live_managed_binding(directory, target, project, source_id)
    return bool(record is not None and record["revision"] == revision
                and record["provider_session_hash"] == recovery._identifier_hash(source_id)
                and record["provider_version"] == EXPECTED_VERSION)


def _run_worker(binary: Path, executable: str) -> tuple[int, dict[str, Any]]:
    checks: dict[str, bool] = {}
    result: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "process_enumeration_status": "not_called",
        "pinned_provider_hash_matches_launch": False,
        "initial_source_a_durable": False, "owned_tui_child_verified": False,
        "direct_child_count": 0, "matching_tui_child_count": 0,
        "tui_stopped_same_pid": False, "bridge_failure_observed": False,
        "failure_revision_unchanged": False, "prepare_refused_while_stopped": False,
        "serve_alive_while_tui_stopped": False, "same_tui_resumed": False,
        "no_implicit_recovery_after_resume": False,
        "source_a_durable_after_failure": False,
        "historical_source_a_exact": False, "no_user_messages": False,
        "no_prompt_submissions": True, "cleanup_completed": False,
        "fixture_setup_checks": checks,
    }

    def callback(context: dict[str, Any]) -> dict[str, Any]:
        handles: list[bridge.ServeProcess] = []
        directory: Path | None = None
        target: str | None = None
        source_id: str | None = None
        source_revision: int | None = None
        stopped_pid: int | None = None
        stop_sent = False
        try:
            project = _project_root(context)
            result["pinned_provider_hash_matches_launch"] = (
                bootstrap.sha256_file(Path(context["executable"])) == EXPECTED_SHA256)
            if not result["pinned_provider_hash_matches_launch"]:
                result["reason"] = "provider_pin_mismatch"
                return result

            source_id = recovery._session(context, project,
                                          "Panelink bridge failure source A",
                                          "source_a", checks)
            if source_id is None:
                result["reason"] = "source_a_setup_failed"
                return result
            handle, directory, startup = recovery._serve(
                context, binary, Path(context["root"]), source_id, checks)
            result["serve_startup_status"] = startup
            if handle is None or directory is None:
                result["reason"] = "managed_serve_start_failed"
                return result
            handles.append(handle)
            target = recovery._target(handle)
            initial = recovery._wait_initial(
                handle, min(context["deadline"], time.monotonic() + 12.0))
            if target is None or initial is None:
                result["reason"] = "source_a_not_durably_confirmed"
                return result
            source_revision = initial["last_confirmed_revision"]
            result["initial_revision"] = source_revision
            result["initial_source_a_durable"] = (
                source_revision == 1 and recovery._stable_source(
                    handle, binary, directory, project, target, source_revision, source_id,
                    checks=3))
            if not result["initial_source_a_durable"]:
                result["reason"] = "source_a_record_not_stable"
                return result

            # The PTY child is started directly by Panelink's private serve
            # process. Only signal it after exact parent and executable checks.
            children = _direct_children(handle.process.pid)
            result["process_enumeration_status"] = _PROCESS_ENUM_STATUS
            executable_path = Path(context["executable"]).resolve(strict=True)
            result["direct_child_count"] = min(len(children or ()), 8)
            if children is None or not children:
                result["reason"] = "owned_tui_child_ambiguous"
                return result
            matching = [candidate for candidate in children
                        if _process_path(candidate) == executable_path
                        and _process_state(candidate, handle.process.pid) is not None]
            result["matching_tui_child_count"] = min(len(matching), 8)
            if len(matching) != 1:
                result["reason"] = "owned_tui_child_identity_unverified"
                return result
            candidate = matching[0]
            stopped_pid = candidate
            result["owned_tui_child_verified"] = True
            os.kill(stopped_pid, signal.SIGSTOP)
            stop_sent = True
            stop_deadline = min(context["deadline"], time.monotonic() + LEASE_WAIT_SECONDS)
            while time.monotonic() < stop_deadline:
                observed = _process_state(stopped_pid, handle.process.pid)
                if observed is None or _process_path(stopped_pid) != executable_path:
                    result["reason"] = "owned_tui_identity_changed_while_stopped"
                    return result
                if observed[0] == "stopped":
                    result["tui_stopped_same_pid"] = True
                    break
                time.sleep(0.05)
            if not result["tui_stopped_same_pid"]:
                result["reason"] = "tui_stop_not_observed"
                return result
            result["serve_alive_while_tui_stopped"] = handle.process.poll() is None
            result["stopped_pid_sha256"] = hashlib.sha256(
                str(stopped_pid).encode("ascii")).hexdigest()

            failure_deadline = min(context["deadline"],
                                   time.monotonic() + LEASE_WAIT_SECONDS)
            observed_status = None
            while time.monotonic() < failure_deadline:
                handle.poll(0.05)
                latest = handle.latest
                if latest is not None and latest["status"] in {"stale", "unavailable"}:
                    observed_status = latest
                    break
            result["failure_status"] = observed_status["status"] if observed_status else None
            result["bridge_failure_observed"] = bool(observed_status)
            result["failure_revision_unchanged"] = bool(
                observed_status is not None
                and observed_status["last_confirmed_revision"] == source_revision
                and handle.process.poll() is None)
            if not result["bridge_failure_observed"]:
                result["reason"] = "bridge_failure_status_not_observed"
                return result

            # A valid same-project candidate must still be refused while the
            # provider UI is paused. Its ID remains private in this worker.
            candidate_id = recovery._session(
                context, project, "Panelink bridge failure choice B", "choice_b", checks)
            if candidate_id is None:
                result["reason"] = "choice_b_setup_failed"
                return result
            prepared = recovery._cli(binary, "prepare", directory, project, target,
                                     session_id=candidate_id, timeout=11.0)
            result["prepare_refused_while_stopped"] = bool(
                prepared.get("valid") is True and prepared.get("returncode") == 1
                and ((prepared.get("state") == "manual_selection_required"
                      and prepared.get("error") in {
                          "provider_choice_unverified", "provider_recovery_unavailable",
                      }) or prepared.get("error") == "provider_recovery_unavailable")
                and prepared.get("automatic_resume") is False)
            result["prepare_while_stopped_state"] = prepared.get("state")
            result["prepare_while_stopped_error"] = prepared.get("error")
            result["source_a_durable_after_failure"] = _source_record_matches(
                directory, project, target, source_revision, source_id)
            if not (result["prepare_refused_while_stopped"]
                    and result["failure_revision_unchanged"]
                    and result["source_a_durable_after_failure"]):
                result["reason"] = "failure_did_not_fail_closed"
                return result

            # Resume precisely the same verified TUI PID; do not restart the
            # provider or reconnect the source by creating a new process.
            os.kill(stopped_pid, signal.SIGCONT)
            stop_sent = False
            resume_deadline = min(context["deadline"], time.monotonic() + 3.0)
            while time.monotonic() < resume_deadline:
                observed = _process_state(stopped_pid, handle.process.pid)
                if observed is not None and observed[0] == "running":
                    result["same_tui_resumed"] = _process_path(stopped_pid) == executable_path
                    break
                time.sleep(0.05)
            result["serve_alive_after_resume"] = handle.process.poll() is None
            if not result["same_tui_resumed"] or not result["serve_alive_after_resume"]:
                result["reason"] = "same_tui_resume_not_verified"
                return result

            # Let the resumed UI emit any unsolicited traffic, then confirm
            # that owner failure stayed latched and no new binding was saved.
            quiet_deadline = min(context["deadline"], time.monotonic() + 2.0)
            while time.monotonic() < quiet_deadline:
                handle.poll(0.05)
            latest = handle.latest
            after_resume = recovery._cli(binary, "status", directory, project, target,
                                         timeout=6.0)
            result["status_after_resume_state"] = after_resume.get("state")
            result["status_after_resume_error"] = after_resume.get("error")
            prepare_after_resume = recovery._cli(
                binary, "prepare", directory, project, target,
                session_id=candidate_id, timeout=6.0)
            result["prepare_after_resume_state"] = prepare_after_resume.get("state")
            result["prepare_after_resume_error"] = prepare_after_resume.get("error")
            result["no_implicit_recovery_after_resume"] = bool(
                handle.process.poll() is None
                and latest is not None
                and latest["status"] in {"stale", "unavailable"}
                and latest["last_confirmed_revision"] == source_revision
                and after_resume.get("state") != "prepared_choice_saved"
                and after_resume.get("state") != "selected_and_saved"
                and prepare_after_resume.get("state") != "prepared_choice_saved"
                and prepare_after_resume.get("state") != "selected_and_saved")
            result["source_a_durable_after_resume"] = _source_record_matches(
                directory, project, target, source_revision, source_id)
            if not (result["no_implicit_recovery_after_resume"]
                    and result["source_a_durable_after_resume"]):
                result["reason"] = "failure_cleared_without_explicit_recovery"
                return result

            result["no_user_messages"] = recovery._no_user_messages(
                context, [source_id, candidate_id])
            result["no_prompt_submissions"] = result["no_user_messages"]
            result["classification"] = "proven"
            result["reason"] = "owned_tui_failure_latched_and_source_preserved"
            return result
        finally:
            if stop_sent and stopped_pid is not None:
                try:
                    os.kill(stopped_pid, signal.SIGCONT)
                except OSError:
                    pass
            result["cleanup_completed"] = all(
                handle.close(min(context["deadline"], time.monotonic() + 6.0))
                for handle in reversed(handles))
            child_reaped = stopped_pid is None
            if stopped_pid is not None:
                try:
                    os.kill(stopped_pid, 0)
                except ProcessLookupError:
                    child_reaped = True
                except OSError:
                    child_reaped = False
                else:
                    executable_path = Path(context["executable"]).resolve(strict=True)
                    if _process_path(stopped_pid) == executable_path:
                        try:
                            os.kill(stopped_pid, signal.SIGTERM)
                        except OSError:
                            pass
                        reap_deadline = time.monotonic() + 1.0
                        while time.monotonic() < reap_deadline:
                            try:
                                os.kill(stopped_pid, 0)
                            except ProcessLookupError:
                                child_reaped = True
                                break
                            except OSError:
                                break
                            time.sleep(0.05)
                        if not child_reaped and _process_path(stopped_pid) == executable_path:
                            try:
                                os.kill(stopped_pid, signal.SIGKILL)
                            except OSError:
                                pass
                            try:
                                os.kill(stopped_pid, 0)
                            except ProcessLookupError:
                                child_reaped = True
                    result["cleanup_completed"] = result["cleanup_completed"] and child_reaped
            if directory is not None and target is not None and source_id is not None:
                historical = bridge.historical_binding(binary, directory,
                                                       _project_root(context), target)
                result["historical_source_a_exact"] = bool(
                    historical is not None and historical["managed_launch_linked"]
                    and historical["provider_session_hash"] == recovery._identifier_hash(source_id)
                    and historical["provider_version"] == EXPECTED_VERSION
                    and historical["binding_revision"] == source_revision
                    and historical["automatic_resume"] is False
                    and historical["current_at_crash"] is False)
            if (result.get("classification") == "proven"
                    and (not result["cleanup_completed"]
                         or not result["historical_source_a_exact"]
                         or not result["no_user_messages"])):
                result.update(classification="blocked", reason="cleanup_or_history_gate_failed")

    bootstrap.TOTAL_DEADLINE_SECONDS = MAX_SECONDS
    tempfile.tempdir = "/private/tmp"
    original_temporary_directory = tempfile.TemporaryDirectory

    def short_private_tempdir(*_args, **_kwargs):
        return original_temporary_directory(prefix="p4f-", dir="/private/tmp")

    tempfile.TemporaryDirectory = short_private_tempdir
    try:
        outcome = bootstrap._run_worker(
            executable, "opencode", after_bootstrap=callback,
            total_deadline_seconds=MAX_SECONDS, scenario="bridge-failure")
    except (OSError, subprocess.SubprocessError, TimeoutError):
        outcome = {"classification": "blocked", "reason": "fixture_worker_failed",
                   "cleanup_completed": False}
    nested = outcome.get("tui") if isinstance(outcome, dict) else None
    if isinstance(nested, dict):
        result.update(nested)
    result["cleanup_completed"] = bool(
        result.get("cleanup_completed") and outcome.get("cleanup_completed"))
    return (0 if result.get("classification") == "proven"
            and result["cleanup_completed"] else 2), result


def _public(report: dict[str, Any]) -> dict[str, Any]:
    allowed = {
        "classification", "reason", "pinned_provider_hash_matches_launch",
        "initial_source_a_durable", "owned_tui_child_verified", "tui_stopped_same_pid",
        "direct_child_count", "matching_tui_child_count",
        "process_enumeration_status",
        "bridge_failure_observed", "failure_status", "failure_revision_unchanged",
        "prepare_refused_while_stopped", "prepare_while_stopped_state",
        "prepare_while_stopped_error", "serve_alive_while_tui_stopped",
        "same_tui_resumed", "serve_alive_after_resume", "no_implicit_recovery_after_resume",
        "status_after_resume_state", "status_after_resume_error",
        "prepare_after_resume_state", "prepare_after_resume_error",
        "source_a_durable_after_failure", "source_a_durable_after_resume",
        "historical_source_a_exact", "no_user_messages", "no_prompt_submissions",
        "cleanup_completed", "initial_revision", "failure_revision", "fork_history_hash_verified",
        "pinned_opencode_version", "pinned_opencode_hash_match", "panelink_binary_hash",
        "sandbox", "network_scope", "serve_startup_status", "fixture_setup_checks",
        "bootstrap_reason", "bootstrap_classification", "bootstrap_prompt_submissions_zero",
    }
    return {key: report[key] for key in allowed if key in report}


def run(panelink_binary: str, executable: str | None = None) -> tuple[int, dict[str, Any]]:
    chosen, launcher = bootstrap.resolve_executable(executable)
    binary = Path(panelink_binary).resolve()
    report: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "pinned_opencode_version": EXPECTED_VERSION,
        "pinned_opencode_hash_match": False,
        "panelink_binary_hash": None,
        "sandbox": "not_started", "network_scope": "fixture_loopback_only",
        "no_prompt_submissions": True, "cleanup_completed": False,
    }
    if chosen is None or launcher != "opencode":
        report["reason"] = "native_opencode_missing"
        return 2, _public(report)
    if not binary.is_file() or not os.access(binary, os.X_OK):
        report["reason"] = "panelink_binary_unavailable"
        return 2, _public(report)
    report["panelink_binary_hash"] = bridge.sha256_file(binary)
    report["pinned_opencode_hash_match"] = bootstrap.sha256_file(Path(chosen)) == EXPECTED_SHA256
    if not report["pinned_opencode_hash_match"]:
        report["reason"] = "pinned_opencode_hash_mismatch"
        return 2, _public(report)
    if sys.platform != "darwin" or shutil.which("sandbox-exec") is None:
        report["reason"] = "macos_loopback_sandbox_unavailable"
        return 2, _public(report)
    cwd = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="panelink-bridge-failure-sandbox-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        profile = root / "loopback.sb"
        profile.write_text(bridge.MANAGED_SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C",
               "TMPDIR": "/private/tmp", bootstrap.WORKER_ENV: "1"}
        argv = [shutil.which("sandbox-exec") or "sandbox-exec", "-f", str(profile),
                sys.executable, str(Path(__file__).resolve()), "--sandbox-worker",
                str(binary), chosen]
        code, raw, sandbox_error = bootstrap.bounded_worker(
            argv, cwd, env, timeout=MAX_SECONDS + 5.0)
    try:
        parsed = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        parsed = {"classification": "blocked",
                  "reason": sandbox_error or "worker_output_invalid"}
    if isinstance(parsed, dict):
        nested = parsed.get("tui")
        if isinstance(nested, dict):
            report.update(_public(nested))
        else:
            report.update(_public(parsed))
        bootstrap_reason = parsed.get("reason")
        report["bootstrap_reason"] = (bootstrap_reason if isinstance(bootstrap_reason, str)
                                       and re.fullmatch(r"[a-z0-9_]{1,64}", bootstrap_reason)
                                       else "unclassified")
        report["bootstrap_classification"] = parsed.get("classification")
        report["bootstrap_prompt_submissions_zero"] = (
            parsed.get("prompt_submissions") == 0 or parsed.get("no_prompt_submissions") is True)
        report["no_prompt_submissions"] = report["bootstrap_prompt_submissions_zero"]
        report["cleanup_completed"] = bool(
            parsed.get("cleanup_completed") is True
            and (not isinstance(nested, dict) or nested.get("cleanup_completed") is True))
    report["sandbox"] = "not_enforced" if sandbox_error else "enforced_loopback_only"
    if sandbox_error:
        report.update(classification="blocked", reason="sandbox_worker_failed")
    code = 0 if code == 0 and report.get("classification") == "proven" \
        and report.get("cleanup_completed") else 2
    return code, _public(report)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--panelink-binary", default="/private/tmp/panelink-p4-bridge-store-target/debug/panelink")
    parser.add_argument("--opencode")
    parser.add_argument("--sandbox-worker", nargs=2, metavar=("PANELINK", "OPENCODE"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.sandbox_worker:
        if os.environ.get(bootstrap.WORKER_ENV) != "1":
            print(json.dumps({"classification": "blocked", "reason": "sandbox_worker_marker_missing"}))
            return 2
        binary, executable = Path(args.sandbox_worker[0]), args.sandbox_worker[1]
        code, report = _run_worker(binary, executable)
    else:
        code, report = run(args.panelink_binary, args.opencode)
    print(json.dumps(_public(report), sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
