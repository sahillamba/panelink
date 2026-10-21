#!/usr/bin/env python3
"""Probe OpenCode's selected TUI session identity in an isolated private PTY.

The probe creates only empty local sessions. It never submits prompt text and
prints only an allowlisted summary containing counts, status labels and hashes.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import selectors
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time
from typing import Any
import fcntl

try:
    from scripts.probe_opencode_identity import (
        _kill_group,
        bounded_command,
        environment,
        free_loopback_port,
        parse_version,
        request,
        resolve_executable,
        session_id,
    )
except ModuleNotFoundError:
    from probe_opencode_identity import (
        _kill_group,
        bounded_command,
        environment,
        free_loopback_port,
        parse_version,
        request,
        resolve_executable,
        session_id,
    )


REPETITIONS = 3
EXPERIMENT_DEADLINE = 60.0
SERVER_DEADLINE = 18.0
PTY_SIZE = (24, 100)
MAX_RECORD_BYTES = 2 * 1024 * 1024
MAX_PTY_BYTES = 512 * 1024
POLL_INTERVAL_SECONDS = 0.1
SWITCH_DEADLINE_SECONDS = 5.0
ANSI_ESCAPE = re.compile(rb"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
SESSION_HASH = re.compile(r"^[0-9a-f]{64}$")


def launcher_argv(executable: str, launcher: str, arguments: list[str]) -> list[str]:
    """Use OpenCode argv directly, or pass raw argv through the Tesco wrapper."""
    if launcher == "tescode":
        return [executable, "workflow", "custom", *arguments]
    return [executable, *arguments]


def hash_session(identifier: str) -> str:
    return hashlib.sha256(identifier.encode("utf-8")).hexdigest()


def parse_records(payload: bytes) -> tuple[list[dict[str, Any]], int]:
    """Parse bounded plugin JSONL; malformed, oversized and secret-shaped rows are dropped."""
    if len(payload) > MAX_RECORD_BYTES:
        payload = payload[-MAX_RECORD_BYTES:]
    records: list[dict[str, Any]] = []
    rejected = 0
    previous_seq = -1
    for raw in payload.splitlines():
        if len(raw) > 4096:
            rejected += 1
            continue
        try:
            item = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            rejected += 1
            continue
        if not isinstance(item, dict):
            rejected += 1
            continue
        seq = item.get("seq")
        monotonic_ms = item.get("monotonic_ms")
        route_name = item.get("route_name")
        session_hash = item.get("session_id_hash")
        callback_registered = item.get("callback_registered", False)
        if (not isinstance(seq, int) or isinstance(seq, bool) or seq <= previous_seq
                or not isinstance(monotonic_ms, (int, float))
                or isinstance(monotonic_ms, bool) or monotonic_ms < 0
                or not isinstance(route_name, str) or len(route_name) > 80
                or not isinstance(callback_registered, bool)
                or (session_hash is not None and
                    (not isinstance(session_hash, str) or not SESSION_HASH.fullmatch(session_hash)))):
            rejected += 1
            continue
        if any(key in item for key in ("session_id", "sessionID", "id", "body", "text", "prompt")):
            rejected += 1
            continue
        previous_seq = seq
        records.append({
            "seq": seq,
            "monotonic_ms": float(monotonic_ms),
            "route_name": route_name,
            "session_id_hash": session_hash,
            "kind": item.get("kind", "poll") if item.get("kind", "poll") in
            ("poll", "plugin_loaded", "route_change_callback") else "poll",
            "callback_registered": callback_registered,
        })
    return records, rejected


def correlate_switches(
    records: list[dict[str, Any]],
    initial_hash: str,
    target_hashes: list[str],
    issued: list[dict[str, float | int]],
    *,
    max_latency_ms: float = 5000.0,
) -> dict[str, Any]:
    """Match each controlled route transition to a later plugin observation."""
    session_records = [record for record in records if record["route_name"] == "session"]
    callback_records = [record for record in records
                        if record.get("kind") == "route_change_callback"]
    initial = next((record for record in session_records
                    if record["session_id_hash"] == initial_hash), None)
    transitions: list[dict[str, Any]] = []
    previous_hash = initial_hash
    last_seq = int(initial["seq"]) if initial else -1
    for index, target in enumerate(target_hashes):
        marker = issued[index] if index < len(issued) else {"after_seq": last_seq, "issued_ms": 0.0}
        after_seq = int(marker.get("after_seq", last_seq))
        issued_ms = float(marker.get("issued_ms", 0.0))
        later = [record for record in session_records if int(record["seq"]) > after_seq]
        later = [record for record in later
                 if float(record["monotonic_ms"]) >= issued_ms]
        matched = next((record for record in later if record["session_id_hash"] == target), None)
        extras = []
        if matched:
            extras = [record["session_id_hash"] for record in later
                      if int(record["seq"]) < int(matched["seq"])
                      and record["session_id_hash"] not in (previous_hash, target)]
            latency = float(matched["monotonic_ms"]) - issued_ms
            last_seq = int(matched["seq"])
        else:
            latency = None
        transitions.append({
            "observed": matched is not None,
            "latency_ms": round(latency, 3) if latency is not None else None,
            "within_latency_bound": latency is not None and 0 <= latency <= max_latency_ms,
            "extra_transition_count": len(extras),
        })
        if matched:
            previous_hash = target
    route_changes: list[str] = []
    for record in session_records:
        value = record["session_id_hash"]
        if value and (not route_changes or route_changes[-1] != value):
            route_changes.append(value)
    callback_sequence = [record["session_id_hash"] for record in callback_records
                         if record["session_id_hash"]]
    callback_sequence_ok = not callback_sequence or callback_sequence == target_hashes
    expected = [initial_hash, *target_hashes]
    expected_index = 0
    extra_transition_count = 0
    for observed_hash in route_changes:
        if expected_index < len(expected) and observed_hash == expected[expected_index]:
            expected_index += 1
        elif observed_hash not in expected or expected_index >= len(expected):
            extra_transition_count += 1
    missed_transition_count = max(0, len(expected) - expected_index - (0 if initial else 1))
    switch_order_observed = (initial is not None and expected_index == len(expected)
                             and callback_sequence_ok)
    latency_ok = all(transition["within_latency_bound"] for transition in transitions)
    no_extras = extra_transition_count == 0 and all(
        transition["extra_transition_count"] == 0 for transition in transitions
    )
    return {
        "initial_route_captured": initial is not None,
        "switch_A_B_A_observed": switch_order_observed and missed_transition_count == 0,
        "latency_bounds_passed": latency_ok,
        "no_extra_transitions": extra_transition_count == 0,
        "missed_transition_count": missed_transition_count,
        "extra_transition_count": extra_transition_count,
        "transitions": transitions,
    }


def classify_repetition(
    *,
    plugin_loaded: bool,
    initial_route_captured: bool,
    switch_observed: bool,
    latency_bounds_passed: bool,
    blocked_reason: str | None,
    missed_transition_count: int = 0,
    extra_transition_count: int = 0,
) -> str:
    if blocked_reason:
        return "blocked"
    if (plugin_loaded and initial_route_captured and switch_observed and latency_bounds_passed
            and missed_transition_count == 0 and extra_transition_count == 0):
        return "proven"
    return "not_proven"


def read_records(path: Path) -> tuple[list[dict[str, Any]], int]:
    try:
        payload = path.read_bytes()
    except OSError:
        return [], 0
    return parse_records(payload)


class PrivatePty:
    """A fixed-size PTY child with bounded output and unconditional group cleanup."""

    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str]):
        self.master, slave = pty.openpty()
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", *PTY_SIZE, 0, 0))
        self.child: subprocess.Popen[bytes] | None = None
        self.output = bytearray()
        self.selector = selectors.DefaultSelector()
        try:
            self.child = subprocess.Popen(
                argv,
                cwd=cwd,
                env=env,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                close_fds=True,
                shell=False,
                start_new_session=True,
                bufsize=0,
            )
        finally:
            os.close(slave)
        os.set_blocking(self.master, False)
        self.selector.register(self.master, selectors.EVENT_READ)

    def send(self, payload: bytes) -> None:
        view = memoryview(payload)
        while view:
            written = os.write(self.master, view)
            view = view[written:]

    def drain(self, timeout: float = 0.0) -> bytes:
        chunks = bytearray()
        for _key, _mask in self.selector.select(timeout):
            try:
                while True:
                    chunk = os.read(self.master, 8192)
                    if not chunk:
                        break
                    chunks.extend(chunk)
                    if len(self.output) + len(chunks) >= MAX_PTY_BYTES:
                        allowance = max(0, MAX_PTY_BYTES - len(self.output))
                        del chunks[allowance:]
                        break
            except (BlockingIOError, OSError):
                pass
        self.output.extend(chunks)
        return bytes(chunks)

    def recent_text(self, payload: bytes) -> str:
        return ANSI_ESCAPE.sub(b"", payload).decode("utf-8", "replace")

    def alive(self) -> bool:
        return self.child is not None and self.child.poll() is None

    def osc_title_seen(self, title: str) -> bool:
        payload = bytes(self.output)
        return (b"\x1b]0;OC | " + title.encode("ascii")) in payload or (
            b"\x1b]2;OC | " + title.encode("ascii")
        ) in payload

    def close(self) -> None:
        if self.child is not None:
            _kill_group(self.child)
            try:
                self.child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                _kill_group(self.child)
                self.child.wait(timeout=1)
        self.selector.close()
        try:
            os.close(self.master)
        except OSError:
            pass


def plugin_source() -> Path:
    return Path(__file__).resolve().parent.parent / "docs" / "progress" / "p4-tui-plugin-fixture" / "plugin.mjs"


def prepare_fixture(root: Path, records_path: Path) -> Path:
    project = root / "project"
    config_dir = project / ".opencode" / "plugins"
    config_dir.mkdir(mode=0o700, parents=True)
    config_path = project / ".opencode" / "tui.json"
    fixture_path = config_dir / "panelink-tui-probe.js"
    shutil.copyfile(plugin_source(), fixture_path)
    fixture_path.chmod(0o600)
    config = {"plugin": [str(fixture_path)]}
    config_path.write_text(json.dumps(config), encoding="utf-8")
    config_path.chmod(0o600)
    records_path.touch(mode=0o600)
    records_path.chmod(0o600)
    return fixture_path


def safe_deadline(deadline: float) -> bool:
    return time.monotonic() < deadline


def wait_until(
    predicate,
    *,
    deadline: float,
    pty_child: PrivatePty | None = None,
    interval: float = 0.05,
) -> bool:
    while safe_deadline(deadline):
        if predicate():
            return True
        if pty_child is not None:
            if not pty_child.alive():
                return False
            pty_child.drain(min(interval, max(0.0, deadline - time.monotonic())))
        else:
            time.sleep(min(interval, max(0.0, deadline - time.monotonic())))
    return bool(predicate())


def rep_result(executable: str, launcher: str, repetition: int) -> dict[str, Any]:
    started = time.monotonic()
    deadline = started + EXPERIMENT_DEADLINE
    summary: dict[str, Any] = {
        "repetition": repetition,
        "status": "blocked",
        "reason": "not_started",
        "plugin_loaded": False,
        "initial_route_captured": False,
        "switch_A_B_A_observed": False,
        "latency_bounds_passed": False,
        "missed_transition_count": 0,
        "extra_transition_count": 0,
        "route_change_callback_api": "candidate_tui.session.select_listener",
        "route_change_callback_observed_count": 0,
        "new_session": "not_attempted",
        "sessions_created_count": 0,
        "transitions": [],
        "elapsed_ms": 0,
    }
    child_server: subprocess.Popen[bytes] | None = None
    tui: PrivatePty | None = None
    with tempfile.TemporaryDirectory(prefix="panelink-tui-identity-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        env = environment(root)
        env.update({"TERM": "xterm-256color", "PANELINK_PROBE_RECORDS": str(root / "route-records.jsonl")})
        project = root / "project"
        project.mkdir(mode=0o700)
        records_path = root / "route-records.jsonl"
        prepare_fixture(root, records_path)
        port = free_loopback_port()
        if port is None:
            summary["reason"] = "loopback_bind_unavailable"
            return finish_rep(summary, started)
        base = f"http://127.0.0.1:{port}"
        try:
            server_log = (root / "server.log").open("wb")
        except OSError:
            summary["reason"] = "private_server_log_unavailable"
            return finish_rep(summary, started)
        try:
            server_argv = launcher_argv(
                executable, launcher,
                ["serve", "--hostname", "127.0.0.1", "--port", str(port)],
            )
            child_server = subprocess.Popen(
                server_argv, cwd=project, env=env, stdin=subprocess.DEVNULL,
                stdout=server_log, stderr=server_log,
                close_fds=True, shell=False, start_new_session=True,
            )
        except OSError:
            summary["reason"] = "server_launch_failed"
            return finish_rep(summary, started)
        try:
            server_status: list[int | None] = [None]

            def server_ready() -> bool:
                server_status[0] = request(base, "GET", "/session", timeout=0.35)[0]
                return server_status[0] == 200

            ready = wait_until(
                server_ready,
                deadline=min(deadline, time.monotonic() + SERVER_DEADLINE),
            )
            if not ready:
                summary["reason"] = "local_server_readiness_timeout"
                return finish_rep(summary, started)
            ids: list[str] = []
            for title in ("Panelink Identity Probe A", "Panelink Identity Probe B"):
                if not safe_deadline(deadline):
                    summary["reason"] = "experiment_deadline"
                    return finish_rep(summary, started)
                code, body = request(base, "POST", "/session", b"{}")
                identifier = session_id(body) if code == 200 else None
                if identifier is None:
                    summary["reason"] = "empty_local_sessions_not_created"
                    summary["session_create_status"] = f"http_{code}" if code is not None else "unavailable"
                    return finish_rep(summary, started)
                ids.append(identifier)
                renamed, _ = request(
                    base, "PATCH", f"/session/{identifier}", json.dumps({"title": title}).encode(),
                )
                if renamed not in (200, 204):
                    summary["session_titles"] = "not_set"
                    summary["reason"] = "empty_sessions_cannot_be_identified_safely"
                    return finish_rep(summary, started)
            hashes = [hash_session(identifier) for identifier in ids]
            summary["sessions_created_count"] = len(hashes)
            tui_argv = launcher_argv(
                executable, launcher, ["--print-logs", "--session", ids[0], str(project)],
            )
            try:
                tui = PrivatePty(tui_argv, project, env)
            except (OSError, subprocess.SubprocessError):
                summary["reason"] = "private_pty_launch_failed"
                return finish_rep(summary, started)
            def records_now() -> tuple[list[dict[str, Any]], int]:
                return read_records(records_path)

            loaded = wait_until(
                lambda: any(record["kind"] == "plugin_loaded" for record in records_now()[0]),
                deadline=min(deadline, time.monotonic() + 12.0), pty_child=tui,
            )
            records, rejected = records_now()
            summary["record_rejected_count"] = rejected
            summary["plugin_loaded"] = loaded
            safe_output = tui.recent_text(bytes(tui.output)) if tui is not None else ""
            summary["pty_output_bytes"] = len(tui.output) if tui is not None else 0
            summary["tui_process_alive_after_plugin_wait"] = tui.alive() if tui is not None else False
            summary["plugin_error_log_seen"] = "[tui.plugin]" in safe_output
            summary["local_plugin_path_seen"] = "panelink-tui-probe.js" in safe_output
            summary["plugin_module_name_seen"] = "panelink-tui-probe" in safe_output
            summary["auth_prompt_seen"] = bool(re.search(
                r"no providers available|connect (?:a |your )?provider|(?:provider|api key).{0,40}(?:connect|missing|required)",
                safe_output.lower(),
            ))
            summary["route_change_callback_observed_count"] = sum(
                record["kind"] == "route_change_callback" for record in records
            )
            summary["route_change_callback_api"] = (
                "tui.session.select_listener_registered" if any(
                    record["callback_registered"] for record in records
                ) else "candidate_listener_not_observed"
            )
            summary["initial_route_captured"] = any(
                record["route_name"] == "session" and record["session_id_hash"] == hashes[0]
                for record in records
            )
            if not loaded:
                navigation = []
                for label in ("B", "A"):
                    title = f"Panelink Identity Probe {label}"
                    tui.output.clear()
                    try:
                        tui.send(b"\x18l")
                    except OSError:
                        navigation.append({"picker_visible": False, "title_observed": False})
                        break
                    picker_buffer = bytearray()
                    picker_until = min(deadline, time.monotonic() + 2.0)
                    while safe_deadline(picker_until) and tui.alive():
                        picker_buffer.extend(tui.drain(0.05))
                        visible = tui.recent_text(bytes(picker_buffer))
                        if all(candidate in visible for candidate in
                               ("Panelink Identity Probe A", "Panelink Identity Probe B")):
                            break
                    picker_visible = all(candidate in tui.recent_text(bytes(picker_buffer))
                                         for candidate in
                                         ("Panelink Identity Probe A", "Panelink Identity Probe B"))
                    if not picker_visible:
                        navigation.append({"picker_visible": False, "title_observed": False})
                        break
                    try:
                        tui.send(title.encode("ascii"))
                        time.sleep(0.12)
                        tui.drain(0.0)
                        tui.send(b"\r")
                    except OSError:
                        navigation.append({"picker_visible": True, "title_observed": False})
                        break
                    title_until = min(deadline, time.monotonic() + SWITCH_DEADLINE_SECONDS)
                    while safe_deadline(title_until) and tui.alive() and not tui.osc_title_seen(title):
                        tui.drain(0.05)
                    navigation.append({
                        "picker_visible": True,
                        "title_observed": tui.osc_title_seen(title),
                    })
                    if not navigation[-1]["title_observed"]:
                        break
                summary["navigation_attempts"] = navigation
                summary["route_change_callback_api"] = "navigation_attempted_plugin_load_not_observed"
                if len(navigation) == 2 and all(item["title_observed"] for item in navigation):
                    try:
                        tui.send(b"\x18n")
                        time.sleep(0.2)
                        tui.drain(0.0)
                        tui.send(b"\x1b")
                        summary["new_session"] = "empty_new_session_key_attempted_no_prompt"
                    except OSError:
                        summary["new_session"] = "attempt_failed"
                if summary.get("auth_prompt_seen"):
                    summary["reason"] = "tui_requires_provider_auth"
                elif summary.get("plugin_error_log_seen"):
                    summary["reason"] = "plugin_load_error"
                else:
                    summary["reason"] = "tui_plugin_not_loaded"
                return finish_rep(summary, started)
            if not summary["initial_route_captured"]:
                summary["reason"] = "initial_session_route_not_captured"
                return finish_rep(summary, started)

            issued: list[dict[str, float | int]] = []
            targets = [hashes[1], hashes[0]]
            labels = ["B", "A"]
            previous = hashes[0]
            for target_hash, label in zip(targets, labels):
                if not safe_deadline(deadline):
                    summary["reason"] = "experiment_deadline"
                    break
                pre_action_records, _ = records_now()
                after_seq = max((int(record["seq"]) for record in pre_action_records), default=-1)
                last_record_ms = max(
                    (float(record["monotonic_ms"]) for record in pre_action_records),
                    default=0.0,
                )
                tui.output.clear()
                # Installed 1.19.0 defaults: Ctrl-X is the leader and L opens session list.
                tui.send(b"\x18")
                time.sleep(0.12)
                tui.send(b"l")
                dialog_text = bytearray()
                dialog_visible = False
                dialog_deadline = min(deadline, time.monotonic() + 2.5)
                while safe_deadline(dialog_deadline) and tui.alive():
                    chunk = tui.drain(0.05)
                    dialog_text.extend(chunk)
                    rendered = tui.recent_text(bytes(dialog_text))
                    if ("Panelink Identity Probe A" in rendered and
                            "Panelink Identity Probe B" in rendered):
                        dialog_visible = True
                        break
                if not dialog_visible:
                    summary["reason"] = "session_list_not_safely_identified"
                    break
                query = f"Panelink Identity Probe {label}".encode("ascii")
                tui.send(query)
                time.sleep(0.12)
                tui.drain(0.0)
                issued_ms = last_record_ms
                tui.send(b"\r")
                marker = {"after_seq": after_seq, "issued_ms": issued_ms}
                issued.append(marker)
                expected_seen = wait_until(
                    lambda target_hash=target_hash: any(
                        int(record["seq"]) > after_seq
                        and record["route_name"] == "session"
                        and record["session_id_hash"] == target_hash
                        for record in records_now()[0]
                    ),
                    deadline=min(deadline, time.monotonic() + SWITCH_DEADLINE_SECONDS),
                    pty_child=tui,
                )
                summary[f"transition_{previous[-8:]}_{target_hash[-8:]}"] = "observed" if expected_seen else "not_observed"
                previous = target_hash
                if not expected_seen:
                    summary["reason"] = "controlled_session_selection_not_observed"
                    break

            records, rejected = records_now()
            summary["record_rejected_count"] = rejected
            correlated = correlate_switches(records, hashes[0], targets, issued)
            summary.update(correlated)
            summary["sessions_created_count"] = len(hashes)

            # The documented default Ctrl-X then N creates an empty session and makes no model call.
            if correlated["switch_A_B_A_observed"] and safe_deadline(deadline) and tui.alive():
                before_new, _ = records_now()
                after_seq = max((int(record["seq"]) for record in before_new), default=-1)
                tui.send(b"\x18n")
                new_seen = wait_until(
                    lambda: any(
                        int(record["seq"]) > after_seq and record["route_name"] == "session"
                        and record["session_id_hash"] not in hashes
                        for record in records_now()[0]
                    ),
                    deadline=min(deadline, time.monotonic() + 3.0),
                    pty_child=tui,
                )
                summary["new_session"] = "empty_session_route_observed" if new_seen else "not_observed"
            else:
                summary["new_session"] = "skipped_switch_not_proven"
            summary["reason"] = summary.get("reason") if summary.get("reason") not in (None, "not_started") else "completed_observations"
            summary["status"] = classify_repetition(
                plugin_loaded=summary["plugin_loaded"],
                initial_route_captured=summary["initial_route_captured"],
                switch_observed=summary["switch_A_B_A_observed"],
                latency_bounds_passed=summary["latency_bounds_passed"],
                blocked_reason=summary["reason"] if summary["reason"] not in
                ("completed_observations", "controlled_session_selection_not_observed") else None,
                missed_transition_count=summary["missed_transition_count"],
                extra_transition_count=summary["extra_transition_count"],
            )
            if summary["reason"] == "controlled_session_selection_not_observed":
                summary["status"] = "not_proven"
            return finish_rep(summary, started)
        finally:
            if tui is not None:
                tui.close()
            if child_server is not None:
                _kill_group(child_server)
                try:
                    child_server.wait(timeout=1)
                except subprocess.TimeoutExpired:
                    _kill_group(child_server)
                    child_server.wait(timeout=1)
            server_log.close()
            summary["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)


def finish_rep(summary: dict[str, Any], started: float) -> dict[str, Any]:
    summary["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)
    if summary["reason"] == "not_started":
        summary["reason"] = "completed_observations"
    if summary["status"] == "blocked" and summary["reason"] != "completed_observations":
        summary["status"] = "blocked"
    elif summary["status"] == "blocked":
        summary["status"] = classify_repetition(
            plugin_loaded=summary["plugin_loaded"],
            initial_route_captured=summary["initial_route_captured"],
            switch_observed=summary["switch_A_B_A_observed"],
            latency_bounds_passed=summary["latency_bounds_passed"],
            blocked_reason=None,
            missed_transition_count=summary["missed_transition_count"],
            extra_transition_count=summary["extra_transition_count"],
        )
    return summary


def run(executable: str | None = None) -> dict[str, Any]:
    chosen, launcher = resolve_executable(executable)
    report: dict[str, Any] = {
        "launcher": launcher,
        "version": None,
        "version_status": "blocked",
        "plugin_api_package_version": "1.18.32",
        "plugin_api_version_matches_cli": False,
        "defaults": {"leader": "ctrl+x", "session_list": "l", "session_new": "n"},
        "route_change_callback_api": "SDK event type tui.session.select; listener observation depends on loaded TUI plugin",
        "network_scope": "loopback_only_local_session_creation",
        "prompt_submissions": 0,
        "repetitions": [],
        "classification": "blocked",
        "durable_acknowledgment": "not_tested",
        "current_at_crash": "not_tested",
    }
    if chosen is None or launcher is None:
        report["version_status"] = "missing_executable"
        report["blocked_reason"] = "launcher_not_found"
        return report
    version_args = ["--version"]
    env_root = tempfile.TemporaryDirectory(prefix="panelink-tui-version-")
    try:
        root = Path(env_root.name)
        root.chmod(0o700)
        env = environment(root)
        project = root / "project"
        project.mkdir(mode=0o700)
        version = bounded_command(launcher_argv(chosen, launcher, version_args), project, env)
        if version.reason or version.returncode != 0:
            report["version_status"] = version.reason or "execution_failed"
            report["blocked_reason"] = "version_check_failed"
            return report
        parsed_version = parse_version(version.stdout + b"\n" + version.stderr)
        report["version"] = parsed_version
        if parsed_version is None:
            report["version_status"] = "unrecognized_version"
            report["blocked_reason"] = "version_unrecognized"
            return report
        report["version_status"] = "proven"
        if parsed_version != "1.19.0":
            report["blocked_reason"] = "installed_cli_not_1.19.0"
            return report
    finally:
        env_root.cleanup()

    report["plugin_api_version_matches_cli"] = False
    for repetition in range(1, REPETITIONS + 1):
        result = rep_result(chosen, launcher, repetition)
        # Raw fixture-generated hashes are restricted to lowercase SHA-256 values.
        report["repetitions"].append(result)
    statuses = [rep.get("status") for rep in report["repetitions"]]
    if len(statuses) != REPETITIONS or any(status == "blocked" for status in statuses):
        report["classification"] = "blocked"
        report["blocked_reason"] = "one_or_more_repetitions_blocked"
    elif all(status == "proven" for status in statuses):
        report["classification"] = "proven"
    else:
        report["classification"] = "not_proven"
        report["blocked_reason"] = "one_or_more_repetitions_not_proven"
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", help="explicit OpenCode/tescode executable")
    arguments = parser.parse_args()
    sandboxed = os.environ.get("PANELINK_TUI_PROBE_SANDBOXED") == "1"
    sandbox = shutil.which("sandbox-exec") if sys.platform == "darwin" else None
    if sys.platform == "darwin" and not sandboxed:
        if sandbox is None:
            result = {
                "launcher": None,
                "version": None,
                "version_status": "blocked_sandbox_unavailable",
                "repetitions": [],
                "classification": "blocked",
                "blocked_reason": "sandbox-exec_required_for_loopback_only_network",
            }
        else:
            with tempfile.TemporaryDirectory(prefix="panelink-tui-probe-parent-") as temporary:
                root = Path(temporary)
                root.chmod(0o700)
                env = environment(root)
                env["PANELINK_TUI_PROBE_SANDBOXED"] = "1"
                chosen, launcher = resolve_executable(arguments.executable)
                if chosen is None or launcher is None:
                    result = {
                        "launcher": None,
                        "version": None,
                        "version_status": "missing_executable",
                        "repetitions": [],
                        "classification": "blocked",
                        "blocked_reason": "launcher_not_found",
                    }
                    print(json.dumps(result, sort_keys=True))
                    return 2
                profile = (
                    '(version 1) (allow default) (deny network*) '
                    '(allow network-outbound (remote ip "localhost:*")) '
                    '(allow network-inbound (local ip "localhost:*"))'
                )
                command = [sandbox, "-p", profile, sys.executable, str(Path(__file__).resolve())]
                command.extend(("--executable", chosen))
                completed = subprocess.run(command, env=env, stdin=subprocess.DEVNULL,
                                           stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                           timeout=REPETITIONS * EXPERIMENT_DEADLINE + 30,
                                           check=False)
                if completed.returncode == 0:
                    sys.stdout.buffer.write(completed.stdout)
                    return 0
                try:
                    result = json.loads(completed.stdout)
                except (json.JSONDecodeError, UnicodeDecodeError):
                    result = {
                        "launcher": None,
                        "version": None,
                        "version_status": "blocked_sandboxed_run_failed",
                        "repetitions": [],
                        "classification": "blocked",
                        "blocked_reason": "sandboxed_child_failed_without_summary",
                    }
    else:
        result = run(arguments.executable)
    print(json.dumps(result, sort_keys=True))
    return 0 if result.get("version_status") == "proven" else 2


if __name__ == "__main__":
    sys.exit(main())
