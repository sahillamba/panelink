#!/usr/bin/env python3
"""Bounded managed-Panelink/OpenCode bridge acceptance probe.

Runs only pinned OpenCode 2.0.32 in a fresh HOME under Apple's loopback-only
sandbox. It creates empty fixture sessions and never submits a prompt. Output
is an allowlisted summary; raw session IDs, PTY screens and credentials stay
in memory and are discarded with the private fixture.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import selectors
import secrets
import shutil
import signal
import stat
import struct
import subprocess
import sys
import threading
import termios
import tempfile
import time
from typing import Any, Callable

try:
    from scripts.probe_opencode_tui_identity import PrivatePty
    from scripts.probe_opencode_v2_identity import (
        EXPECTED_SHA256,
        EXPECTED_VERSION,
        WORKER_ENV,
        _kill_group,
        _run_worker,
        bounded_worker,
        resolve_executable as _resolve_opencode,
        verify_no_user_messages,
    )
except ModuleNotFoundError:
    from probe_opencode_tui_identity import PrivatePty
    from probe_opencode_v2_identity import (
        EXPECTED_SHA256,
        EXPECTED_VERSION,
        WORKER_ENV,
        _kill_group,
        _run_worker,
        bounded_worker,
        resolve_executable as _resolve_opencode,
        verify_no_user_messages,
    )


MAX_WORKER_SECONDS = 60.0
MAX_SERVE_LINE = 8 * 1024
MAX_SERVE_BUFFER = 16 * 1024
MAX_SERVE_EVENTS = 64
MAX_ATTACH_TRANSCRIPT_BYTES = 512 * 1024
CHILD_DEADLINE = 14.0
SWITCH_DEADLINE = 7.5
PTY_SIZE = (24, 100)
SESSION_ID_RE = re.compile(r"ses_[0-9A-Za-z]{26}\Z", re.ASCII)
PRIVATE_TUI_KEYBINDS = {
    # The pinned 2.0.32 bundle uses these command IDs. Keep every fixture route
    # away from Panelink attach's Ctrl-X detach and Ctrl-B prefix bindings.
    # Preserve the independently proven local session list for ordinary
    # session changes. Reserve the global picker for project transitions.
    "session.list": "ctrl+p",
    "open.menu": "ctrl+o",
    # The pinned default also assigns Ctrl-G to session.first.
    "session.first": "none",
    "command.palette.show": "none",
    "session.new": "ctrl+n",
    "session.fork": "ctrl+r",
}
MANAGED_SANDBOX_PROFILE = '''(version 1)
(allow default)
(deny network*)
(allow network-outbound (remote ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
(allow network-bind (local unix-socket))
(allow network-outbound (local unix-socket))
'''


def parse_target(value: Any) -> tuple[int, int, int, int, int] | None:
    if not isinstance(value, str):
        return None
    parts = value.split(":")
    if len(parts) != 5 or any(
        not part.isascii() or not part.isdigit() or str(int(part)) != part for part in parts
    ):
        return None
    numbers = tuple(int(part) for part in parts)
    if any(number <= 0 or number > 0xFFFFFFFFFFFFFFFF for number in numbers):
        return None
    return numbers  # type: ignore[return-value]


def safe_target(value: Any) -> str | None:
    parsed = parse_target(value)
    return ":".join(str(number) for number in parsed) if parsed else None


def compact_fixed_evidence(value: Any) -> Any:
    """Drop repetitive pane geometry from the bounded machine-readable report."""
    if isinstance(value, dict):
        return {
            key: compact_fixed_evidence(item)
            for key, item in value.items()
            if ("screen_positions" not in key and not key.endswith("_positions")
                and key != "screen_row_widths")
        }
    if isinstance(value, list):
        return [compact_fixed_evidence(item) for item in value]
    if isinstance(value, tuple):
        return [compact_fixed_evidence(item) for item in value]
    return value


PICKER_TRACE_KEYS = (
    "picker_open_key", "picker_open_sent", "picker_open_write_ok",
    "output_advanced_after_picker_open", "alive_before_input",
    "alive_after_menu_wait", "fresh_pre_action_read_valid",
    "fresh_pre_action_screen_nonempty", "fresh_menu_child_read_valid",
    "menu_child_picker_heading", "menu_child_sessions_category",
    "menu_child_projects_category", "menu_child_input_owner",
    "menu_child_owner_revision", "menu_child_target_visible_before_filter",
    "target_filter_sent", "target_filter_write_ok", "filtered_child_read_valid",
    "filtered_child_picker_heading", "filtered_child_target_visible",
    "filtered_child_competitor_visible", "filtered_child_row_layout_safe",
    "target_only_filtered_row_seen", "filter_authority", "enter_sent",
    "enter_write_ok", "filter_query_mode", "filter_query_mode_valid",
    "filter_guard_failure",
)


def compact_picker_trace(value: Any) -> dict[str, Any] | None:
    if not isinstance(value, dict):
        return None
    return {key: value[key] for key in PICKER_TRACE_KEYS if key in value}


def compact_binding_checkpoints(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    allowed = ("route", "revision", "provider_session_hash")
    return [
        {key: item[key] for key in allowed if key in item}
        for item in value if isinstance(item, dict)
    ]


def compact_bridge_snapshots(value: Any) -> list[list[dict[str, Any]]]:
    if not isinstance(value, list):
        return []
    result: list[list[dict[str, Any]]] = []
    for target in value:
        if not isinstance(target, list):
            continue
        result.append([
            {key: item[key] for key in ("revision", "status") if key in item}
            for item in target[-12:] if isinstance(item, dict)
        ])
    return result


def safe_cli_target(value: Any) -> str | None:
    """Validate provider-binding CLI's structured five-field target."""
    if not isinstance(value, dict):
        return None
    keys = ("generation", "workspace", "pane", "session", "occupant_revision")
    numbers: list[int] = []
    for key in keys:
        number = value.get(key)
        if (not isinstance(number, int) or isinstance(number, bool)
                or not 0 < number <= 0xFFFFFFFFFFFFFFFF):
            return None
        numbers.append(number)
    return ":".join(str(number) for number in numbers)


def pinned_cli_config_content(executable: Path) -> str | None:
    """Require the pinned binary's private CLI-config and v2 command IDs."""
    try:
        metadata = executable.stat()
        if not executable.is_file() or metadata.st_size <= 0 or metadata.st_size > 512 * 1024 * 1024:
            return None
        artifact = executable.read_bytes()
    except OSError:
        return None
    required = (
        b"OPENCODE_CLI_CONFIG_CONTENT", b"cli.json", b"session.list",
        b"open.menu", b"session.first", b"session.new", b"session.fork",
    )
    if any(marker not in artifact for marker in required):
        return None
    return json.dumps({"keybinds": PRIVATE_TUI_KEYBINDS},
                      sort_keys=True, separators=(",", ":"))


def managed_cli_config_content(executable: Path, observer_dir: Path) -> str | None:
    """Compose the private keymap with the launch plugin and read-only observer.

    The launch config lives at ``<bridge>/opencode-config/cli.json`` and the
    production plugin at its sibling ``identity-plugin`` directory. OpenCode
    resolves local plugin paths relative to the config source, so this keeps
    production's exact generated plugin registered while adding the observer.
    The observer path is absolute because it lives beside the server storage
    directory, not under the launch config.
    """
    keymap = pinned_cli_config_content(executable)
    if keymap is None:
        return None
    observer_path = observer_dir.resolve(strict=True)
    if not observer_path.is_dir():
        return None
    config = json.loads(keymap)
    config["plugins"] = ["../identity-plugin", str(observer_path)]
    return json.dumps(config, sort_keys=True, separators=(",", ":"))


def append_rolling(buffer: bytearray, payload: bytes, limit: int) -> None:
    """Append bytes while retaining only the newest bounded suffix."""
    if limit <= 0:
        return
    if len(payload) >= limit:
        buffer[:] = payload[-limit:]
        return
    buffer.extend(payload)
    if len(buffer) > limit:
        del buffer[:-limit]


def sanitized_snapshot(value: Any, expected: str) -> dict[str, Any] | None:
    if not isinstance(value, dict) or value.get("action") != "provider-binding":
        return None
    target = safe_target(value.get("target"))
    revision = value.get("last_confirmed_revision")
    status = value.get("status")
    if (target != expected or status not in {
            "initializing", "unbound", "pending", "last_confirmed", "stale",
            "ambiguous", "unavailable", "storage_failed",
        }
            or (revision is not None and (not isinstance(revision, int) or isinstance(revision, bool)
                                           or not 0 < revision <= 0xFFFFFFFFFFFFFFFF))
            or value.get("current_at_crash") is not False
            or value.get("automatic_resume") is not False):
        return None
    return {
        "target": target,
        "status": status,
        "last_confirmed_revision": revision,
        "current_at_crash": False,
        "automatic_resume": False,
    }


class ServeProcess:
    """A serve child with bounded line capture and no retained raw diagnostics."""

    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], expected_target: str):
        self.expected_target = expected_target
        self.process = subprocess.Popen(
            argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            close_fds=True, shell=False, start_new_session=True, bufsize=0,
        )
        assert self.process.stdout is not None
        assert self.process.stderr is not None
        os.set_blocking(self.process.stdout.fileno(), False)
        os.set_blocking(self.process.stderr.fileno(), False)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.selector.register(self.process.stderr, selectors.EVENT_READ)
        self.buffer = bytearray()
        self.stderr_capture = bytearray()
        self.event_count = 0
        self.startup: dict[str, Any] | None = None
        self.snapshots: list[dict[str, Any]] = []
        self.invalid_line = False
        self.failure: str | None = None

    def poll(self, timeout: float = 0.0) -> None:
        if self.process.stdout is None or self.selector.get_map() == {}:
            return
        for key, _mask in self.selector.select(max(0.0, timeout)):
            try:
                chunk = os.read(key.fileobj.fileno(), 4096)
            except (BlockingIOError, OSError):
                continue
            if not chunk:
                self.selector.unregister(key.fileobj)
                continue
            if key.fileobj is self.process.stderr:
                self.stderr_capture.extend(chunk)
                if len(self.stderr_capture) > 4096:
                    del self.stderr_capture[:-4096]
                continue
            if len(self.buffer) + len(chunk) > MAX_SERVE_BUFFER:
                self.invalid_line = True
                self.buffer.clear()
                continue
            self.buffer.extend(chunk)
            while b"\n" in self.buffer:
                line, _, tail = self.buffer.partition(b"\n")
                self.buffer = bytearray(tail)
                if len(line) > MAX_SERVE_LINE:
                    self.invalid_line = True
                    continue
                self._accept_line(bytes(line))

    def _accept_line(self, line: bytes) -> None:
        self.event_count += 1
        if self.event_count > MAX_SERVE_EVENTS:
            self.invalid_line = True
            return
        try:
            value = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            self.invalid_line = True
            return
        if isinstance(value, dict) and value.get("ok") is False:
            label = value.get("error")
            if isinstance(label, str) and label in {
                "provider_bridge_launch_unavailable", "listener_permission_denied",
                "server_directory_invalid", "server_storage_unavailable",
                "server_start_failed", "launch_context_invalid", "terminfo_invalid",
                "listener_unavailable", "signal_registration_failed",
            }:
                self.failure = label
            return
        if (isinstance(value, dict) and value.get("action") == "serve"
                and value.get("state") == "running"):
            target = safe_target(value.get("target"))
            generation = value.get("generation")
            if ((self.expected_target and target != self.expected_target)
                    or not isinstance(generation, int)
                    or isinstance(generation, bool) or generation <= 0):
                self.invalid_line = True
                return
            self.startup = {"target": target, "generation": generation}
            if not self.expected_target:
                self.expected_target = target
            elif self.expected_target != target:
                self.invalid_line = True
            return
        clean = sanitized_snapshot(value, self.expected_target)
        if clean is not None:
            self.snapshots.append(clean)
            if len(self.snapshots) > 16:
                del self.snapshots[:-16]

    @property
    def latest(self) -> dict[str, Any] | None:
        return self.snapshots[-1] if self.snapshots else None

    def stderr_category(self) -> str:
        """Classify a small stderr tail without retaining or printing its text."""
        lowered = bytes(self.stderr_capture).lower()
        self.stderr_capture.clear()
        if not lowered:
            return "empty"
        if b"unexpected argument" in lowered or b"unrecognized option" in lowered:
            return "cli_arguments_rejected"
        if b"permission denied" in lowered or b"operation not permitted" in lowered:
            return "permission_denied"
        if b"no such file" in lowered or b"not found" in lowered:
            return "executable_or_path_not_found"
        if b"provider" in lowered and (b"unavailable" in lowered or b"failed" in lowered):
            return "provider_startup_failed"
        if b"panic" in lowered:
            return "process_panicked"
        if b"usage:" in lowered:
            return "cli_usage_error"
        return "unclassified_stderr"

    def close(self, deadline: float) -> bool:
        self.selector.close()
        process = self.process

        def close_pipes() -> None:
            for stream in (process.stdout, process.stderr):
                if stream is not None and not stream.closed:
                    stream.close()

        if process.poll() is None:
            try:
                process.send_signal(signal.SIGTERM)
            except OSError:
                pass
        try:
            process.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            _kill_group(process)
            try:
                process.wait(timeout=1.0)
            except subprocess.SubprocessError:
                close_pipes()
                return False
            close_pipes()
            return False
        close_pipes()
        try:
            os.killpg(process.pid, 0)
            _kill_group(process)
            return False
        except ProcessLookupError:
            return True
        except OSError:
            return process.poll() is not None


def wait_for(predicate, deadline: float, *, pty_child: PrivatePty | None = None) -> bool:
    while time.monotonic() < deadline:
        if predicate():
            return True
        if pty_child is not None:
            if not pty_child.alive():
                return False
            pty_child.drain(min(0.05, max(0.0, deadline - time.monotonic())))
        else:
            time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
    return bool(predicate())


class PanelinkAttachPty(PrivatePty):
    """Attach client PTY with the slave installed as its controlling terminal."""

    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], status_dir: Path):
        self.master, slave = pty.openpty()
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", *PTY_SIZE, 0, 0))
        self.observer = os.dup(slave)
        self.child: subprocess.Popen[bytes] | None = None
        self.output = bytearray()
        self.selector = selectors.DefaultSelector()
        status_path = status_dir / "attach-child.status"
        release_path = status_dir / "attach-child.release"
        supervisor = (
            'status_path=$1; release_path=$2; shift 2; trap \':\' HUP INT TERM; '
            '"$@"; code=$?; printf "%s\\n" "$code" > "$status_path.tmp" && '
            '/bin/mv "$status_path.tmp" "$status_path"; '
            'while [ ! -f "$release_path" ]; do /bin/sleep 0.05; done; exit 0'
        )
        def attach_controlling_terminal() -> None:
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        try:
            self.child = subprocess.Popen(
                ["/bin/sh", "-c", supervisor, "panelink-attach-supervisor",
                 str(status_path), str(release_path), *argv], cwd=cwd, env=env,
                stdin=slave, stdout=slave, stderr=slave,
                close_fds=True, shell=False, start_new_session=False,
                preexec_fn=attach_controlling_terminal,
                bufsize=0,
            )
        finally:
            os.close(slave)
        os.set_blocking(self.master, False)
        self.selector.register(self.master, selectors.EVENT_READ)
        self.status_path = status_path
        self.release_path = release_path
        self.total_output_bytes = 0
        self._output_lock = threading.RLock()
        self._pump_stop = threading.Event()
        self._pump_activity = threading.Event()
        self._pump_thread = threading.Thread(
            target=self._drain_forever,
            name="panelink-managed-pty-drain",
            daemon=True,
        )
        self._pump_thread.start()

    def _drain_forever(self) -> None:
        while not self._pump_stop.is_set():
            try:
                ready = self.selector.select(0.01)
            except (OSError, ValueError):
                return
            if not ready:
                continue
            changed = False
            while not self._pump_stop.is_set():
                try:
                    chunk = os.read(self.master, 8192)
                except (BlockingIOError, OSError):
                    break
                if not chunk:
                    break
                with self._output_lock:
                    self.total_output_bytes += len(chunk)
                    append_rolling(self.output, chunk, MAX_ATTACH_TRANSCRIPT_BYTES)
                changed = True
            if changed:
                self._pump_activity.set()

    def drain(self, timeout: float = 0.0) -> bytes:
        """Wait briefly for pump activity; the dedicated thread owns PTY reads."""
        if timeout > 0:
            self._pump_activity.clear()
            self._pump_activity.wait(min(timeout, 0.05))
        return b""

    def output_snapshot(self) -> tuple[bytes, int]:
        with self._output_lock:
            return bytes(self.output), self.total_output_bytes

    def output_ready(self) -> bool:
        with self._output_lock:
            return bool(self.output)

    def screen_text(self) -> str:
        snapshot, _sequence = self.output_snapshot()
        renderer = object.__new__(PrivatePty)
        renderer.output = bytearray(snapshot)
        return renderer.screen_text()

    def alive(self) -> bool:
        if self.child is None:
            return False
        if self.status_path.exists():
            return False
        return self.child.poll() is None

    def raw_mode(self) -> bool:
        try:
            attributes = termios.tcgetattr(self.observer)
        except OSError:
            return False
        return not (attributes[3] & termios.ICANON)

    def close(self) -> None:
        if self.alive():
            try:
                self.send(b"\x18")  # Panelink's documented detach key.
            except OSError:
                pass
            deadline = time.monotonic() + 1.0
            while time.monotonic() < deadline and not self.status_path.exists():
                self.drain(0.02)
        try:
            self.release_path.write_text("release\n", encoding="ascii")
            self.release_path.chmod(0o600)
        except OSError:
            pass
        if self.child is not None and self.child.poll() is None:
            try:
                self.child.wait(timeout=1.0)
            except subprocess.TimeoutExpired:
                _kill_group(self.child)
                try:
                    self.child.wait(timeout=1.0)
                except subprocess.SubprocessError:
                    pass
        if self.child is not None and self.child.poll() is None:
            _kill_group(self.child)
            try:
                self.child.wait(timeout=1.0)
            except subprocess.SubprocessError:
                pass
        self._pump_stop.set()
        self._pump_thread.join(timeout=1.0)
        if self._pump_thread.is_alive():
            self._pump_thread.join(timeout=1.0)
        if self._pump_thread.is_alive():
            return
        self.selector.close()
        try:
            os.close(self.observer)
        except OSError:
            pass
        try:
            os.close(self.master)
        except OSError:
            pass


def status_after(handle: ServeProcess, status: str, deadline: float) -> dict[str, Any] | None:
    while time.monotonic() < deadline and handle.process.poll() is None:
        handle.poll(0.05)
        current = handle.latest
        if current is not None and current["status"] == status:
            return current
    return handle.latest if handle.latest and handle.latest["status"] == status else None


def confirmation_after(handle: ServeProcess, revision: int, deadline: float) -> dict[str, Any] | None:
    while time.monotonic() < deadline and handle.process.poll() is None:
        handle.poll(0.05)
        current = handle.latest
        if (current is not None and current["status"] == "last_confirmed"
                and isinstance(current["last_confirmed_revision"], int)
                and current["last_confirmed_revision"] > revision):
            return current
    return None


def pty_output_snapshot(pty_child: PrivatePty) -> tuple[bytes, int]:
    snapshot = getattr(pty_child, "output_snapshot", None)
    if callable(snapshot):
        return snapshot()
    return (bytes(pty_child.output),
            getattr(pty_child, "total_output_bytes", len(pty_child.output)))


def pty_output_sequence(pty_child: PrivatePty) -> int:
    return pty_output_snapshot(pty_child)[1]


def wait_initial_pair(
    handles: list[ServeProcess], deadline: float, *, attach: PrivatePty | None = None,
) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    confirmed: list[dict[str, Any] | None] = [None] * len(handles)
    while time.monotonic() < deadline and any(item is None for item in confirmed):
        for index, handle in enumerate(handles):
            handle.poll(0.0)
            current = handle.latest
            if current is not None and current["status"] == "last_confirmed":
                confirmed[index] = current
        if attach is not None and attach.alive():
            # Keep the PTY consumer active while provider snapshots settle;
            # otherwise a healthy full-screen client can hit its output-stall
            # deadline before the fixture begins the session-switch checks.
            attach.drain(0.05)
        else:
            time.sleep(0.05)
    return confirmed[0], confirmed[1]


def screen_markers(pty_child: PrivatePty | None) -> dict[str, Any]:
    if pty_child is None:
        return {"bytes": 0, "alive": False, "exit_code": None}
    pty_child.drain(0.0)
    payload, total_bytes = pty_output_snapshot(pty_child)
    text = pty_child.recent_text(payload).lower()
    screen = pty_child.screen_text().lower()
    child_exit_code = None
    status_path = getattr(pty_child, "status_path", None)
    if isinstance(status_path, Path):
        try:
            raw_status = status_path.read_text(encoding="ascii").strip()
            if raw_status in {"0", "1", "2", "3"}:
                child_exit_code = int(raw_status)
        except OSError:
            pass
    fixed_host_errors = {
        "frame_compose_failed": "could not compose the frame",
        "frame_write_failed": "could not write the frame",
        "terminal_output_stalled": "terminal output made no progress",
        "terminal_restore_failed": "could not restore the terminal",
        "terminal_input_closed": "terminal input closed",
        "terminal_input_read_failed": "could not read terminal input",
        "terminal_input_decode_failed": "terminal input was malformed or oversized",
        "terminal_input_timeout": "terminal input sequence timed out",
        "terminal_size_query_failed": "could not query the controlling terminal",
        "internal_panic": "internal error (terminal restored)",
    }
    host_error_category = next(
        (category for category, marker in fixed_host_errors.items() if marker in text), None
    )
    return {
        "bytes": total_bytes,
        "retained_bytes": len(payload),
        "alive": pty_child.alive(),
        "exit_code": child_exit_code if status_path else (
            pty_child.child.poll() if pty_child.child is not None else None),
        "open_code": "opencode" in text or "oc |" in text,
        "session_a": "panelink v2 identity a" in text,
        "session_b": "panelink v2 identity b" in text,
        "open_session_heading": "open session or project" in screen,
        "sessions_heading": "sessions" in screen,
        "session_a_visible": "panelink v2 identity a" in screen,
        "session_b_visible": "panelink v2 identity b" in screen,
        "open_session_heading_in_transcript": "open session or project" in text,
        "sessions_heading_in_transcript": "sessions" in text,
        "session_a_in_transcript": "panelink v2 identity a" in text,
        "session_b_in_transcript": "panelink v2 identity b" in text,
        "plugin_error": "[tui.plugin]" in text or "plugin failed" in text,
        "connection_error": "connection refused" in text or "failed to connect" in text,
        "tty_error": "could not open the controlling terminal" in text,
        "tty_query_error": "could not query the controlling terminal" in text,
        "permission_error": "permission denied" in text,
        "operation_not_permitted": "operation not permitted" in text,
        "context_unavailable": "context_directory_unavailable" in text,
        "ui_context_unavailable": "context_unavailable" in text,
        "ui_context_mismatch": "context_mismatch" in text,
        "ui_handshake_invalid": "handshake_invalid" in text,
        "ui_response_invalid": "response_invalid" in text,
        "ui_layout_invalid": "layout invalid" in text,
        "ui_pane_disconnected": "pane disconnected" in text,
        "ui_server_unavailable": "server unavailable" in text,
        "ui_transport_failed": "transport_failed" in text,
        "ui_request_invalid": "request_invalid" in text,
        "ui_server_rejected": "server rejected the attachment" in text,
        "ui_context_directory_invalid": "context directory" in text,
        "ui_terminal_initialization_failed": any(token in text for token in (
            "could not register stop signals", "could not enter raw mode",
            "could not initialize terminal modes", "could not compose the frame",
            "could not read terminal input", "terminal input closed",
            "terminal input was malformed", "terminal input sequence timed out",
            "could not write the frame", "terminal output made no progress",
            "could not restore the terminal", "internal error (terminal restored)",
        )),
        "ui_input_closed": "terminal input closed" in text,
        "ui_input_decode_error": "terminal input was malformed or oversized" in text,
        "ui_input_timeout": "terminal input sequence timed out" in text,
        "ui_output_failure": any(token in text for token in (
            "could not write the frame", "terminal output made no progress",
            "could not compose the frame",
        )),
        "terminal_not_interactive": "attach needs an interactive terminal" in text,
        "terminal_open_failed": "could not open the controlling terminal" in text,
        "terminal_query_failed": "could not query the controlling terminal" in text,
        "not_found_error": "no such file" in text or "not found" in text,
        "server_error": "server unavailable" in text or "server is not running" in text,
        "generic_error": "error" in text or "failed" in text,
        "host_error_category": host_error_category,
        "auth_prompt": "password" in text or "authentication" in text,
        "provider_prompt": "connect a provider" in text or "no providers available" in text,
        "input_owner_rejected": "input is owned by another client" in text
            or "input owned by another client" in text,
        "keys_not_sent": "keys not sent" in text,
        "input_delivery_failed": any(token in text for token in (
            "input not delivered", "input partially delivered", "input owned by another client",
            "pane input is owned by another client",
        )),
        "queued_keys_status": "queued keys" in text,
        "no_focused_pane_status": "no focused pane" in text,
    }


def visible_session_menu(pty_child: PrivatePty, labels: tuple[str, str], deadline: float,
                         trace: dict[str, Any] | None = None,
                         *, after_output_sequence: int = 0) -> bool:
    while time.monotonic() < deadline and pty_child.alive():
        pty_child.drain(0.05)
        output_sequence = pty_output_sequence(pty_child)
        screen = pty_child.screen_text()
        lowered = screen.lower()
        heading = session_picker_heading(screen)
        picker_region = session_picker_region(screen)
        trace_labels = [label in picker_region for label in labels]
        if trace is not None:
            trace["menu_heading_seen"] = trace.get("menu_heading_seen", False) or heading
            trace["open_session_title_seen"] = (
                trace.get("open_session_title_seen", False)
                or "open session or project" in lowered
            )
            trace["recent_sessions_title_seen"] = (
                trace.get("recent_sessions_title_seen", False)
                or "recent sessions" in lowered
            )
            trace["both_session_rows_seen"] = trace.get("both_session_rows_seen", False) or all(trace_labels)
            trace["menu_output_sequence"] = output_sequence
            trace["menu_screen_positions"] = fixed_screen_positions(screen, {
                "heading": "Sessions", "target": labels[1], "competitor": labels[0],
                "current_session_title": "Panelink V2 Identity A",
            })
        # A modal must be identifiable by its exact title, or by the generic
        # Sessions heading together with both exact rows inside that modal.
        if output_sequence > after_output_sequence and session_modal_ready(screen, labels):
            return True
    return False


def session_picker_heading(screen: str) -> bool:
    """Recognize exact known modal headings, not incidental screen text."""
    return bool(picker_heading_position(screen))


def picker_heading_position(screen: str) -> list[list[int]]:
    """Locate the first structural title, respecting native row order.

    The local picker has a later Open action row. The global picker has an
    Open title before its search field and Sessions category. Searching for
    Open anywhere conflates those layouts and loses the local query rows.
    """
    lines = screen.splitlines()[:PTY_SIZE[0]]
    for row, line in enumerate(lines):
        normalized = line.strip().lower()
        local = normalized == "sessions" or normalized.startswith((
            "sessions for project", "open session or project", "recent sessions"))
        # The pinned title renderer puts its fixed esc hint on the same row.
        # Match the complete token sequence, never the OpenCode header.
        global_title = normalized.split() in (["open"], ["open", "esc"]) and any(
            later.strip().lower() == "sessions" for later in lines[row + 1:])
        if local or global_title:
            return [[row, len(line) - len(line.lstrip())]]
    return []


def picker_heading_marker(screen: str) -> str:
    """Choose the exact heading coordinate for the active native picker."""
    position = picker_heading_position(screen)
    if (position and screen.splitlines()[position[0][0]].strip().lower().split()
            in (["open"], ["open", "esc"])):
        return "Open"
    return "Sessions"


def filter_guard_summary(markers: dict[str, Any] | None) -> dict[str, Any]:
    """Bounded structural diagnostics; never retain screen text or titles."""
    if not isinstance(markers, dict):
        return {"screen_available": False}
    positions = markers.get("screen_positions", {})
    headings = positions.get("heading", [])
    row = headings[0][0] if len(headings) == 1 else -1
    def safe_points(name: str) -> list[list[int]]:
        points = positions.get(name, [])
        return [point for point in points if isinstance(point, list) and len(point) == 2
                and all(isinstance(value, int) for value in point)]

    return {
        "screen_available": True,
        "heading_count": len(headings), "heading_row": row,
        "global_open": markers.get("contains_open_menu_heading"),
        "target_index": markers.get("target_fixture_index"),
        "fixture_count": markers.get("fixture_title_count"),
        "unknown_selectable_row_count": markers.get("unknown_selectable_row_count"),
        "session_result_row_count": markers.get("session_result_row_count"),
        "session_rows_boundary_verified": markers.get("session_rows_boundary_verified"),
        "session_rows_overflowed": markers.get("session_rows_overflowed"),
        "session_rows_guard_reason": markers.get("session_rows_guard_reason"),
        "session_modal_bounds": markers.get("session_modal_bounds"),
        "projects_category": markers.get("contains_projects_category"),
        "owner_revision": markers.get("owner_revision"),
        "row_counts": {key: sum(point[0] > row for point in points)
                       for key, points in positions.items()},
        "modal_projection": {
            "heading_row": row,
            "sessions_category_rows": [point[0] for point in safe_points("sessions_category")],
            "target_coordinates": safe_points("target"),
            "filter_query_coordinates": safe_points("filter_query"),
        },
    }


def fixture_route_diagnostic(path: Path, nonce: str,
                             projects: set[str]) -> tuple[dict[str, Any] | None,
                                                         dict[str, Any]]:
    """Return strict parser output plus fixed, hash-only stream diagnostics."""
    try:
        payload = path.read_bytes()
    except FileNotFoundError:
        return None, {"status": "missing", "byte_count": 0, "line_count": 0,
                      "accepted_count": 0, "rejected_count": 0, "instance_count": 0}
    except OSError:
        return None, {"status": "unreadable", "byte_count": 0, "line_count": 0,
                      "accepted_count": 0, "rejected_count": 0, "instance_count": 0}
    line_count = len(payload.splitlines())
    base = {"byte_count": len(payload), "line_count": line_count,
            "accepted_count": 0, "rejected_count": 0, "instance_count": 0}
    if not payload:
        return None, {**base, "status": "empty"}
    try:
        from scripts.probe_opencode_v2_remaining_identity import _read_transition_records
    except ModuleNotFoundError:
        try:
            from probe_opencode_v2_remaining_identity import _read_transition_records
        except ModuleNotFoundError:
            return None, {**base, "status": "parser_unavailable"}
    try:
        records, instances, rejected = _read_transition_records(path, nonce, projects)
    except (OSError, ValueError, UnicodeDecodeError):
        return None, {**base, "status": "rejected_stream", "rejected_count": 1}
    instance_count = len(set(instances))
    if rejected:
        return None, {**base, "status": "rejected_stream", "rejected_count": rejected,
                      "instance_count": instance_count}
    if not records:
        return None, {**base, "status": "no_valid_records"}
    record = records[-1]
    status = "instance_mismatch" if instance_count != 1 else "valid"
    diagnostic = {
        **base,
        "status": status,
        "accepted_count": len(records),
        "instance_count": instance_count,
        "latest_sequence": record.get("seq"),
        "latest_route": record.get("route_name"),
        "latest_project_sha256": record.get("project_hash"),
        "latest_session_sha256": record.get("session_id_hash"),
        "latest_session_top_level": record.get("session_top_level"),
        "latest_instance_sha256": (hashlib.sha256(instances[-1].encode("ascii")).hexdigest()
                                   if instances else None),
    }
    if status != "valid":
        return None, diagnostic
    # The strict parser returns the validated instance stream separately from
    # its route projection. Preserve that validated link for same-client
    # sequence checks instead of silently dropping it at this adapter.
    return {**record, "tui_instance": instances[-1]}, diagnostic


def read_fixture_route(path: Path, nonce: str, projects: set[str]) -> dict[str, Any] | None:
    """Read the existing nonce/sequence-checked, hash-only test observer."""
    record, _diagnostic = fixture_route_diagnostic(path, nonce, projects)
    return record


def fixture_route_matches(record: dict[str, Any] | None, project_hash: str,
                          session_hash: str, instance: str | None = None,
                          after_seq: int = 0) -> bool:
    return bool(record and record.get("route_name") == "session"
                and record.get("session_top_level") is True
                and record.get("project_hash") == project_hash
                and record.get("session_id_hash") == session_hash
                and (instance is None or record.get("tui_instance") == instance)
                and record.get("seq", 0) > after_seq)


def foreign_refusal_acceptance(checks: dict[str, bool]) -> bool:
    """Require exact observer, visible refusal, live ownership, and no mutation."""
    required = (
        "selected_foreign", "exact_foreign_route", "same_tui_instance",
        "sequence_advanced", "stale_or_unavailable", "a_binding_preserved_before_cli",
        "picker_closed", "pane_owner_client", "pane_live", "pane_applied",
        "manual_refusal_valid", "manual_refusal_no_ticket",
        "foreign_route_persisted_after_refusal", "a_binding_preserved_after_cli",
    )
    return all(checks.get(key) is True for key in required)


def wait_fixture_route(path: Path, nonce: str, projects: set[str],
                       project_hash: str, session_hash: str, instance: str | None,
                       after_seq: int, deadline: float) -> dict[str, Any] | None:
    """Wait for the same native TUI to acknowledge one exact route transition."""
    while time.monotonic() < deadline:
        record = read_fixture_route(path, nonce, projects)
        if fixture_route_matches(record, project_hash, session_hash,
                                 instance=instance, after_seq=after_seq):
            return record
        time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
    return None


def session_picker_screen(screen: str, labels: tuple[str, str]) -> bool:
    picker = session_picker_region(screen)
    return session_picker_heading(picker) and all(label in picker for label in labels)


def filtered_session_row(screen: str, target_label: str, other_label: str) -> bool:
    picker = session_picker_region(screen)
    return (session_picker_heading(picker) and target_label in picker
            and other_label not in picker)


def fresh_child_modal_rows(markers: dict[str, Any], *, filtered: bool,
                           require_projects_category: bool = False) -> bool:
    """Require one result row in the pinned TUI's live control-read screen.

    The native picker renders the exact query in its input row and once in the
    result row. Both coordinates must be below the modal heading; a duplicate
    matching session adds another result coordinate and fails closed.
    """
    positions = markers.get("screen_positions")
    if (markers.get("status") != "known" or markers.get("input_owner") != "client"
            or markers.get("contains_picker_heading") is not True
            or (require_projects_category
                and markers.get("contains_projects_category") is not True)
            or not isinstance(positions, dict)):
        return False
    heading = positions.get("heading")
    sessions_category = positions.get("sessions_category")
    target = positions.get("target")
    competitor = positions.get("competitor")
    filter_query = positions.get("filter_query")
    if (not isinstance(heading, list) or len(heading) != 1
            or not isinstance(target, list) or not isinstance(competitor, list)):
        return False
    heading_row = heading[0][0] if (isinstance(heading[0], list) and len(heading[0]) == 2
                                    and all(isinstance(v, int) for v in heading[0])) else None
    if heading_row is None:
        return False
    if markers.get("contains_open_menu_heading") is True:
        # In the pinned global picker, the exact dialog title is "Open" and
        # the search field is above the Sessions category. Require that real
        # category below the title while counting query/result rows from Open.
        # Before typing, multiple results are expected. After filtering,
        # project results and unrecognized session rows make Enter ambiguous.
        if (markers.get("contains_sessions_category") is not True
                or not isinstance(sessions_category, list)
                or not any(isinstance(point, list) and len(point) == 2
                           and all(isinstance(value, int) for value in point)
                           and point[0] > heading_row for point in sessions_category)
                or (filtered and markers.get("contains_projects_category") is not False)
                or (filtered and markers.get("session_rows_boundary_verified") is not True)
                or (filtered and markers.get("session_rows_overflowed") is not False)
                or (filtered and markers.get("unknown_selectable_row_count") != 0)):
            return False
    target_rows = [point[0] for point in target
                   if isinstance(point, list) and len(point) == 2
                   and all(isinstance(v, int) for v in point) and point[0] > heading_row]
    competitor_rows = [point[0] for point in competitor
                       if isinstance(point, list) and len(point) == 2
                       and all(isinstance(v, int) for v in point) and point[0] > heading_row]
    if filtered:
        # The pinned UI retains the previous unfiltered result list while its
        # debounced query is pending. The target must therefore be the only
        # known fixture with a result coordinate below the heading. The query
        # and its result each render the exact target label once.
        target_index = markers.get("target_fixture_index")
        query_is_target = markers.get("filter_query_is_target_label")
        if query_is_target is None:
            query_is_target = True
        query_mode = markers.get("filter_query_mode", "title")
        if query_mode not in {"title", "fixture_session_id"}:
            return False
        exact_session_query = query_mode == "fixture_session_id"
        expected_target_rows = 1 if exact_session_query or query_is_target is not True else 2
        if (not isinstance(target_index, int) or target_index < 0
                or markers.get("contains_target_label") is not True
                or len(target_rows) != expected_target_rows):
            return False
        if markers.get("contains_open_menu_heading") is True:
            if not isinstance(filter_query, list):
                return False
            query_points = [point for point in filter_query
                            if isinstance(point, list) and len(point) == 2
                            and all(isinstance(value, int) for value in point)]
            # The query must occur once in the search field and once in the
            # sole result. Any other occurrence makes activation ambiguous.
            if exact_session_query:
                if (markers.get("filter_query_is_fixture_session_id") is not True
                        or not isinstance(sessions_category, list)
                        or len(sessions_category) != 1
                        or len(target_rows) != 1
                        or len(query_points) != 1
                        or not (heading_row < query_points[0][0]
                                < sessions_category[0][0] < target_rows[0])
                        or markers.get("session_result_row_count") != 1):
                    return False
            elif (len(query_points) != 2
                  or sum(point[0] in target_rows for point in query_points)
                  != expected_target_rows):
                return False
        fixture_rows: dict[int, list[int]] = {}
        for name, points in positions.items():
            if not name.startswith("fixture_label_") or not isinstance(points, list):
                continue
            try:
                index = int(name.removeprefix("fixture_label_"))
            except ValueError:
                return False
            fixture_rows[index] = [
                point[0] for point in points
                if isinstance(point, list) and len(point) == 2
                and all(isinstance(value, int) for value in point)
                and point[0] > heading_row
            ]
        expected_indices = markers.get("fixture_title_count")
        if not isinstance(expected_indices, int) or expected_indices < 2:
            return False
        if set(fixture_rows) != set(range(expected_indices)):
            return False
        return all(
            (len(rows) == expected_target_rows if index == target_index else not rows)
            for index, rows in fixture_rows.items()
        )
    fixture_row_count = 0
    for name, points in positions.items():
        if not name.startswith("fixture_label_") or not isinstance(points, list):
            continue
        fixture_row_count += sum(
            isinstance(point, list) and len(point) == 2
            and all(isinstance(value, int) for value in point) and point[0] > heading_row
            for point in points
        )
    return fixture_row_count >= 1


def stable_fresh_child_modal_rows(read_child: Callable[[], dict[str, Any]],
                                  target_label: str, fixture_titles: tuple[str, ...],
                                  deadline: float,
                                  filter_text: str | None = None,
                                  filter_mode: str = "title",
                                  fixture_session_id: str | None = None
                                  ) -> tuple[bool, dict[str, Any] | None]:
    """Require two identical filtered child reads beyond the native debounce."""
    previous_signature: tuple[Any, ...] | None = None
    previous_at = 0.0
    last: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        current = read_child()
        last = current if isinstance(current, dict) else None
        query_matches_fixture = (
            filter_mode == "title"
            and (filter_text is None or filter_text == target_label)
            and last is not None
            and last.get("filter_query_is_target_label") is not False
        ) or (
            filter_mode == "fixture_session_id"
            and fixture_session_id is not None
            and SESSION_ID_RE.fullmatch(fixture_session_id) is not None
            and filter_text == fixture_session_id
            and last is not None
            and last.get("filter_query_mode") == "fixture_session_id"
            and last.get("filter_query_is_fixture_session_id") is True
        )
        if (last is not None
                and fresh_child_modal_rows(last, filtered=True)
                and query_matches_fixture
                and last.get("target_fixture_index") == fixture_titles.index(target_label)
                and last.get("fixture_title_count") == len(fixture_titles)):
            signature = (
                last.get("input_owner"), last.get("owner_revision"),
                tuple((key, tuple(tuple(point) for point in points))
                      for key, points in sorted(last.get("screen_positions", {}).items())
                      if key in {"heading", "target", "filter_query"}
                      or key.startswith("fixture_label_")),
            )
            now = time.monotonic()
            if (signature == previous_signature and now - previous_at >= 0.20):
                return True, last
            if signature != previous_signature:
                previous_signature = signature
                previous_at = now
        else:
            previous_signature = None
            previous_at = 0.0
        time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
    return False, last


def session_picker_region(screen: str) -> str:
    """Return the native picker from its exact modal title onward."""
    position = picker_heading_position(screen)
    return "\n".join(screen.splitlines()[position[0][0]:]) if position else ""


def global_picker_session_result_rows(
        screen: str
        ) -> tuple[list[str] | None, bool, bool, list[int] | None, str]:
    """Project only the pinned global dialog's contiguous Sessions result rows.

    OpenCode 2.0.32 renders one title/footer row per session, then a blank
    DialogLayout gap before the next category. The dialog is centered, has a
    large width capped at terminal-width minus two, and has a three-cell title
    inset. The supplied PTY width is fixed by this probe; if the observed title
    does not land at that source-derived coordinate, refuse to interpret rows.
    """
    lines = screen.splitlines()[:PTY_SIZE[0]]
    heading = picker_heading_position(screen)
    if (not heading or not lines or
            lines[heading[0][0]].strip().lower().split() not in (["open"], ["open", "esc"])):
        return None, False, False, None, "picker_geometry_unavailable"
    width = PTY_SIZE[1]
    dialog_width = min(88, width - 2)
    if width < 5 or dialog_width <= 0:
        return None, False, False, None, "invalid_terminal_width"
    left = (width - dialog_width) // 2
    right = left + dialog_width
    heading_row, heading_column = heading[0]
    if heading_column != left + 3:
        return None, False, False, [left, right], "heading_column_mismatch"
    categories = [
        (row, len(line) - len(line.lstrip()))
        for row, line in enumerate(lines)
        if row > heading_row and line.strip().lower() == "sessions"
    ]
    if (len(categories) != 1 or not left <= categories[0][1] < right):
        return None, False, False, [left, right], "sessions_category_unavailable"

    rows: list[str] = []
    for line in lines[categories[0][0] + 1:]:
        projected = line[left:right]
        if not projected.strip():
            return rows, True, False, [left, right], "ok"
        content = projected.strip()
        # The pinned renderer emits list overflow as its own one-character
        # row. An ellipsis in a title/footer is row content; exact fixture
        # title coordinates and the right-edge check below guard that case.
        if content in {"…", "..."}:
            return rows, True, True, [left, right], "ellipsis"
        # A nonblank final cell means the visible row reaches the modal edge;
        # its title/footer could have been clipped by the viewport.
        if len(line) >= right and line[right - 1:right].strip():
            return rows, True, True, [left, right], "modal_right_edge_nonblank"
        rows.append(content)
    return rows, False, False, [left, right], "blank_boundary_missing"


def session_modal_ready(screen: str, labels: tuple[str, str]) -> bool:
    del labels  # exact filtered-result gating happens before Enter.
    return session_picker_heading(screen)


def fixed_screen_positions(screen: str, markers: dict[str, str]) -> dict[str, list[list[int]]]:
    """Return only fixed marker row/column coordinates from a fixture screen."""
    positions: dict[str, list[list[int]]] = {}
    for name, marker in markers.items():
        found: list[list[int]] = []
        for row, line in enumerate(screen.splitlines()[:PTY_SIZE[0]]):
            start = 0
            while len(found) < 8:
                column = line.find(marker, start)
                if column < 0:
                    break
                found.append([row, min(column, PTY_SIZE[1] - 1)])
                start = column + max(1, len(marker))
        positions[name] = found
    return positions


def exact_line_positions(screen: str, marker: str) -> list[list[int]]:
    """Locate only rows whose stripped text equals a structural marker."""
    result: list[list[int]] = []
    for row, line in enumerate(screen.splitlines()[:PTY_SIZE[0]]):
        stripped = line.strip()
        if stripped.lower() == marker.lower():
            result.append([row, line.lower().find(marker.lower())])
    return result


def managed_owner_markers(binary: Path, directory: Path, context_file: Path,
                          expected_target: str) -> dict[str, Any]:
    """Read exact-target controller ownership without exposing its lease ID."""
    try:
        completed = subprocess.run(
            [str(binary), "control", "--directory", str(directory),
             "--context", str(context_file), "describe"],
            cwd=Path.cwd(), env={"PATH": os.defpath, "HOME": str(context_file.parent), "LANG": "C"},
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=2, check=False, close_fds=True, shell=False,
        )
        if completed.returncode != 0 or len(completed.stdout) > 4096:
            return {"status": "unavailable"}
        value = json.loads(completed.stdout)
    except (OSError, subprocess.SubprocessError, ValueError, UnicodeDecodeError):
        return {"status": "unavailable"}
    lease = value.get("owner_lease") if isinstance(value, dict) else None
    revision = value.get("owner_revision") if isinstance(value, dict) else None
    if (not isinstance(value, dict) or value.get("action") != "describe"
            or safe_target(value.get("target")) != expected_target
            or isinstance(lease, bool) or not isinstance(lease, int) or lease < 0
            or isinstance(revision, bool) or not isinstance(revision, int) or revision <= 0
            or not isinstance(value.get("child_exited"), bool)):
        return {"status": "unavailable"}
    return {
        "status": "known",
        "input_owner": "client" if lease else "none",
        "owner_revision": revision,
        "child_exited": value["child_exited"],
    }


def select_session(pty_child: PrivatePty, target_label: str, other_label: str,
                   labels: tuple[str, str], deadline: float,
                   trace: dict[str, Any] | None = None,
                   *, pre_read: dict[str, Any] | None = None,
                   fresh_child_read: Callable[[], dict[str, Any]] | None = None,
                   fixture_titles: tuple[str, ...] | None = None,
                   open_chord: bytes = b"\x10",
                   filter_text: str | None = None,
                   filter_mode: str = "title",
                   fixture_session_id: str | None = None) -> bool:
    if fixture_titles is None:
        fixture_titles = labels
    if filter_text is None:
        filter_text = target_label
    if filter_mode == "fixture_session_id":
        if (fixture_session_id is None
                or SESSION_ID_RE.fullmatch(fixture_session_id) is None
                or filter_text != fixture_session_id):
            if trace is not None:
                trace["filter_query_mode_valid"] = False
            return False
        if trace is not None:
            trace["filter_query_mode"] = filter_mode
            trace["filter_query_mode_valid"] = True
    elif filter_mode != "title":
        return False
    elif trace is not None:
        trace["filter_query_mode"] = filter_mode
    if target_label not in fixture_titles:
        return False
    if trace is not None:
        trace["alive_before_input"] = pty_child.alive()
    if not pty_child.alive():
        return False
    # A fresh Panelink control-read response of the same managed child is a
    # pre-action observation. The read function returns fixed markers only;
    # screen text remains in memory and is never included in the result.
    if (not isinstance(pre_read, dict) or pre_read.get("status") != "known"
            or pre_read.get("contains_opencode_title") is not True
            or not isinstance(pre_read.get("screen_bytes"), int)
            or pre_read["screen_bytes"] <= 0):
        if trace is not None:
            trace["fresh_pre_action_read_valid"] = False
        return False
    if trace is not None:
        trace["fresh_pre_action_read_valid"] = True
    pty_child.drain(0.05)  # bounded screen read immediately before input
    before_sequence = pty_output_sequence(pty_child)
    before_screen = pty_child.screen_text()
    if not before_screen:
        if trace is not None:
            trace["fresh_pre_action_screen_nonempty"] = False
        return False
    if trace is not None:
        trace["fresh_pre_action_screen_nonempty"] = True
        trace["pre_input_output_sequence"] = before_sequence
    try:
        pty_child.send(open_chord)
    except OSError:
        if trace is not None:
            trace["picker_open_write_ok"] = False
        return False
    if trace is not None:
        trace["picker_open_key"] = {
            b"\x10": "ctrl+p", b"\x07": "ctrl+g", b"\x0f": "ctrl+o",
        }.get(open_chord, "other")
        trace["picker_open_sent"] = True
        trace["picker_open_write_ok"] = True
        trace["alive_after_picker_open"] = pty_child.alive()
    output_after_open = pty_output_sequence(pty_child)
    if trace is not None:
        trace["output_sequence_immediately_after_picker_open"] = output_after_open
    menu_seen = visible_session_menu(
        pty_child, labels, min(deadline, time.monotonic() + 4.0), trace,
        after_output_sequence=before_sequence,
    )
    if trace is not None:
        trace["alive_after_menu_wait"] = pty_child.alive()
        trace["output_advanced_after_picker_open"] = (
            pty_output_sequence(pty_child) > before_sequence
        )
    # The child control read is the authoritative current pane. Attach VT
    # reconstruction remains diagnostic because its bounded rolling suffix
    # can begin mid-screen after long-running TUI output.
    menu_child = fresh_child_read() if fresh_child_read is not None else None
    require_projects_category = False
    menu_child_ok = isinstance(menu_child, dict) and fresh_child_modal_rows(
        menu_child, filtered=False,
        require_projects_category=require_projects_category,
    )
    if trace is not None:
        trace["fresh_menu_child_read_valid"] = (
            isinstance(menu_child, dict) and menu_child.get("status") == "known"
        )
        trace["menu_child_picker_heading"] = (
            isinstance(menu_child, dict) and menu_child.get("contains_picker_heading") is True
        )
        trace["menu_child_sessions_category"] = (
            isinstance(menu_child, dict) and menu_child.get("contains_sessions_category") is True
        )
        trace["menu_child_contains_both_rows"] = menu_child_ok
        trace["menu_child_target_visible_before_filter"] = bool(
            isinstance(menu_child, dict)
            and menu_child.get("contains_target_label") is True
        )
        trace["menu_child_projects_category"] = (
            isinstance(menu_child, dict)
            and menu_child.get("contains_projects_category") is True
        )
        trace["menu_child_screen_agree"] = bool(menu_seen and menu_child_ok)
        trace["menu_child_screen_positions"] = (
            menu_child.get("screen_positions") if isinstance(menu_child, dict) else None
        )
        trace["menu_child_input_owner"] = (
            menu_child.get("input_owner") if isinstance(menu_child, dict) else None
        )
        trace["menu_child_owner_revision"] = (
            menu_child.get("owner_revision") if isinstance(menu_child, dict) else None
        )
    if fresh_child_read is not None and not menu_child_ok:
        return False
    if not menu_seen and not menu_child_ok:
        return False
    try:
        # Match the already-proven native fixture's single bounded title
        # write. The exact ASCII title is the only text sent and remains in
        # the picker; it is never sent to the prompt.
        encoded_title = filter_text.encode("ascii")
        if not encoded_title or len(encoded_title) > 128:
            return False
        pty_child.send(encoded_title)
    except OSError:
        if trace is not None:
            trace["target_filter_write_ok"] = False
        return False
    if trace is not None:
        trace["target_filter_sent"] = True
        trace["target_filter_write_ok"] = True
        trace["filtered_heading_seen"] = False
        trace["filtered_target_visible"] = False
        trace["filtered_competitor_visible"] = False
        trace["filtered_output_advanced"] = False
        trace["filtered_child_read_valid"] = False
    filter_sequence = pty_output_sequence(pty_child)
    filtered_deadline = min(deadline, time.monotonic() + 2.0)
    child_filter_ready = False
    while time.monotonic() < filtered_deadline and pty_child.alive():
        pty_child.drain(0.05)
        output_sequence = pty_output_sequence(pty_child)
        screen = pty_child.screen_text()
        heading_visible = session_picker_heading(screen)
        target_visible = target_label in screen
        other_visible = other_label in screen
        if trace is not None:
            trace["filtered_heading_seen"] = heading_visible
            trace["filtered_target_visible"] = target_visible
            trace["filtered_competitor_visible"] = other_visible
            trace["filtered_output_advanced"] = output_sequence > filter_sequence
            trace["filtered_screen_positions"] = fixed_screen_positions(screen, {
                "heading": "Sessions", "target": target_label,
                "competitor": other_label,
            })
        stable, current_child = (stable_fresh_child_modal_rows(
            fresh_child_read, target_label, fixture_titles,
            min(filtered_deadline, time.monotonic() + 1.0), filter_text,
            filter_mode, fixture_session_id,
        ) if fresh_child_read is not None else (False, None))
        if stable and isinstance(current_child, dict):
            child_filter_ready = True
            if trace is not None:
                trace["filtered_child_read_valid"] = True
                trace["filtered_child_picker_heading"] = True
                trace["filtered_child_target_visible"] = True
                trace["filtered_child_competitor_visible"] = False
                trace["filtered_child_row_layout_safe"] = True
                trace["filtered_child_screen_positions"] = current_child.get("screen_positions")
                trace["filter_authority"] = "fresh_managed_child_screen"
            break
        if (filtered_session_row(screen, target_label, other_label)
                and output_sequence > filter_sequence):
            if trace is not None:
                trace["target_only_filtered_row_seen"] = True
                trace["filtered_output_sequence"] = output_sequence
            break
    else:
        # The managed child's control-read screen is the current native pane;
        # permit it to authorize activation only when its fixed row
        # coordinates prove exactly one matching result.
        failed_filter_child = None
        child_filter_deadline = min(deadline, time.monotonic() + 1.5)
        if fresh_child_read is not None and time.monotonic() < child_filter_deadline:
            child_filter_ready, failed_filter_child = stable_fresh_child_modal_rows(
                fresh_child_read, target_label, fixture_titles, child_filter_deadline,
                filter_text, filter_mode, fixture_session_id,
            )
        if trace is not None and isinstance(failed_filter_child, dict):
            trace["filtered_child_read_valid"] = failed_filter_child.get("status") == "known"
            trace["filtered_child_picker_heading"] = (
                failed_filter_child.get("contains_picker_heading") is True
            )
            trace["filtered_child_target_visible"] = (
                failed_filter_child.get("contains_target_label") is True
            )
            trace["filtered_child_competitor_visible"] = (
                failed_filter_child.get("contains_competitor_label") is True
            )
            trace["filtered_attach_child_screen_agree"] = False
            trace["filtered_child_screen_positions"] = failed_filter_child.get("screen_positions")
            trace["filtered_child_row_layout_safe"] = child_filter_ready
        if not child_filter_ready:
            if trace is not None:
                trace["filter_guard_failure"] = filter_guard_summary(failed_filter_child)
            return False
        if trace is not None:
            trace["target_only_filtered_row_seen"] = True
            trace["filter_authority"] = "fresh_managed_child_screen"
    if child_filter_ready and trace is not None:
        trace["target_only_filtered_row_seen"] = True
    pty_child.drain(0.02)  # inspect the filtered row again immediately before activation
    final_screen = pty_child.screen_text()
    attach_has_picker = session_picker_heading(final_screen)
    attach_filtered = filtered_session_row(final_screen, target_label, other_label)
    if trace is not None:
        trace["attach_vt_filtered_row_seen"] = attach_filtered
        trace["attach_vt_picker_heading_seen"] = attach_has_picker
    if (attach_has_picker and not attach_filtered
            and not (trace and trace.get("filter_authority") == "fresh_managed_child_screen")):
        return False
    final_child = None
    final_child_ok = False
    if fresh_child_read is not None:
        child_read_deadline = min(deadline, time.monotonic() + 1.0)
        final_child_ok, final_child = stable_fresh_child_modal_rows(
            fresh_child_read, target_label, fixture_titles, child_read_deadline,
            filter_text, filter_mode, fixture_session_id,
        )
    if trace is not None:
        trace["filtered_child_read_valid"] = final_child_ok
        trace["filtered_child_picker_heading"] = bool(
            isinstance(final_child, dict) and final_child.get("contains_picker_heading") is True
        )
        trace["filtered_child_target_visible"] = bool(
            isinstance(final_child, dict) and final_child.get("contains_target_label") is True
        )
        trace["filtered_child_competitor_visible"] = bool(
            isinstance(final_child, dict) and final_child.get("contains_competitor_label") is True
        )
        trace["filtered_attach_child_screen_agree"] = final_child_ok
        trace["filtered_child_screen_positions"] = (
            final_child.get("screen_positions") if isinstance(final_child, dict) else None
        )
    if fresh_child_read is not None and not final_child_ok:
        if trace is not None:
            trace["filter_guard_failure"] = filter_guard_summary(final_child)
        return False
    try:
        pty_child.send(b"\r")
    except OSError:
        if trace is not None:
            trace["enter_write_ok"] = False
        return False
    if trace is not None:
        trace["enter_sent"] = True
        trace["enter_write_ok"] = True
    return True


def compile_terminfo(root: Path) -> Path | None:
    tic = shutil.which("tic")
    if tic is None:
        return None
    output = root / "terminfo"
    output.mkdir(mode=0o700)
    source = Path(__file__).resolve().parents[1] / "docs/terminfo/panelink-probe.terminfo"
    try:
        result = subprocess.run(
            [tic, "-x", "-o", str(output), str(source)], stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            timeout=3, check=False, close_fds=True, shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return output if result.returncode == 0 else None


def historical_binding(binary: Path, directory: Path, project: Path, target: str,
                       diagnostic: list[str] | None = None) -> dict[str, Any] | None:
    try:
        result = subprocess.run(
            [str(binary), "provider-binding", "show", "--directory", str(directory),
             "--target", target, "--project", str(project)],
            cwd=project, env={"PATH": os.defpath, "HOME": str(project), "LANG": "C"},
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=3, check=False, close_fds=True, shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        if diagnostic is not None:
            diagnostic.append("command_failed")
        return None
    if result.returncode != 0 or len(result.stdout) > 4096:
        if diagnostic is not None:
            diagnostic.append("command_status_invalid")
        return None
    try:
        value = json.loads(result.stdout)
    except (ValueError, UnicodeDecodeError):
        if diagnostic is not None:
            diagnostic.append("json_invalid")
        return None
    identifier = value.get("provider_session_id") if isinstance(value, dict) else None
    if not isinstance(value, dict):
        reason = "record_invalid"
    elif value.get("state") != "last_confirmed_conversation":
        reason = "state_unexpected"
    elif safe_cli_target(value.get("target")) != target:
        reason = "target_mismatch"
    elif (not isinstance(identifier, str) or not 1 <= len(identifier) <= 128
          or not identifier.isascii()
          or any(not (char.isalnum() or char in "_.-") for char in identifier)):
        reason = "session_id_invalid"
    elif (not isinstance(value.get("binding_revision"), int)
          or isinstance(value.get("binding_revision"), bool)
          or not 1 <= value["binding_revision"] <= 0xFFFFFFFFFFFFFFFF):
        reason = "binding_revision_invalid"
    elif value.get("provider_version") != EXPECTED_VERSION:
        reason = "provider_version_invalid"
    elif value.get("current_at_crash") is not False or value.get("automatic_resume") is not False:
        reason = "recovery_flags_invalid"
    elif value.get("managed_launch_linked") is not True:
        reason = "managed_link_missing"
    else:
        reason = "valid"
    if reason != "valid":
        if diagnostic is not None:
            diagnostic.append(reason)
        return None
    if diagnostic is not None:
        diagnostic.append("valid")
    return {
        "target": target,
        "provider_session_hash": hashlib.sha256(identifier.encode("ascii")).hexdigest(),
        "binding_revision": value.get("binding_revision"),
        "provider_version": value.get("provider_version"),
        "current_at_crash": False,
        "automatic_resume": False,
        "managed_launch_linked": True,
    }


def read_live_managed_binding(directory: Path, target: str, project: Path,
                              expected_session_id: str | None) -> dict[str, Any] | None:
    """Read and validate the fixture's exact atomic combined slot while live.

    This is a read-only acceptance-fixture decoder, not a production storage
    API and not a lock bypass for mutation. It opens only the deterministic
    exact-target slot with no-follow semantics and verifies owner, mode, inode,
    bounds, full target/project/version, and the expected fixture ID. It returns
    a hash and revision only; the ID and bytes remain transient.
    """
    parsed_target = parse_target(target)
    if parsed_target is None or (expected_session_id is not None and not expected_session_id.isascii()):
        return None
    bindings_root = directory / "provider-bindings"
    target_directory = bindings_root / "-".join(str(field) for field in parsed_target)
    slot = target_directory / "managed-provider.record"
    try:
        uid = os.geteuid()
        for path in (directory, bindings_root, target_directory):
            metadata = path.lstat()
            if (not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != uid
                    or metadata.st_mode & 0o077):
                return None
        descriptor = os.open(
            slot,
            os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0),
        )
    except OSError:
        return None
    try:
        before = os.fstat(descriptor)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.geteuid()
                or before.st_mode & 0o077 or not 72 <= before.st_size <= 8_552):
            return None
        chunks = bytearray()
        while len(chunks) <= 8_552:
            chunk = os.read(descriptor, min(2_048, 8_553 - len(chunks)))
            if not chunk:
                break
            chunks.extend(chunk)
        after = os.fstat(descriptor)
        named = slot.lstat()
        if (len(chunks) > 8_552 or len(chunks) != before.st_size
                or (before.st_dev, before.st_ino, before.st_size)
                    != (after.st_dev, after.st_ino, after.st_size)
                or (after.st_dev, after.st_ino) != (named.st_dev, named.st_ino)
                or not stat.S_ISREG(named.st_mode) or named.st_uid != os.geteuid()
                or named.st_mode & 0o077):
            return None
    except OSError:
        return None
    finally:
        os.close(descriptor)

    record = bytes(chunks)
    if record[:8] != b"PLMPRV01" or len(record) < 72:
        return None
    launch_generation = int.from_bytes(record[8:16], "big")
    launch_lease = int.from_bytes(record[16:24], "big")
    launch_sequence = int.from_bytes(record[24:32], "big")
    fingerprint = record[32:64]
    path_len = int.from_bytes(record[66:68], "big")
    binding_len = int.from_bytes(record[68:72], "big")
    if (launch_generation != parsed_target[0] or not launch_lease or not launch_sequence
            or not any(fingerprint) or record[64] != 1 or record[65] not in (1, 2)
            or path_len > 4_096 or binding_len > 4_388
            or 72 + path_len + binding_len != len(record)):
        return None
    executable_path = record[72:72 + path_len]
    if executable_path and (not executable_path.startswith(b"/") or b"\0" in executable_path):
        return None
    binding = record[72 + path_len:]
    if len(binding) < 96 or binding[:8] != b"PLBIND01" or any(binding[80:96]):
        return None
    found_target = tuple(
        int.from_bytes(binding[offset:offset + 8], "big")
        for offset in (8, 16, 24, 32, 40)
    )
    bridge_instance = int.from_bytes(binding[48:56], "big")
    revision = int.from_bytes(binding[56:64], "big")
    verified_at = int.from_bytes(binding[64:72], "big")
    source = int.from_bytes(binding[72:74], "big")
    version_len = int.from_bytes(binding[74:76], "big")
    project_len = int.from_bytes(binding[76:78], "big")
    session_len = int.from_bytes(binding[78:80], "big")
    if (found_target != parsed_target or not bridge_instance or not revision or not verified_at
            or source not in (1, 2) or not 1 <= version_len <= 128
            or not 1 <= project_len <= 4_096 or not 1 <= session_len <= 128
            or 96 + version_len + project_len + session_len != len(binding)):
        return None
    version_start = 96
    project_start = version_start + version_len
    session_start = project_start + project_len
    version_bytes = binding[version_start:project_start]
    project_bytes = binding[project_start:session_start]
    session_bytes = binding[session_start:]
    if (not all(byte in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789.+-"
                for byte in version_bytes)
            or project_bytes != os.fsencode(project.resolve())
            or any(byte < 0x20 or byte > 0x7E for byte in project_bytes)
            or not all(byte in b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_.-"
                       for byte in session_bytes)):
        return None
    try:
        actual_session_id = session_bytes.decode("ascii")
        provider_version = version_bytes.decode("ascii")
    except UnicodeDecodeError:
        return None
    if ((expected_session_id is not None and actual_session_id != expected_session_id)
            or provider_version != EXPECTED_VERSION):
        return None
    return {
        "target": target,
        "revision": revision,
        "provider_version": provider_version,
        "provider_session_hash": hashlib.sha256(session_bytes).hexdigest(),
    }


def wait_live_managed_binding(directory: Path, target: str, project: Path,
                              expected_session_id: str, minimum_revision: int,
                              deadline: float) -> dict[str, Any] | None:
    while time.monotonic() < deadline:
        record = read_live_managed_binding(directory, target, project, expected_session_id)
        if record is not None and record["revision"] >= minimum_revision:
            return record
        time.sleep(0.025)
    return None


def managed_pane_status(binary: Path, directory: Path, context_dir: Path,
                        expected_target: str) -> dict[str, Any]:
    """Read only fixed pane liveness fields through Panelink's own CLI."""
    try:
        entries = sorted(context_dir.iterdir(), key=lambda path: path.name)
    except OSError:
        return {"status": "context_directory_unavailable"}
    for path in entries[:8]:
        try:
            metadata = path.lstat()
            if not path.is_file() or path.is_symlink() or metadata.st_size > 4096:
                continue
        except OSError:
            continue
        try:
            completed = subprocess.run(
                [str(binary), "control", "--directory", str(directory),
                 "--context", str(path), "panes"],
                cwd=Path.cwd(), env={"PATH": os.defpath, "HOME": str(context_dir.parent), "LANG": "C"},
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                timeout=2, check=False, close_fds=True, shell=False,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if completed.returncode != 0 or len(completed.stdout) > 4096:
            continue
        try:
            value = json.loads(completed.stdout)
        except (ValueError, UnicodeDecodeError):
            continue
        if (not isinstance(value, dict) or value.get("action") != "panes"
                or not isinstance(value.get("panes"), list)):
            continue
        for pane in value["panes"]:
            if not isinstance(pane, dict) or safe_target(pane.get("target")) != expected_target:
                continue
            live = pane.get("live")
            state = pane.get("state")
            exit_status = pane.get("exit_status")
            exit_code = exit_status.get("code") if isinstance(exit_status, dict) else None
            if (not isinstance(live, bool) or state not in {"applied", "exited"}
                    or (exit_code is not None and (not isinstance(exit_code, int)
                                                  or isinstance(exit_code, bool)
                                                  or not -255 <= exit_code <= 255))):
                return {"status": "pane_metadata_invalid"}
            return {"status": "known", "live": live, "state": state,
                    "exit_code": exit_code}
    return {"status": "context_or_pane_unavailable"}


def create_screen_observer(binary: Path, directory: Path, context_dir: Path,
                           target: str) -> bool:
    """Create a separate read-only fixture lease so the live attach lock is untouched."""
    try:
        context_dir.mkdir(mode=0o700)
    except OSError:
        return False
    path = context_dir / "observer.ctx"
    try:
        completed = subprocess.run(
            [str(binary), "control", "--directory", str(directory),
             "--context", str(path), "init", target],
            cwd=Path.cwd(), env={"PATH": os.defpath, "HOME": str(context_dir.parent), "LANG": "C"},
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=3, check=False, close_fds=True, shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    if completed.returncode != 0 or len(completed.stdout) > 4096:
        return False
    try:
        value = json.loads(completed.stdout)
        metadata = path.lstat()
    except (OSError, ValueError, UnicodeDecodeError):
        return False
    return (isinstance(value, dict) and value.get("ok") is True
            and value.get("action") == "init" and path.is_file() and not path.is_symlink()
            and metadata.st_size <= 4096 and metadata.st_mode & 0o077 == 0)


def managed_pane_screen_markers(binary: Path, directory: Path, context_dir: Path,
                                expected_target: str,
                                labels: tuple[str, str] | None = None,
                                fixture_titles: tuple[str, ...] | None = None,
                                target_label: str | None = None,
                                other_label: str | None = None,
                                filter_text: str | None = None,
                                filter_mode: str = "title",
                                fixture_session_id: str | None = None) -> dict[str, Any]:
    """Read a bounded screen in memory and return only fixed boolean markers."""
    try:
        entries = sorted(context_dir.iterdir(), key=lambda path: path.name)
    except OSError:
        return {"status": "context_directory_unavailable"}
    for path in entries[:8]:
        try:
            metadata = path.lstat()
            if not path.is_file() or path.is_symlink() or metadata.st_size > 4096:
                continue
        except OSError:
            continue
        try:
            completed = subprocess.run(
                [str(binary), "control", "--directory", str(directory),
                 "--context", str(path), "read", expected_target],
                cwd=Path.cwd(), env={"PATH": os.defpath, "HOME": str(context_dir.parent), "LANG": "C"},
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                timeout=2, check=False, close_fds=True, shell=False,
            )
        except (OSError, subprocess.SubprocessError):
            continue
        if completed.returncode != 0 or len(completed.stdout) > 256 * 1024:
            continue
        try:
            value = json.loads(completed.stdout)
        except (ValueError, UnicodeDecodeError):
            continue
        if (not isinstance(value, dict) or value.get("action") != "read"
                or safe_target(value.get("target")) != expected_target
                or not isinstance(value.get("text"), str)):
            continue
        original_screen = value["text"]
        screen = original_screen.lower()
        picker_region = session_picker_region(original_screen)
        picker_lines = picker_region.splitlines()
        sessions_line = next((index for index, line in enumerate(picker_lines)
                              if line.strip().lower() == "sessions"), None)
        projects_line = next((index for index, line in enumerate(picker_lines)
                              if line.strip().lower() == "projects"), None)
        if fixture_titles is None:
            fixture_titles = labels or ()
        known_titles = fixture_titles
        (session_results, session_rows_boundary, session_rows_overflowed,
         modal_bounds, session_rows_guard_reason) = (
            global_picker_session_result_rows(original_screen)
        )
        session_rows_boundary_verified = bool(
            session_results is not None and session_rows_boundary
        )
        session_result_row_count = (
            len(session_results) if session_results is not None and session_rows_boundary else None
        )
        unknown_selectable_row_count = (
            sum(not any(title.lower() in line.lower() for title in known_titles)
                for line in session_results)
            if session_result_row_count is not None and not session_rows_overflowed else None
        )
        if filter_text is None:
            filter_text = target_label
        if filter_mode not in {"title", "fixture_session_id"}:
            filter_text = None
        session_id_query = (
            filter_mode == "fixture_session_id"
            and fixture_session_id is not None
            and SESSION_ID_RE.fullmatch(fixture_session_id) is not None
            and filter_text == fixture_session_id
        )
        target_fixture_index = (fixture_titles.index(target_label)
                                if target_label in fixture_titles else None)
        return {
            "status": "known",
            "screen_bytes": len(value["text"].encode("utf-8")),
            "screen_row_count": len(original_screen.splitlines()),
            "screen_row_widths": [len(line) for line in original_screen.splitlines()[:PTY_SIZE[0]]],
            "contains_opencode_title": "oc |" in screen or "opencode" in screen,
            "contains_open_session_title": "open session or project" in screen,
            "contains_recent_sessions_title": "recent sessions" in screen,
            "contains_sessions_title": "sessions" in screen,
            "contains_session_a": "panelink v2 identity a" in screen,
            "contains_session_b": "panelink v2 identity b" in screen,
            "contains_picker_heading": session_picker_heading(original_screen),
            "contains_open_menu_heading": (
                picker_heading_marker(original_screen) == "Open"
                and session_picker_heading(original_screen)
            ),
            "contains_sessions_category": any(
                line.strip().lower() == "sessions" for line in picker_region.splitlines()
            ),
            "contains_both_fixture_labels": (all(label in picker_region for label in labels)
                                              if labels is not None else False),
            "contains_any_fixture_label": (any(label in picker_region for label in labels)
                                            if labels is not None else False),
            "contains_target_label": (target_label in picker_region
                                       if target_label is not None else False),
            "contains_target_title_anywhere": (target_label.lower() in screen
                                                 if target_label is not None else False),
            "contains_competitor_label": (other_label in picker_region
                                          if other_label is not None else False),
            "target_fixture_index": target_fixture_index,
            "fixture_title_count": len(fixture_titles),
            "filter_query_is_target_label": filter_text == target_label,
            "filter_query_mode": filter_mode,
            "filter_query_is_fixture_session_id": session_id_query,
            "contains_projects_category": any(
                line.strip().lower() == "projects" for line in picker_region.splitlines()
            ),
            "unknown_selectable_row_count": unknown_selectable_row_count,
            "session_result_row_count": session_result_row_count,
            "session_rows_boundary_verified": session_rows_boundary_verified,
            "session_rows_overflowed": session_rows_overflowed,
            "session_rows_guard_reason": session_rows_guard_reason,
            "session_modal_bounds": modal_bounds,
            "screen_positions": {
                **fixed_screen_positions(original_screen, {
                "heading": picker_heading_marker(original_screen),
                "sessions_category": "Sessions",
                "projects_category": "Projects",
                **({f"fixture_label_{index}": label
                   for index, label in enumerate(fixture_titles)} if fixture_titles else {}),
                **({"target": target_label} if target_label is not None else {}),
                **({"competitor": other_label} if other_label is not None else {}),
                **({"filter_query": filter_text} if filter_text is not None else {}),
                }),
                "heading": picker_heading_position(original_screen),
                "sessions_category": exact_line_positions(original_screen, "Sessions"),
                "projects_category": exact_line_positions(original_screen, "Projects"),
            },
            **managed_owner_markers(binary, directory, path, expected_target),
            "contains_error_marker": "error" in screen or "failed" in screen,
            "contains_provider_prompt": "connect a provider" in screen,
        }
    return {"status": "context_or_screen_unavailable"}


def post_enter_state(binary: Path, directory: Path, context_dir: Path,
                     expected_target: str, labels: tuple[str, str],
                     fixture_titles: tuple[str, ...] | None = None) -> dict[str, Any]:
    """Capture fixed pane/owner/screen markers without retaining screen text."""
    screen = managed_pane_screen_markers(
        binary, directory, context_dir, expected_target, labels=labels,
        fixture_titles=fixture_titles,
    )
    pane = managed_pane_status(binary, directory, context_dir, expected_target)
    return {
        "screen_status": screen.get("status"),
        "contains_opencode_title": screen.get("contains_opencode_title") is True,
        "contains_session_a": screen.get("contains_session_a") is True,
        "contains_session_b": screen.get("contains_session_b") is True,
        "contains_picker_heading": screen.get("contains_picker_heading") is True,
        "contains_open_session_title": screen.get("contains_open_session_title") is True,
        "contains_projects_category": screen.get("contains_projects_category") is True,
        "input_owner": screen.get("input_owner"),
        "owner_revision": screen.get("owner_revision"),
        "child_exited": screen.get("child_exited"),
        "screen_positions": screen.get("screen_positions"),
        "pane_status": pane.get("status"),
        "pane_live": pane.get("live"),
        "pane_state": pane.get("state"),
    }


def managed_acceptance(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    """Start two real managed TUI children and verify snapshots and durability."""
    try:
        from scripts.p4_managed_transition_fixtures import (
            create_managed_transition_fixtures,
            safe_evidence as managed_transition_safe_evidence,
        )
    except ModuleNotFoundError:
        from p4_managed_transition_fixtures import (
            create_managed_transition_fixtures,
            safe_evidence as managed_transition_safe_evidence,
        )
    root: Path = context["root"]
    project: Path = context["project"]
    session_a, session_b = context["session_ids"]
    labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")
    result: dict[str, Any] = {
        "classification": "blocked",
        "reason": "not_started",
        "managed_serve_count": 0,
        "initial_last_confirmed": False,
        "initial_a_binding_verified": False,
        "root_a_to_b_to_a": False,
        "keyboard_route_proven": False,
        "passive_b_unchanged": False,
        "home_stale_and_returned": False,
        "historical_after_stop": False,
        "historical_identity_matches": False,
        "historical_revision_advanced": False,
        "current_at_crash": False,
        "automatic_resume": False,
        "prompt_submissions": 0,
        "attempted_control_count": 0,
        "private_noncolliding_keymap": False,
        "user_message_absence_verified": False,
        "cleanup_completed": False,
        "targets": [],
        "final_revisions": [],
        "historical_lookup": [],
        "pane_status": None,
        "pane_screen": None,
        "bridge_snapshots": [],
        "binding_revision_advanced_for_b": False,
        "binding_revision_advanced_for_a": False,
        "interim_b_binding_verified": False,
        "post_enter_binding_matches_a": False,
        "post_enter_binding_matches_b": False,
        "post_enter_binding_revision": None,
        "post_enter_actual_binding": None,
        "post_enter_state": None,
        "post_enter_after_500ms": None,
        "return_a_binding_verified": False,
        "managed_new_root_selected": False,
        "managed_imported_fork_selected": False,
        "managed_fork_returned_to_a": False,
        "managed_foreign_project_refused": False,
        "managed_foreign_project_returned_to_a": False,
        "foreign_route_evidence": None,
        "foreign_return_route_evidence": None,
        "route_observer_initial": [],
        "route_observer_home_return": None,
        "route_observer_failure": None,
        "transition_fixtures": None,
        "transition_binding_checkpoints": [],
        "live_binding_checkpoints": [],
        "user_message_absence_before": False,
        "user_message_absence_after": False,
        "serve_lines_valid": True,
        "tui_markers": {"bytes": 0},
    }
    terminfopath = compile_terminfo(root)
    if terminfopath is None:
        result["reason"] = "private_terminfo_compilation_failed"
        return result
    child_env = dict(context["env"])
    private_config = pinned_cli_config_content(Path(context["executable"]))
    if private_config is None:
        result["reason"] = "pinned_cli_keybind_config_unverified"
        return result
    # This is an ephemeral environment override for the two disposable native
    # children. OpenCode merges it with the launch-owned cli.json so its plugin
    # registration remains active; no user config file is read or changed.
    child_env["OPENCODE_CLI_CONFIG_CONTENT"] = private_config
    result["private_noncolliding_keymap"] = True
    child_env.update({"TERM": "xterm-256color",
                      "OPENCODE_SERVER_PASSWORD": context["password"]})
    child_env.pop("TERMINFO", None)
    # Build the extra API-owned transition roots before managed TUI children
    # connect. This keeps fixture construction from stealing their client
    # input lease while still making exact roots available in the native list.
    transition_fixtures, transition_checks = create_managed_transition_fixtures(
        context, project,
    )
    result["transition_fixtures"] = managed_transition_safe_evidence(
        transition_fixtures, transition_checks,
    )
    if transition_fixtures is None:
        result["reason"] = "managed_transition_fixtures_unverified"
        return result
    fixture_titles = (
        *labels,
        transition_fixtures.new_root_title,
        transition_fixtures.fork_root_title,
        "Panelink recovery imported fork parent",
        "Panelink recovery fork seed",
        transition_fixtures.foreign_root_title,
    )
    server_dirs = [root / "serve-a", root / "serve-b"]
    for path in server_dirs:
        path.mkdir(mode=0o700)
    handles: list[ServeProcess] = []
    route_paths: list[Path] = []
    route_nonces: list[str] = []
    route_initial: list[dict[str, Any]] = []
    route_projects = {transition_fixtures.project_sha256,
                      transition_fixtures.foreign_project_sha256}
    attach: PrivatePty | None = None
    started: list[tuple[int, int, int, int, int]] = []
    try:
        for index, (directory, identifier) in enumerate(zip(server_dirs, (session_a, session_b))):
            # Add the existing read-only test observer alongside the unchanged
            # production plugin. Both must load: production bindings and this
            # independently nonce-checked stream are required below. Keep the
            # plugin and its output beside the storage directory, because
            # ServerStorage initialization requires its directory to be empty.
            observer_dir = root / f"route-observer-{index}"
            observer_dir.mkdir(mode=0o700)
            route_path = root / f"route-observer-{index}.jsonl"
            route_path.touch(mode=0o600)
            nonce = secrets.token_hex(16)
            observer_source = (Path(__file__).resolve().parents[1] / "docs/progress"
                               / "p4-v2-tui-plugin-fixture/plugin.mjs").read_text()
            observer_source = observer_source.replace(
                "process.env.PANELINK_PROBE_RECORDS", json.dumps(str(route_path))).replace(
                "process.env.PANELINK_PROBE_CLIENT_NONCE", json.dumps(nonce))
            observer_entry = observer_dir / "tui.ts"
            observer_entry.write_text(observer_source)
            observer_entry.chmod(0o600)
            route_paths.append(route_path)
            route_nonces.append(nonce)
            native_env = dict(child_env)
            native_config = managed_cli_config_content(
                Path(context["executable"]), observer_dir,
            )
            if native_config is None:
                result["reason"] = "managed_plugin_config_unverified"
                return result
            native_env["OPENCODE_CLI_CONFIG_CONTENT"] = native_config
            argv = [str(binary), "serve", "--directory", str(directory), "--terminfo",
                    str(terminfopath), "--initialize", "--allow-input", "--opencode-bridge",
                    "--", context["executable"], "--server", context["base"],
                    "--session", identifier, str(project)]
            # Targets use the first generation/workspace/pane/session/occupant tuple.
            handle = ServeProcess(argv, project, native_env, expected_target="")
            handles.append(handle)
        result["managed_serve_count"] = len(handles)
        serve_deadline = min(context["deadline"], time.monotonic() + CHILD_DEADLINE)
        def startup_unsettled(handle: ServeProcess) -> bool:
            # An exited child can still have stdout/stderr buffered in its
            # pipes. Keep draining until both streams reach EOF before
            # classifying a startup failure.
            return (handle.startup is None and
                    (handle.process.poll() is None or bool(handle.selector.get_map())))

        while time.monotonic() < serve_deadline and any(startup_unsettled(handle) for handle in handles):
            for handle in handles:
                if handle.startup is None:
                    handle.poll(0.0)
            time.sleep(0.05)
        for handle in handles:
            if handle.startup is not None:
                started.append(parse_target(handle.startup["target"]))  # type: ignore[arg-type]
            if handle.startup is None or handle.invalid_line:
                failure = handle.failure
                result["managed_serve_startup_diagnostics"] = [
                    {
                        "startup_seen": item.startup is not None,
                        "recognized_failure": item.failure,
                        "process_alive": item.process.poll() is None,
                        "exit_code": item.process.poll(),
                        "event_count": item.event_count,
                        "invalid_line": item.invalid_line,
                        "stderr_category": item.stderr_category(),
                    }
                    for item in handles
                ]
                result.update(reason=(f"managed_serve_{failure}" if failure
                                      else "managed_serve_startup_unverified"))
                return result
        if len(handles) != 2 or len(started) != 2:
            result["reason"] = "managed_serve_count_mismatch"
            return result
        if handles[0].expected_target == handles[1].expected_target:
            result["reason"] = "managed_targets_not_unique"
            return result
        result["targets"] = [handle.expected_target for handle in handles]

        attach_context = root / "attach-context"
        attach_context.mkdir(mode=0o700)
        screen_observer_context = root / "screen-observer-context"
        screen_observer_available = create_screen_observer(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target,
        )
        result["screen_observer_available"] = screen_observer_available
        # Match the repository's attach-host fixture: keep the interactive
        # client environment minimal and never pass the native provider's
        # server password or plugin socket capability to Panelink itself.
        attach_env = {
            "PATH": os.defpath,
            "HOME": child_env.get("HOME", str(root)),
            "ZDOTDIR": child_env.get("HOME", str(root)),
            "PS1": "P4MB$ ",
        }
        try:
            attach = PanelinkAttachPty(
                [str(binary), "attach", "--directory", str(server_dirs[0]),
                 "--context-dir", str(attach_context)],
                project,
                attach_env,
                attach_context,
            )
        except (OSError, subprocess.SubprocessError):
            result["reason"] = "panelink_attach_tty_setup_failed"
            return result
        attach_deadline = min(context["deadline"], time.monotonic() + 5.0)
        if not wait_for(lambda: attach.alive() and attach.raw_mode() and attach.output_ready(), attach_deadline,
                        pty_child=attach):
            result["reason"] = "panelink_attach_not_ready"
            result["tui_markers"] = screen_markers(attach)
            return result

        first, second = wait_initial_pair(
            handles, min(context["deadline"], time.monotonic() + 18.0), attach=attach,
        )
        if first is None or second is None:
            result["reason"] = "initial_managed_snapshot_not_confirmed"
            result["tui_markers"] = screen_markers(attach)
            return result
        result["initial_last_confirmed"] = True
        initial_b_revision = second["last_confirmed_revision"]
        initial_a_binding = wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
            first["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        )
        result["initial_a_binding_verified"] = bool(
            initial_a_binding
            and initial_a_binding["revision"] == first["last_confirmed_revision"]
        )
        if not result["initial_a_binding_verified"]:
            result["reason"] = "initial_exact_binding_not_verified"
            return result
        result["live_binding_checkpoints"].append({
            "route": "A", "revision": initial_a_binding["revision"],
            "provider_session_hash": initial_a_binding["provider_session_hash"],
        })
        # Prove both independently loaded nonce streams before the first UI
        # key. This distinguishes a bad/missing observer from later UI routing.
        initial_routes: list[dict[str, Any] | None] = [None, None]
        for index, session_id in enumerate((session_a, session_b)):
            expected_session_hash = hashlib.sha256(session_id.encode("ascii")).hexdigest()
            initial_routes[index] = wait_fixture_route(
                route_paths[index], route_nonces[index], route_projects,
                transition_fixtures.project_sha256, expected_session_hash,
                instance=None, after_seq=0,
                deadline=min(context["deadline"], time.monotonic() + 5.0),
            )
            _record, observer_status = fixture_route_diagnostic(
                route_paths[index], route_nonces[index], route_projects,
            )
            observer_status["expected_project_match"] = (
                observer_status.get("latest_project_sha256")
                == transition_fixtures.project_sha256
            )
            observer_status["expected_session_match"] = (
                observer_status.get("latest_session_sha256") == expected_session_hash
            )
            observer_status["exact_initial_route"] = initial_routes[index] is not None
            result["route_observer_initial"].append(observer_status)
        if any(route is None for route in initial_routes):
            result["route_observer_failure"] = "initial_exact_route_unverified"
            result["reason"] = "initial_route_observer_unverified"
            return result
        initial_route_a = initial_routes[0]
        initial_route_b = initial_routes[1]
        assert initial_route_a is not None and initial_route_b is not None
        if (initial_route_a.get("tui_instance") == initial_route_b.get("tui_instance")
                or initial_route_a.get("seq", 0) <= 0 or initial_route_b.get("seq", 0) <= 0):
            result["route_observer_failure"] = "initial_instance_or_sequence_invalid"
            result["reason"] = "initial_route_observer_unverified"
            return result
        no_user_messages, _message_statuses = verify_no_user_messages(
            context["base"], context["password"], (session_a, session_b),
        )
        result["user_message_absence_before"] = no_user_messages
        if not no_user_messages:
            result["reason"] = "fixture_user_messages_present_before_switch"
            return result
        # A fresh Panelink pane read and PTY transcript precede the input.
        # The same checks are repeated for the return route below.
        pre_switch_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
        )
        result["attempted_control_count"] += 1
        switch_trace: dict[str, bool] = {}
        switch_child_read = lambda: managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
            labels=labels, fixture_titles=fixture_titles,
            target_label=labels[1], other_label=labels[0],
        )
        switch_to_b = select_session(
            attach, labels[1], labels[0], labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            switch_trace, pre_read=pre_switch_screen, fresh_child_read=switch_child_read,
            fixture_titles=fixture_titles,
        )
        result["switch_trace"] = switch_trace
        if switch_to_b:
            result["post_enter_state"] = post_enter_state(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target, labels, fixture_titles,
            )
            time.sleep(0.5)
            result["post_enter_after_500ms"] = post_enter_state(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target, labels, fixture_titles,
            )
        observed_b = (confirmation_after(handles[0], first["last_confirmed_revision"],
                                         min(context["deadline"], time.monotonic() + 5.0))
                      if switch_to_b else None)
        live_b = (wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_b,
            observed_b["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        ) if observed_b is not None else None)
        if observed_b is not None:
            actual_binding = read_live_managed_binding(
                server_dirs[0], handles[0].expected_target, project, None,
            )
            if actual_binding is not None:
                candidates = {
                    "A": session_a, "B": session_b,
                    "new_root": transition_fixtures.new_root_id,
                    "fork_root": transition_fixtures.fork_root_id,
                    "fork_parent": transition_fixtures.fork_parent_id,
                    "foreign_root": transition_fixtures.foreign_root_id,
                }
                result["post_enter_actual_binding"] = {
                    **actual_binding,
                    "fixture": next((name for name, identifier in candidates.items()
                        if hashlib.sha256(identifier.encode("ascii")).hexdigest()
                        == actual_binding["provider_session_hash"]), "other"),
                }
            current_a_binding = read_live_managed_binding(
                server_dirs[0], handles[0].expected_target, project, session_a,
            )
            current_b_binding = (live_b or read_live_managed_binding(
                server_dirs[0], handles[0].expected_target, project, session_b,
            ))
            result["post_enter_binding_matches_a"] = current_a_binding is not None
            result["post_enter_binding_matches_b"] = current_b_binding is not None
            current_binding = current_b_binding or current_a_binding
            result["post_enter_binding_revision"] = (
                current_binding["revision"] if current_binding is not None else None
            )
        result["interim_b_binding_verified"] = bool(
            live_b and live_b["revision"] >= 2
            and observed_b and live_b["revision"] >= observed_b["last_confirmed_revision"]
        )
        if live_b is not None:
            result["live_binding_checkpoints"].append({
                "route": "B", "revision": live_b["revision"],
                "provider_session_hash": live_b["provider_session_hash"],
            })
        result["binding_revision_advanced_for_b"] = bool(
            observed_b and observed_b["status"] == "last_confirmed"
            and observed_b["last_confirmed_revision"] >= 2
            and observed_b["last_confirmed_revision"] > first["last_confirmed_revision"]
            and result["interim_b_binding_verified"]
        )
        if observed_b is not None:
            result["switch_b_binding_revision"] = observed_b["last_confirmed_revision"]
        switch_back_a = False
        observed_a = None
        if result["binding_revision_advanced_for_b"]:
            return_trace: dict[str, bool] = {}
            return_child_read = lambda: managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
                labels=labels, fixture_titles=fixture_titles,
                target_label=labels[0], other_label=labels[1],
            )
            pre_return_screen = managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
            )
            switch_back_a = select_session(
                attach, labels[0], labels[1], labels,
                min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
                return_trace, pre_read=pre_return_screen, fresh_child_read=return_child_read,
                fixture_titles=fixture_titles,
            )
            result["return_trace"] = return_trace
            observed_a = (confirmation_after(handles[0], observed_b["last_confirmed_revision"],
                                             min(context["deadline"], time.monotonic() + 5.0))
                          if switch_back_a else None)
            live_a = (wait_live_managed_binding(
                server_dirs[0], handles[0].expected_target, project, session_a,
                observed_a["last_confirmed_revision"],
                min(context["deadline"], time.monotonic() + 3.0),
            ) if observed_a is not None else None)
            result["return_a_binding_verified"] = bool(
                live_a and live_a["revision"] > observed_b["last_confirmed_revision"]
                and observed_a and live_a["revision"] >= observed_a["last_confirmed_revision"]
            )
            if live_a is not None:
                result["live_binding_checkpoints"].append({
                    "route": "A", "revision": live_a["revision"],
                    "provider_session_hash": live_a["provider_session_hash"],
                })
            result["binding_revision_advanced_for_a"] = bool(
                observed_a and observed_a["status"] == "last_confirmed"
                and observed_a["last_confirmed_revision"] > observed_b["last_confirmed_revision"]
                and result["return_a_binding_verified"]
            )
            if observed_a is not None:
                result["switch_a_binding_revision"] = observed_a["last_confirmed_revision"]
        result["root_a_to_b_to_a"] = bool(
            result["binding_revision_advanced_for_b"]
            and result["binding_revision_advanced_for_a"]
        )
        result["keyboard_route_proven"] = bool(
            switch_trace.get("enter_sent") is True
            and isinstance(result.get("return_trace"), dict)
            and result["return_trace"].get("enter_sent") is True
            and result["root_a_to_b_to_a"]
        )
        handles[1].poll(0.0)
        latest_b = handles[1].latest
        result["passive_b_unchanged"] = bool(
            latest_b and latest_b["status"] == "last_confirmed"
            and latest_b["last_confirmed_revision"] == initial_b_revision
        )
        if not result["root_a_to_b_to_a"]:
            inspection_context = screen_observer_context if screen_observer_available else attach_context
            result["pane_status"] = managed_pane_status(
                binary, server_dirs[0], inspection_context, handles[0].expected_target,
            )
            result["pane_screen"] = managed_pane_screen_markers(
                binary, server_dirs[0], inspection_context, handles[0].expected_target,
            )
            result["reason"] = "managed_session_switch_not_proven"
            return result

        # Select the API-created, independently verified new root through the
        # same native picker, then do the same for a nonempty imported-history
        # fork. The fixture IDs remain in memory and only their hashes leave.
        transition_revision = observed_a["last_confirmed_revision"]
        for route, session_id, title, other_title, expected_hash in (
            ("new_root", transition_fixtures.new_root_id,
             transition_fixtures.new_root_title, labels[0],
             transition_fixtures.new_root_sha256),
            ("imported_fork", transition_fixtures.fork_root_id,
             transition_fixtures.fork_root_title, transition_fixtures.new_root_title,
             transition_fixtures.fork_root_sha256),
        ):
            pre_transition_screen = managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target,
            )
            transition_trace: dict[str, Any] = {}
            target_read = lambda title=title, other_title=other_title: managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target, labels=labels,
                fixture_titles=fixture_titles,
                target_label=title, other_label=other_title,
            )
            selected = select_session(
                attach, title, other_title, labels,
                min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
                transition_trace, pre_read=pre_transition_screen,
                fresh_child_read=target_read,
                fixture_titles=fixture_titles,
            )
            result[f"{route}_trace"] = transition_trace
            observed = (confirmation_after(
                handles[0], transition_revision,
                min(context["deadline"], time.monotonic() + 5.0),
            ) if selected else None)
            live = (wait_live_managed_binding(
                server_dirs[0], handles[0].expected_target, project, session_id,
                observed["last_confirmed_revision"],
                min(context["deadline"], time.monotonic() + 3.0),
            ) if observed is not None else None)
            exact = bool(
                observed and observed["status"] == "last_confirmed"
                and live and live["provider_session_hash"] == expected_hash
                and live["revision"] >= observed["last_confirmed_revision"]
                and observed["last_confirmed_revision"] > transition_revision
            )
            result[f"managed_{route}_selected"] = exact
            if not exact:
                result["reason"] = f"managed_{route}_selection_not_proven"
                return result
            result["transition_binding_checkpoints"].append({
                "route": route, "revision": live["revision"],
                "provider_session_hash": live["provider_session_hash"],
            })
            transition_revision = observed["last_confirmed_revision"]

        pre_fork_return = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target,
        )
        fork_return_trace: dict[str, Any] = {}
        fork_return_child = lambda: managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target, labels=labels,
            fixture_titles=fixture_titles,
            target_label=labels[0], other_label=transition_fixtures.fork_root_title,
        )
        returned_to_a = select_session(
            attach, labels[0], transition_fixtures.fork_root_title, labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            fork_return_trace, pre_read=pre_fork_return,
            fresh_child_read=fork_return_child,
            fixture_titles=fixture_titles,
        )
        result["fork_return_trace"] = fork_return_trace
        fork_returned = (confirmation_after(
            handles[0], transition_revision,
            min(context["deadline"], time.monotonic() + 5.0),
        ) if returned_to_a else None)
        fork_return_live = (wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
            fork_returned["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        ) if fork_returned is not None else None)
        result["managed_fork_returned_to_a"] = bool(
            fork_returned and fork_return_live
            and fork_return_live["provider_session_hash"] == initial_a_binding["provider_session_hash"]
            and fork_return_live["revision"] >= fork_returned["last_confirmed_revision"]
            and fork_returned["last_confirmed_revision"] > transition_revision
        )
        if not result["managed_fork_returned_to_a"]:
            result["reason"] = "managed_fork_return_to_a_not_proven"
            return result
        result["transition_binding_checkpoints"].append({
            "route": "A_after_fork", "revision": fork_return_live["revision"],
            "provider_session_hash": fork_return_live["provider_session_hash"],
        })
        transition_revision = fork_returned["last_confirmed_revision"]

        # Open the native new-session route with its private fixture-only
        # Ctrl-N mapping. The source must lose certainty before returning to A.
        pre_home_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
        )
        if (pre_home_screen.get("status") != "known"
                or pre_home_screen.get("contains_opencode_title") is not True
                or pre_home_screen.get("input_owner") != "client"):
            result["reason"] = "fresh_home_pre_action_read_unverified"
            return result
        attach.drain(0.05)
        if not attach.screen_text() or not attach.alive():
            result["reason"] = "home_pre_action_screen_unavailable"
            return result
        result["attempted_control_count"] += 1
        home_before_revision = transition_revision
        attach.send(b"\x0e")  # fixture-only mapping: session.new
        home_stale = status_after(
            handles[0], "stale", min(context["deadline"], time.monotonic() + 5.0),
        )
        result["home_stale_observed"] = bool(
            home_stale and home_stale["last_confirmed_revision"] == home_before_revision
        )
        preserved_a = read_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
        )
        result["home_preserved_last_confirmed_a"] = bool(
            preserved_a and preserved_a["revision"] == home_before_revision
            and preserved_a["provider_session_hash"] == initial_a_binding["provider_session_hash"]
        )
        # Return only through a fresh, unique native session-list result.
        pre_home_return_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
        )
        home_route_watermark = read_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
        )
        if (not isinstance(home_route_watermark, dict)
                or home_route_watermark.get("tui_instance") != initial_route_a.get("tui_instance")):
            result["route_observer_failure"] = "home_return_watermark_unverified"
            result["reason"] = "home_return_observer_unverified"
            return result
        home_return_trace: dict[str, Any] = {}
        home_return_child = lambda: managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
            labels=labels, fixture_titles=fixture_titles,
            target_label=labels[0], other_label=labels[1],
        )
        returned = select_session(
            attach, labels[0], labels[1], labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            home_return_trace, pre_read=pre_home_return_screen,
            fresh_child_read=home_return_child,
            fixture_titles=fixture_titles,
        )
        result["home_return_trace"] = home_return_trace
        home_route = (wait_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
            transition_fixtures.project_sha256,
            hashlib.sha256(session_a.encode("ascii")).hexdigest(),
            initial_route_a.get("tui_instance"), home_route_watermark.get("seq", 0),
            min(context["deadline"], time.monotonic() + 5.0),
        ) if returned else None)
        _home_record, home_route_diagnostic = fixture_route_diagnostic(
            route_paths[0], route_nonces[0], route_projects,
        )
        home_route_diagnostic["same_initial_instance"] = bool(
            home_route is not None
            and home_route.get("tui_instance") == initial_route_a.get("tui_instance")
        )
        home_route_diagnostic["sequence_advanced"] = bool(
            home_route is not None
            and home_route.get("seq", 0) > home_route_watermark.get("seq", 0)
        )
        home_route_diagnostic["exact_home_return_route"] = home_route is not None
        result["route_observer_home_return"] = home_route_diagnostic
        if home_route is None:
            result["route_observer_failure"] = "fresh_home_return_route_unverified"
            result["reason"] = "home_return_observer_unverified"
            return result
        restored = status_after(handles[0], "last_confirmed",
                                min(context["deadline"], time.monotonic() + 5.0)) if returned else None
        restored_a = (wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
            restored["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        ) if restored is not None else None)
        result["home_stale_and_returned"] = bool(
            result["home_stale_observed"] and result["home_preserved_last_confirmed_a"]
            and restored and restored["last_confirmed_revision"] > home_before_revision
            and restored_a and restored_a["provider_session_hash"]
            == initial_a_binding["provider_session_hash"]
            and restored_a["revision"] >= restored["last_confirmed_revision"]
        )
        if not result["home_stale_and_returned"]:
            result["reason"] = "home_stale_return_not_proven"
            return result
        result["live_binding_checkpoints"].append({
            "route": "A_after_home", "revision": restored_a["revision"],
            "provider_session_hash": restored_a["provider_session_hash"],
        })

        # Switch through the pinned public open-menu command. Require the
        # independent nonce/sequence observer to map the same TUI instance to
        # the exact foreign project and session before evaluating refusal.
        route_before_foreign = read_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
        )
        route_before_foreign_seq = (route_before_foreign.get("seq", 0)
                                     if isinstance(route_before_foreign, dict) else 0)
        route_instance = (route_before_foreign.get("tui_instance")
                          if isinstance(route_before_foreign, dict) else None)
        if (not fixture_route_matches(
                route_before_foreign, transition_fixtures.project_sha256,
                hashlib.sha256(session_a.encode("ascii")).hexdigest(),
                after_seq=0)
                or not isinstance(route_instance, str)):
            result["reason"] = "foreign_route_precondition_unverified"
            return result
        pre_foreign_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target,
        )
        foreign_trace: dict[str, Any] = {}
        foreign_child_read = lambda: managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target, labels=labels,
            fixture_titles=fixture_titles,
            target_label=transition_fixtures.foreign_root_title,
            other_label=labels[0],
            filter_text=transition_fixtures.foreign_root_title,
        )
        selected_foreign = select_session(
            attach, transition_fixtures.foreign_root_title, labels[0], labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            foreign_trace, pre_read=pre_foreign_screen,
            fresh_child_read=foreign_child_read, open_chord=b"\x0f",
            fixture_titles=fixture_titles,
            filter_text=transition_fixtures.foreign_root_title,
        )
        result["foreign_project_trace"] = foreign_trace
        foreign_revision = restored["last_confirmed_revision"]
        foreign_session_hash = hashlib.sha256(
            transition_fixtures.foreign_root_id.encode("ascii"),
        ).hexdigest()
        foreign_route = (wait_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
            transition_fixtures.foreign_project_sha256, foreign_session_hash,
            route_instance, route_before_foreign_seq,
            min(context["deadline"], time.monotonic() + 4.0),
        ) if selected_foreign else None)
        result["foreign_route_evidence"] = {
            "acknowledged": foreign_route is not None,
            "same_tui_instance": bool(
                foreign_route is not None
                and foreign_route.get("tui_instance") == route_instance
            ),
            "sequence_advanced": bool(
                foreign_route is not None
                and foreign_route.get("seq", 0) > route_before_foreign_seq
            ),
            "project_sha256": (transition_fixtures.foreign_project_sha256
                               if foreign_route is not None else None),
            "session_sha256": (foreign_session_hash if foreign_route is not None else None),
            "sequence": (foreign_route.get("seq") if foreign_route is not None else None),
        }
        refusal_deadline = min(context["deadline"], time.monotonic() + 5.0)
        refusal_snapshot = None
        while foreign_route is not None and time.monotonic() < refusal_deadline:
            handles[0].poll(0.05)
            candidate = handles[0].latest
            if (candidate and candidate["status"] in {"stale", "unavailable"}
                    and candidate["last_confirmed_revision"] == foreign_revision):
                refusal_snapshot = candidate
                break
        preserved_foreign = read_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
        )
        foreign_surface = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target, labels=labels,
            fixture_titles=fixture_titles,
            target_label=transition_fixtures.foreign_root_title,
            other_label=labels[0],
        )
        # The independent route observer is the identity authority. The
        # rendered title remains useful diagnostic evidence, while the live
        # pane/owner and a closed picker establish that the route is settled.
        surface_deadline = min(context["deadline"], time.monotonic() + 3.0)
        foreign_surface_pane = managed_pane_status(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target,
        )
        def settled_foreign_surface() -> bool:
            return bool(
                foreign_surface.get("status") == "known"
                and foreign_surface.get("contains_picker_heading") is False
                and foreign_surface.get("input_owner") == "client"
                and foreign_surface_pane.get("live") is True
                and foreign_surface_pane.get("state") == "applied"
            )

        while (foreign_route is not None and time.monotonic() < surface_deadline
               and attach.alive() and not settled_foreign_surface()):
            handles[0].poll(0.05)
            foreign_surface = managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target, labels=labels,
                fixture_titles=fixture_titles,
                target_label=transition_fixtures.foreign_root_title,
                other_label=labels[0],
            )
            foreign_surface_pane = managed_pane_status(
                binary, server_dirs[0], screen_observer_context,
                handles[0].expected_target,
            )
        result["foreign_session_surface"] = {
            "screen_status": foreign_surface.get("status"),
            "foreign_title_visible": (
                foreign_surface.get("contains_target_title_anywhere") is True
            ),
            "session_picker_closed": (
                foreign_surface.get("contains_picker_heading") is False
            ),
            "input_owner": foreign_surface.get("input_owner"),
            "owner_revision": foreign_surface.get("owner_revision"),
            "pane_live": foreign_surface_pane.get("live") is True,
            "pane_state": foreign_surface_pane.get("state"),
        }
        # Exercise the public prepare command against the exact foreign
        # session while it remains selected in this TUI. This is a refusal
        # check only: prepare must not navigate or produce a ticket.
        try:
            from scripts.probe_panelink_manual_recovery import _cli as manual_cli
            from scripts.probe_panelink_manual_recovery import _manual_error as manual_error
        except ModuleNotFoundError:
            from probe_panelink_manual_recovery import _cli as manual_cli
            from probe_panelink_manual_recovery import _manual_error as manual_error

        foreign_before_refusal = read_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
        )
        foreign_before_refusal_valid = fixture_route_matches(
            foreign_before_refusal, transition_fixtures.foreign_project_sha256,
            foreign_session_hash, instance=route_instance,
            after_seq=(foreign_route.get("seq", 0) - 1 if foreign_route else 0),
        )
        before_refusal_seq = (foreign_before_refusal.get("seq")
                              if foreign_before_refusal_valid else None)
        manual_cli_result = (manual_cli(
            binary, "prepare", server_dirs[0], project,
            handles[0].expected_target,
            session_id=transition_fixtures.foreign_root_id,
            timeout=12.0,
        ) if before_refusal_seq is not None else {"valid": False})
        accepted_manual_errors = (
            "provider_recovery_unavailable", "provider_project_mismatch",
        )
        manual_refusal_error = next((error for error in accepted_manual_errors
                                     if manual_error(manual_cli_result, error)), None)
        manual_refusal_no_ticket = (
            manual_refusal_error is not None
            and manual_cli_result.get("ticket") is None
            and manual_cli_result.get("selected_confirmed") is not True
        )
        manual_state = manual_cli_result.get("state")
        manual_error_value = manual_cli_result.get("error")
        result["foreign_manual_refusal"] = {
            "valid": manual_cli_result.get("valid") is True,
            "returncode": (manual_cli_result.get("returncode")
                           if type(manual_cli_result.get("returncode")) is int else None),
            "state": (manual_state if manual_state == "manual_selection_required"
                      else "unexpected"),
            "error": (manual_refusal_error if manual_refusal_error is not None
                      else "unexpected" if manual_error_value is not None else None),
            "revision": (manual_cli_result.get("revision")
                         if type(manual_cli_result.get("revision")) is int else None),
            "ticket_present": manual_cli_result.get("ticket") is not None,
            "selected_confirmed": manual_cli_result.get("selected_confirmed") is True,
            "automatic_resume": manual_cli_result.get("automatic_resume") is True,
            "current_at_crash": manual_cli_result.get("current_at_crash") is True,
            "managed_launch_linked": manual_cli_result.get("managed_launch_linked") is True,
        }
        foreign_after_refusal = (wait_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
            transition_fixtures.foreign_project_sha256, foreign_session_hash,
            route_instance, before_refusal_seq,
            min(context["deadline"], time.monotonic() + 4.0),
        ) if manual_refusal_error is not None and before_refusal_seq is not None else None)
        result["foreign_route_after_refusal"] = {
            "acknowledged": foreign_after_refusal is not None,
            "same_tui_instance": bool(
                foreign_after_refusal is not None
                and foreign_after_refusal.get("tui_instance") == route_instance
            ),
            "sequence_advanced_after_refusal": bool(
                foreign_after_refusal is not None and before_refusal_seq is not None
                and foreign_after_refusal.get("seq", 0) > before_refusal_seq
            ),
            "sequence": (foreign_after_refusal.get("seq")
                         if foreign_after_refusal is not None else None),
        }
        preserved_after_refusal = read_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
        )
        checks = {
            "selected_foreign": selected_foreign,
            "exact_foreign_route": foreign_route is not None,
            "same_tui_instance": result["foreign_route_evidence"]["same_tui_instance"],
            "sequence_advanced": result["foreign_route_evidence"]["sequence_advanced"],
            "stale_or_unavailable": bool(
                refusal_snapshot and refusal_snapshot.get("status") in {"stale", "unavailable"}
                and refusal_snapshot.get("last_confirmed_revision") == foreign_revision
            ),
            "a_binding_preserved_before_cli": bool(
                preserved_foreign
                and preserved_foreign["revision"] == foreign_revision
                and preserved_foreign["provider_session_hash"]
                == initial_a_binding["provider_session_hash"]
            ),
            "picker_closed": result["foreign_session_surface"]["session_picker_closed"],
            "pane_owner_client": (
                result["foreign_session_surface"]["input_owner"] == "client"
            ),
            "pane_live": result["foreign_session_surface"]["pane_live"],
            "pane_applied": result["foreign_session_surface"]["pane_state"] == "applied",
            "manual_refusal_valid": manual_refusal_error is not None,
            "manual_refusal_no_ticket": manual_refusal_no_ticket,
            "foreign_route_persisted_after_refusal": bool(
                result["foreign_route_after_refusal"]["same_tui_instance"]
                and result["foreign_route_after_refusal"]["sequence_advanced_after_refusal"]
            ),
            "a_binding_preserved_after_cli": bool(
                preserved_after_refusal
                and preserved_foreign
                and preserved_after_refusal["revision"] == preserved_foreign["revision"]
                and preserved_after_refusal["provider_session_hash"]
                == initial_a_binding["provider_session_hash"]
            ),
        }
        result["foreign_refusal_checks"] = checks
        result["managed_foreign_project_refused"] = foreign_refusal_acceptance(checks)
        if not result["managed_foreign_project_refused"]:
            result["reason"] = "foreign_project_scope_refusal_not_proven"
            return result

        pre_foreign_return = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target,
        )
        foreign_return_trace: dict[str, Any] = {}
        foreign_return_child = lambda: managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context,
            handles[0].expected_target, labels=labels,
            fixture_titles=fixture_titles,
            target_label=labels[0],
            other_label=transition_fixtures.foreign_root_title,
            filter_text=session_a,
            filter_mode="fixture_session_id",
            fixture_session_id=session_a,
        )
        returned_from_foreign = select_session(
            attach, labels[0], transition_fixtures.foreign_root_title, labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            foreign_return_trace, pre_read=pre_foreign_return,
            fresh_child_read=foreign_return_child, open_chord=b"\x0f",
            fixture_titles=fixture_titles, filter_text=session_a,
            filter_mode="fixture_session_id", fixture_session_id=session_a,
        )
        result["foreign_return_trace"] = foreign_return_trace
        foreign_return_route = (wait_fixture_route(
            route_paths[0], route_nonces[0], route_projects,
            transition_fixtures.project_sha256,
            hashlib.sha256(session_a.encode("ascii")).hexdigest(),
            route_instance, foreign_after_refusal.get("seq", route_before_foreign_seq),
            min(context["deadline"], time.monotonic() + 4.0),
        ) if returned_from_foreign and foreign_after_refusal is not None else None)
        result["foreign_return_route_evidence"] = {
            "acknowledged": foreign_return_route is not None,
            "same_tui_instance": bool(
                foreign_return_route is not None
                and foreign_return_route.get("tui_instance") == route_instance
            ),
            "sequence_advanced": bool(
                foreign_return_route is not None and foreign_route is not None
                and foreign_return_route.get("seq", 0) > foreign_route.get("seq", 0)
            ),
            "project_sha256": (transition_fixtures.project_sha256
                               if foreign_return_route is not None else None),
            "session_sha256": (hashlib.sha256(session_a.encode("ascii")).hexdigest()
                               if foreign_return_route is not None else None),
            "sequence": (foreign_return_route.get("seq")
                         if foreign_return_route is not None else None),
        }
        foreign_return_snapshot = (confirmation_after(
            handles[0], foreign_revision,
            min(context["deadline"], time.monotonic() + 5.0),
        ) if returned_from_foreign else None)
        foreign_return_live = (wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_a,
            foreign_return_snapshot["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        ) if foreign_return_snapshot is not None else None)
        if returned_from_foreign and foreign_return_route is None:
            latest_route, latest_route_diagnostic = fixture_route_diagnostic(
                route_paths[0], route_nonces[0], route_projects,
            )
            latest_provider_snapshot = handles[0].latest
            result["foreign_return_latest_observer_diagnostic"] = {
                key: latest_route_diagnostic.get(key)
                for key in (
                    "status", "latest_route", "latest_project_sha256",
                    "latest_session_sha256", "latest_instance_sha256",
                    "latest_sequence", "accepted_count", "rejected_count",
                )
            }
            result["foreign_return_latest_observer_diagnostic"].update({
                "matches_a": fixture_route_matches(
                    latest_route, transition_fixtures.project_sha256,
                    hashlib.sha256(session_a.encode("ascii")).hexdigest(),
                    instance=route_instance,
                    after_seq=(foreign_after_refusal.get("seq", 0)
                               if foreign_after_refusal else 0),
                ),
                "matches_b": fixture_route_matches(
                    latest_route, transition_fixtures.project_sha256,
                    hashlib.sha256(session_b.encode("ascii")).hexdigest(),
                    instance=route_instance,
                    after_seq=(foreign_after_refusal.get("seq", 0)
                               if foreign_after_refusal else 0),
                ),
                "matches_foreign": fixture_route_matches(
                    latest_route, transition_fixtures.foreign_project_sha256,
                    foreign_session_hash, instance=route_instance,
                    after_seq=(foreign_after_refusal.get("seq", 0)
                               if foreign_after_refusal else 0),
                ),
            })
            result["foreign_return_provider_runtime_status"] = {
                "snapshot_status": (latest_provider_snapshot.get("status")
                                    if isinstance(latest_provider_snapshot, dict) else None),
                "snapshot_revision": (latest_provider_snapshot.get("last_confirmed_revision")
                                      if isinstance(latest_provider_snapshot, dict) else None),
                "fresh_last_confirmed_after_foreign": bool(
                    foreign_return_snapshot
                    and foreign_return_snapshot.get("status") == "last_confirmed"
                    and foreign_return_snapshot.get("last_confirmed_revision", 0)
                    > foreign_revision
                ),
                "disk_binding_present": foreign_return_live is not None,
                "disk_binding_revision": (foreign_return_live.get("revision")
                                          if foreign_return_live else None),
                "disk_binding_matches_a": bool(
                    foreign_return_live
                    and foreign_return_live.get("provider_session_hash")
                    == initial_a_binding["provider_session_hash"]
                ),
                "disk_revision_matches_runtime": bool(
                    foreign_return_live and foreign_return_snapshot
                    and foreign_return_live.get("revision")
                    == foreign_return_snapshot.get("last_confirmed_revision")
                ),
            }
        result["managed_foreign_project_returned_to_a"] = bool(
            foreign_return_route is not None
            and result["foreign_return_route_evidence"]["same_tui_instance"]
            and result["foreign_return_route_evidence"]["sequence_advanced"]
            and foreign_return_live and foreign_return_snapshot
            and foreign_return_live["provider_session_hash"]
            == initial_a_binding["provider_session_hash"]
            and foreign_return_live["revision"]
            == foreign_return_snapshot["last_confirmed_revision"]
            and foreign_return_snapshot["last_confirmed_revision"] > foreign_revision
        )
        if not result["managed_foreign_project_returned_to_a"]:
            result["reason"] = "foreign_project_return_to_a_not_proven"
            return result
        result["live_binding_checkpoints"].append({
            "route": "A_after_foreign_project",
            "revision": foreign_return_live["revision"],
            "provider_session_hash": foreign_return_live["provider_session_hash"],
        })

        if attach is not None:
            attach.close()
            attach = None
        for index, handle in enumerate(handles):
            clean = handle.close(min(context["deadline"], time.monotonic() + 6.0))
            if not clean:
                result["reason"] = "managed_server_cleanup_unverified"
                return result
        historical = [
            historical_binding(binary, directory, project, handle.expected_target,
                               result["historical_lookup"])
            for directory, handle in zip(server_dirs, handles)
        ]
        expected_hashes = [hashlib.sha256(session_a.encode()).hexdigest(),
                           hashlib.sha256(session_b.encode()).hexdigest()]
        result["historical_identity_matches"] = bool(
            all(item is not None for item in historical)
            and historical[0]["provider_session_hash"] == expected_hashes[0]
            and historical[1]["provider_session_hash"] == expected_hashes[1]
            and historical[0]["provider_version"] == EXPECTED_VERSION
            and historical[1]["provider_version"] == EXPECTED_VERSION
        )
        result["historical_revision_advanced"] = bool(
            historical[0] and historical[0]["binding_revision"] >= 2
        )
        result["historical_after_stop"] = (
            result["historical_identity_matches"] and result["historical_revision_advanced"]
        )
        result["current_at_crash"] = historical[0]["current_at_crash"] if historical[0] else None
        result["automatic_resume"] = historical[0]["automatic_resume"] if historical[0] else None
        result["final_revisions"] = [
            historical[0]["binding_revision"] if historical[0] else None,
            historical[1]["binding_revision"] if historical[1] else None,
        ]
        if not result["historical_after_stop"]:
            result["reason"] = "historical_exact_binding_not_verified"
            return result
        result.update(classification="proven", reason="managed_bridge_and_historical_binding_verified")
        return result
    except OSError:
        result.update(classification="blocked", reason="managed_io_failure")
        return result
    except subprocess.SubprocessError:
        result.update(classification="blocked", reason="managed_subprocess_failure")
        return result
    except TimeoutError:
        result.update(classification="blocked", reason="managed_timeout")
        return result
    finally:
        if attach is not None:
            result["tui_markers"] = screen_markers(attach)
            attach.close()
        cleanup = True
        for handle in reversed(handles):
            if handle.process.poll() is None:
                cleanup = handle.close(min(context["deadline"], time.monotonic() + 6.0)) and cleanup
        result["cleanup_completed"] = cleanup
        # Preserve independent durability evidence even when the UI picker
        # gate blocks the full switch scenario. Lookup occurs only after each
        # owned server has stopped cleanly and keeps identifiers hashed.
        if (cleanup and result["initial_last_confirmed"]
                and not result["historical_after_stop"] and len(handles) == 2):
            historical = [
                historical_binding(binary, directory, project, handle.expected_target,
                                   result["historical_lookup"])
                for directory, handle in zip(server_dirs, handles)
            ]
            expected_hashes = [hashlib.sha256(session_a.encode()).hexdigest(),
                               hashlib.sha256(session_b.encode()).hexdigest()]
            result["historical_identity_matches"] = bool(
                all(item is not None for item in historical)
                and historical[0]["provider_session_hash"] == expected_hashes[0]
                and historical[1]["provider_session_hash"] == expected_hashes[1]
                and historical[0]["provider_version"] == EXPECTED_VERSION
                and historical[1]["provider_version"] == EXPECTED_VERSION
            )
            result["historical_revision_advanced"] = bool(
                historical[0] and historical[0]["binding_revision"] >= 2
            )
            result["historical_after_stop"] = (
                result["historical_identity_matches"] and result["historical_revision_advanced"]
            )
            result["final_revisions"] = [
                item["binding_revision"] if item else None for item in historical
            ]
            if result["historical_after_stop"]:
                result["current_at_crash"] = historical[0]["current_at_crash"]
                result["automatic_resume"] = historical[0]["automatic_resume"]
        if result["initial_last_confirmed"]:
            checked_ids = [session_a, session_b]
            if transition_fixtures is not None:
                checked_ids.extend((
                    transition_fixtures.new_root_id,
                    transition_fixtures.fork_root_id,
                    transition_fixtures.fork_parent_id,
                    transition_fixtures.foreign_root_id,
                ))
            # The shared verifier intentionally caps a batch at three IDs.
            # Keep those fixed bounds while covering every fixture session.
            no_user_messages = True
            for offset in range(0, len(checked_ids), 3):
                group_ok, _statuses = verify_no_user_messages(
                    context["base"], context["password"],
                    tuple(checked_ids[offset:offset + 3]),
                )
                no_user_messages = no_user_messages and group_ok
            result["user_message_absence_after"] = no_user_messages
            result["user_message_absence_verified"] = (
                result.get("user_message_absence_before") is True and no_user_messages
            )
            if not no_user_messages and result.get("classification") == "proven":
                result.update(classification="blocked",
                              reason="fixture_user_message_absence_unverified")
        result["bridge_snapshots"] = [
            [{"status": item["status"], "revision": item["last_confirmed_revision"]}
             for item in handle.snapshots[-8:]]
            for handle in handles
        ]
        result["serve_lines_valid"] = all(not handle.invalid_line for handle in handles)
        if not cleanup and result.get("classification") == "proven":
            result.update(classification="blocked", reason="managed_cleanup_unverified")


def sha256_file(path: Path) -> str | None:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as stream:
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
    except OSError:
        return None
    return digest.hexdigest()


def run(panelink_binary: str, executable: str | None = None) -> tuple[int, dict[str, Any]]:
    opencode, launcher = _resolve_opencode(executable)
    panelink = Path(panelink_binary).resolve()
    summary: dict[str, Any] = {
        "classification": "blocked",
        "reason": "not_started",
        "opencode_version": None,
        "pinned_binary_match": False,
        "sandbox": "not_started",
        "network_scope": "loopback_only_sandbox",
        "managed_serve_count": 0,
        "root_a_to_b_to_a": False,
        "passive_b_unchanged": False,
        "home_stale_and_returned": False,
        "historical_after_stop": False,
        "current_at_crash": None,
        "automatic_resume": None,
        "prompt_submissions": 0,
        "cleanup_completed": False,
    }
    if opencode is None or launcher != "opencode":
        return 2, {**summary, "reason": "native_opencode_missing"}
    if not panelink.is_file() or not os.access(panelink, os.X_OK):
        return 2, {**summary, "reason": "panelink_binary_unavailable"}
    if sha256_file(Path(opencode)) != EXPECTED_SHA256:
        return 2, {**summary, "reason": "pinned_opencode_hash_mismatch"}
    if sys.platform != "darwin":
        return 2, {**summary, "reason": "macos_sandbox_required"}
    sandbox = shutil.which("sandbox-exec")
    if sandbox is None:
        return 2, {**summary, "reason": "sandbox_exec_unavailable"}
    summary["pinned_binary_match"] = True
    cwd = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="panelink-p4-bridge-sandbox-") as temp:
        root = Path(temp)
        root.chmod(0o700)
        profile = root / "loopback.sb"
        profile.write_text(MANAGED_SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        worker_env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C",
                      "TMPDIR": "/private/tmp", WORKER_ENV: "1"}
        argv = [sandbox, "-f", str(profile), sys.executable, str(Path(__file__).resolve()),
                "--sandbox-worker", str(panelink), opencode]
        code, raw, sandbox_error = bounded_worker(argv, cwd, worker_env, timeout=72.0)
    try:
        value = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        value = {"classification": "blocked", "reason": sandbox_error or "worker_output_invalid"}
    if isinstance(value, dict):
        summary.update(value)
    else:
        summary["reason"] = "worker_output_invalid"
    summary["sandbox"] = "not_enforced" if sandbox_error else "enforced_loopback_only"
    if sandbox_error:
        summary["classification"] = "blocked"
        summary["reason"] = sandbox_error
    return (0 if code == 0 and summary.get("classification") == "proven"
            and summary.get("cleanup_completed") is True else 2), summary


def managed_callback(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    try:
        return managed_acceptance(context, binary)
    except (OSError, subprocess.SubprocessError, TimeoutError) as error:
        # Keep diagnostics to a fixed exception class; never include exception
        # text, paths, environment values, or PTY contents.
        return {
            "classification": "blocked",
            "reason": "managed_callback_" + type(error).__name__,
            "cleanup_completed": False,
        }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--panelink-binary", default="/private/tmp/panelink-p4-bridge-store-target/debug/panelink")
    parser.add_argument("--opencode", help="optional explicit native OpenCode binary")
    parser.add_argument("--sandbox-worker", nargs=2, metavar=("PANELINK", "OPENCODE"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.sandbox_worker:
        if os.environ.get(WORKER_ENV) != "1":
            print(json.dumps({"classification": "blocked", "reason": "sandbox_worker_marker_missing"}))
            return 2
        binary, opencode = map(Path, args.sandbox_worker)
        result = _run_worker(str(opencode), "opencode",
                             after_bootstrap=lambda context: managed_callback(context, binary),
                             total_deadline_seconds=MAX_WORKER_SECONDS,
                             scenario="panelink_managed_bridge")
        worker_result = result
        tui = worker_result.get("tui", {})
        result = {
            "classification": ("proven" if worker_result.get("classification") == "selected_tui_identity_observed"
                               and tui.get("classification") == "proven"
                               and tui.get("user_message_absence_verified") is True
                               and tui.get("user_message_absence_after") is True
                               else tui.get("classification", worker_result.get("classification", "blocked"))),
            "reason": tui.get("reason", worker_result.get("reason", "worker_failed")),
            "opencode_version": worker_result.get("version"),
            "pinned_binary_match": worker_result.get("binary_sha256_matches") is True,
            "sandbox": "worker", "network_scope": worker_result.get("network_scope"),
            "prompt_submissions": worker_result.get("prompt_submissions", 0),
            "user_message_absence_verified": (
                tui.get("user_message_absence_verified") is True
                and tui.get("user_message_absence_after") is True
            ),
            "cleanup_completed": worker_result.get("cleanup_completed") is True
                and tui.get("cleanup_completed") is True,
            **{key: tui.get(key) for key in (
                "managed_serve_count", "initial_last_confirmed", "root_a_to_b_to_a",
                "keyboard_route_proven", "managed_new_root_selected",
                "managed_imported_fork_selected", "managed_fork_returned_to_a",
                "managed_foreign_project_refused", "managed_foreign_project_returned_to_a",
                "foreign_route_evidence", "foreign_route_after_refusal",
                "foreign_manual_refusal", "foreign_refusal_checks",
                "foreign_return_route_evidence",
                "foreign_return_latest_observer_diagnostic",
                "foreign_return_provider_runtime_status",
                "route_observer_initial", "route_observer_home_return",
                "route_observer_failure",
                "transition_fixtures",
                "initial_a_binding_verified",
                "passive_b_unchanged", "home_stale_and_returned", "historical_after_stop",
                "home_stale_observed", "home_preserved_last_confirmed_a",
                "historical_identity_matches", "historical_revision_advanced",
                "current_at_crash", "automatic_resume", "targets", "final_revisions",
                "serve_lines_valid", "historical_lookup",
                "binding_revision_advanced_for_b", "binding_revision_advanced_for_a",
                "interim_b_binding_verified", "return_a_binding_verified",
                "post_enter_binding_matches_a", "post_enter_binding_matches_b",
                "post_enter_binding_revision", "user_message_absence_before",
                "user_message_absence_after", "user_message_absence_verified",
                "managed_serve_startup_diagnostics",
            )},
            "transition_binding_checkpoints": compact_binding_checkpoints(
                tui.get("transition_binding_checkpoints")),
            "live_binding_checkpoints": compact_binding_checkpoints(
                tui.get("live_binding_checkpoints")),
            "bridge_snapshots": compact_bridge_snapshots(tui.get("bridge_snapshots")),
            **{key: compact_picker_trace(tui.get(key)) for key in (
                "switch_trace", "return_trace", "new_root_trace", "imported_fork_trace",
                "fork_return_trace", "home_return_trace", "foreign_project_trace",
                "foreign_return_trace", "home_trace",
            ) if isinstance(tui.get(key), dict)},
            "foreign_session_surface": {
                key: tui.get("foreign_session_surface", {}).get(key)
                for key in ("screen_status", "foreign_title_visible", "session_picker_closed",
                            "input_owner", "owner_revision", "pane_live", "pane_state")
                if key in tui.get("foreign_session_surface", {})
            },
            "tui_markers": {
                key: tui.get("tui_markers", {}).get(key)
                for key in ("alive", "open_code", "provider_prompt", "session_a_visible",
                            "session_b_visible", "sessions_heading", "auth_prompt",
                            "generic_error", "input_delivery_failed", "host_error_category")
                if key in tui.get("tui_markers", {})
            },
        }
        print(json.dumps(compact_fixed_evidence(result), sort_keys=True,
                         separators=(",", ":")))
        return 0 if result["classification"] == "proven" and result["cleanup_completed"] else 2
    code, result = run(args.panelink_binary, args.opencode)
    print(json.dumps(compact_fixed_evidence(result), sort_keys=True,
                     separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
