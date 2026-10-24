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
import selectors
import shutil
import signal
import stat
import struct
import subprocess
import sys
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
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            close_fds=True, shell=False, start_new_session=True, bufsize=0,
        )
        assert self.process.stdout is not None
        os.set_blocking(self.process.stdout.fileno(), False)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = bytearray()
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

    def close(self, deadline: float) -> bool:
        self.selector.close()
        process = self.process
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
                return False
            return False
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

    def drain(self, timeout: float = 0.0) -> bytes:
        """Always drain the PTY; retain only a bounded rolling transcript."""
        chunks = bytearray()
        for _key, _mask in self.selector.select(max(0.0, timeout)):
            while True:
                try:
                    chunk = os.read(self.master, 8192)
                except (BlockingIOError, OSError):
                    break
                if not chunk:
                    break
                self.total_output_bytes += len(chunk)
                append_rolling(chunks, chunk, MAX_ATTACH_TRANSCRIPT_BYTES)
        append_rolling(self.output, chunks, MAX_ATTACH_TRANSCRIPT_BYTES)
        return bytes(chunks)

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
    text = pty_child.recent_text(bytes(pty_child.output)).lower()
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
        "bytes": getattr(pty_child, "total_output_bytes", len(pty_child.output)),
        "retained_bytes": len(pty_child.output),
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
        output_sequence = getattr(pty_child, "total_output_bytes", len(pty_child.output))
        screen = pty_child.screen_text()
        lowered = screen.lower()
        heading = session_picker_heading(lowered)
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
            trace["menu_screen_shape"] = safe_screen_shape(screen, labels[0], labels[1])
        # A modal must be identifiable by its exact title, or by the generic
        # Sessions heading together with both exact rows inside that modal.
        if output_sequence > after_output_sequence and session_modal_ready(screen, labels):
            return True
    return False


def session_picker_heading(lowered_screen: str) -> bool:
    """Recognize exact known modal headings, not incidental screen text."""
    normalized = lowered_screen.lower()
    return ("open session or project" in normalized
            or "recent sessions" in normalized or "Sessions" in lowered_screen)


def session_picker_screen(screen: str, labels: tuple[str, str]) -> bool:
    picker = session_picker_region(screen)
    heading = ("open session or project" in picker.lower()
               or "recent sessions" in picker.lower() or "Sessions" in picker)
    return heading and all(label in picker for label in labels)


def filtered_session_row(screen: str, target_label: str, other_label: str) -> bool:
    picker = session_picker_region(screen)
    return (session_picker_heading(picker) and target_label in picker
            and other_label not in picker)


def session_picker_region(screen: str) -> str:
    """Exclude the current-session header above the native picker."""
    index = screen.find("Sessions")
    if index >= 0:
        return screen[index:]
    lowered = screen.lower()
    for marker in ("open session or project", "recent sessions"):
        index = lowered.find(marker)
        if index >= 0:
            return screen[index:]
    return ""


def session_modal_ready(screen: str, labels: tuple[str, str]) -> bool:
    picker = session_picker_region(screen)
    lowered = screen.lower()
    exact_title = "open session or project" in lowered or "recent sessions" in lowered
    generic_with_rows = "Sessions" in picker and all(label in picker for label in labels)
    return exact_title or generic_with_rows


def open_project_picker_region(screen: str) -> str:
    """Keep the native modal, including project rows above its Sessions list."""
    lowered = screen.lower()
    for marker in ("open session or project", "recent sessions", "sessions"):
        index = lowered.find(marker)
        if index >= 0:
            return screen[index:]
    return ""


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


def safe_screen_shape(screen: str, label_a: str | None = None,
                      label_b: str | None = None) -> list[dict[str, Any]]:
    """Describe only allowlisted UI words and fixture labels by row."""
    shape: list[dict[str, Any]] = []
    for row, line in enumerate(screen.splitlines()[:PTY_SIZE[0]]):
        lowered = line.lower()
        markers: list[str] = []
        if label_a and label_a in line:
            markers.append("fixture_a")
        if label_b and label_b in line:
            markers.append("fixture_b")
        for marker, name in (
            ("open session or project", "open_session_or_project"),
            ("recent sessions", "recent_sessions"),
            ("sessions", "sessions"),
            ("new session", "new_session"),
            ("command palette", "command_palette"),
            ("commands", "commands"),
            ("project", "project_word"),
            ("prompt", "prompt_word"),
            ("model", "model_word"),
            ("provider", "provider_word"),
            ("loading", "loading"),
        ):
            if marker in lowered:
                markers.append(name)
        if any(char in line for char in "╭╮╰╯│─┌┐└┘├┤┬┴┼"):
            markers.append("box_border")
        if any(char in line for char in ("?", "›", "⌕", "🔍")):
            markers.append("search_or_prompt_marker")
        if line.strip() and not markers:
            markers.append("other_nonempty")
        if markers:
            shape.append({"row": row, "markers": sorted(set(markers))})
    return shape


def select_session(pty_child: PrivatePty, target_label: str, other_label: str,
                   labels: tuple[str, str], deadline: float,
                   trace: dict[str, Any] | None = None,
                   *, pre_read: dict[str, Any] | None = None,
                   fresh_child_read: Callable[[], dict[str, Any]] | None = None) -> bool:
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
    before_sequence = getattr(pty_child, "total_output_bytes", len(pty_child.output))
    before_screen = pty_child.screen_text()
    if not before_screen:
        if trace is not None:
            trace["fresh_pre_action_screen_nonempty"] = False
        return False
    if trace is not None:
        trace["fresh_pre_action_screen_nonempty"] = True
        trace["pre_input_output_sequence"] = before_sequence
    try:
        pty_child.send(b"\x0f")  # native OpenCode Ctrl-O project/session modal
    except OSError:
        if trace is not None:
            trace["ctrl_o_write_ok"] = False
        return False
    if trace is not None:
        trace["ctrl_o_sent"] = True
        trace["ctrl_o_write_ok"] = True
        trace["alive_after_ctrl_o"] = pty_child.alive()
    output_after_ctrl_o = getattr(pty_child, "total_output_bytes", len(pty_child.output))
    if trace is not None:
        trace["output_sequence_immediately_after_ctrl_o"] = output_after_ctrl_o
    menu_seen = visible_session_menu(
        pty_child, labels, min(deadline, time.monotonic() + 4.0), trace,
        after_output_sequence=before_sequence,
    )
    if trace is not None:
        trace["alive_after_menu_wait"] = pty_child.alive()
        trace["output_advanced_after_ctrl_o"] = (
            getattr(pty_child, "total_output_bytes", len(pty_child.output)) > before_sequence
        )
    if not menu_seen:
        unavailable_menu_child = (fresh_child_read()
                                  if fresh_child_read is not None else None)
        if trace is not None and isinstance(unavailable_menu_child, dict):
            trace["fresh_menu_child_read_valid"] = unavailable_menu_child.get("status") == "known"
            trace["menu_child_picker_heading"] = (
                unavailable_menu_child.get("contains_picker_heading") is True
            )
            trace["menu_child_contains_both_rows"] = (
                unavailable_menu_child.get("contains_both_fixture_labels") is True
            )
            trace["menu_child_screen_positions"] = unavailable_menu_child.get("screen_positions")
            trace["menu_child_screen_shape"] = unavailable_menu_child.get("screen_shape")
        # Ctrl-P is a separate, non-committing diagnostic key. It can expose
        # whether the managed native TUI is receiving shortcuts even when
        # Ctrl-O does not open the expected picker. Never type a command or
        # press Enter in this diagnostic path.
        try:
            pty_child.send(b"\x10")
            if trace is not None:
                trace["ctrl_p_sent"] = True
        except OSError:
            if trace is not None:
                trace["ctrl_p_sent"] = False
        ctrl_p_sequence = getattr(pty_child, "total_output_bytes", len(pty_child.output))
        ctrl_p_deadline = min(deadline, time.monotonic() + 1.5)
        while (time.monotonic() < ctrl_p_deadline and pty_child.alive()
               and getattr(pty_child, "total_output_bytes", len(pty_child.output)) <= ctrl_p_sequence):
            pty_child.drain(0.05)
        ctrl_p_screen = pty_child.screen_text()
        if trace is not None:
            trace["ctrl_p_output_advanced"] = (
                getattr(pty_child, "total_output_bytes", len(pty_child.output)) > ctrl_p_sequence
            )
            trace["ctrl_p_attach_screen_shape"] = safe_screen_shape(
                ctrl_p_screen, labels[0], labels[1]
            )
        ctrl_p_child = fresh_child_read() if fresh_child_read is not None else None
        if trace is not None and isinstance(ctrl_p_child, dict):
            trace["ctrl_p_child_read_valid"] = ctrl_p_child.get("status") == "known"
            trace["ctrl_p_child_screen_shape"] = ctrl_p_child.get("screen_shape")
        return False
    # Compare the attach transcript with a fresh server-side pane read before
    # typing; retained transcript bytes can leave stale rows in a VT replay.
    menu_child = fresh_child_read() if fresh_child_read is not None else None
    menu_child_ok = (
        isinstance(menu_child, dict) and menu_child.get("status") == "known"
        and menu_child.get("contains_picker_heading") is True
    )
    if trace is not None:
        trace["fresh_menu_child_read_valid"] = menu_child_ok
        trace["menu_attach_child_screen_agree"] = menu_child_ok
        trace["menu_child_screen_positions"] = (
            menu_child.get("screen_positions") if isinstance(menu_child, dict) else None
        )
        trace["menu_child_screen_shape"] = (
            menu_child.get("screen_shape") if isinstance(menu_child, dict) else None
        )
    if fresh_child_read is not None and not menu_child_ok:
        return False
    try:
        # Match the already-proven native fixture's single bounded title
        # write. The exact ASCII title is the only text sent and remains in
        # the picker; it is never sent to the prompt.
        encoded_title = target_label.encode("ascii")
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
    filter_sequence = getattr(pty_child, "total_output_bytes", len(pty_child.output))
    filtered_deadline = min(deadline, time.monotonic() + 2.0)
    while time.monotonic() < filtered_deadline and pty_child.alive():
        pty_child.drain(0.05)
        output_sequence = getattr(pty_child, "total_output_bytes", len(pty_child.output))
        screen = pty_child.screen_text()
        heading_visible = session_picker_heading(screen.lower())
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
        if (filtered_session_row(screen, target_label, other_label)
                and output_sequence > filter_sequence):
            if trace is not None:
                trace["target_only_filtered_row_seen"] = True
                trace["filtered_output_sequence"] = output_sequence
            break
    else:
        # Even when VT reconstruction never reaches a safe filtered row,
        # sample the managed child's current screen once. This distinguishes
        # a stale attach transcript from a picker that did not filter.
        failed_filter_child = fresh_child_read() if fresh_child_read is not None else None
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
        return False
    pty_child.drain(0.02)  # inspect the filtered row again immediately before activation
    final_screen = pty_child.screen_text()
    if not filtered_session_row(final_screen, target_label, other_label):
        return False
    final_child = None
    final_child_ok = False
    if fresh_child_read is not None:
        child_read_deadline = min(deadline, time.monotonic() + 1.5)
        while time.monotonic() < child_read_deadline:
            final_child = fresh_child_read()
            final_child_ok = (
                isinstance(final_child, dict) and final_child.get("status") == "known"
                and final_child.get("contains_picker_heading") is True
                and final_child.get("contains_target_label") is True
                and final_child.get("contains_competitor_label") is False
            )
            if final_child_ok:
                break
            time.sleep(0.05)
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
                              expected_session_id: str) -> dict[str, Any] | None:
    """Read and validate the fixture's exact atomic combined slot while live.

    This is a read-only acceptance-fixture decoder, not a production storage
    API and not a lock bypass for mutation. It opens only the deterministic
    exact-target slot with no-follow semantics and verifies owner, mode, inode,
    bounds, full target/project/version, and the expected fixture ID. It returns
    a hash and revision only; the ID and bytes remain transient.
    """
    parsed_target = parse_target(target)
    if parsed_target is None or not expected_session_id.isascii():
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
    if (actual_session_id != expected_session_id or provider_version != EXPECTED_VERSION):
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
                                target_label: str | None = None,
                                other_label: str | None = None) -> dict[str, Any]:
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
        return {
            "status": "known",
            "screen_bytes": len(value["text"].encode("utf-8")),
            "contains_opencode_title": "oc |" in screen or "opencode" in screen,
            "contains_open_session_title": "open session or project" in screen,
            "contains_recent_sessions_title": "recent sessions" in screen,
            "contains_sessions_title": "sessions" in screen,
            "contains_session_a": "panelink v2 identity a" in screen,
            "contains_session_b": "panelink v2 identity b" in screen,
            "contains_picker_heading": session_picker_heading(screen),
            "contains_both_fixture_labels": (all(label in picker_region for label in labels)
                                              if labels is not None else False),
            "contains_target_label": (target_label in picker_region
                                       if target_label is not None else False),
            "contains_competitor_label": (other_label in picker_region
                                          if other_label is not None else False),
            "screen_positions": fixed_screen_positions(original_screen, {
                "heading": "Sessions",
                **({"target": target_label} if target_label is not None else {}),
                **({"competitor": other_label} if other_label is not None else {}),
            }),
            "screen_shape": safe_screen_shape(
                original_screen,
                labels[0] if labels is not None else None,
                labels[1] if labels is not None else None,
            ),
            "contains_error_marker": "error" in screen or "failed" in screen,
            "contains_provider_prompt": "connect a provider" in screen,
        }
    return {"status": "context_or_screen_unavailable"}


def explicit_fixture_navigation(binary: Path, directory: Path, target: str,
                                project: Path, session_id: str) -> dict[str, Any]:
    """Issue the product's explicit prepare/resume route action for this fixture."""
    base = [str(binary), "provider-binding"]
    scope = ["--directory", str(directory), "--target", target,
             "--project", str(project)]
    env = {"PATH": os.defpath, "HOME": str(directory.parent), "LANG": "C", "LC_ALL": "C"}
    try:
        prepared = subprocess.run(
            base + ["prepare"] + scope + ["--session", session_id],
            cwd=project, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, timeout=10, check=False, close_fds=True, shell=False,
        )
        if prepared.returncode != 0 or len(prepared.stdout) > 16 * 1024:
            return {"selected": False}
        value = json.loads(prepared.stdout)
        ticket = value.get("ticket") if isinstance(value, dict) else None
        revision = value.get("revision") if isinstance(value, dict) else None
        if (not isinstance(value, dict) or value.get("ok") is not True
                or value.get("state") != "prepared_choice_saved"
                or value.get("selected_confirmed") is not False
                or not isinstance(ticket, str) or len(ticket) != 32
                or any(char not in "0123456789abcdef" for char in ticket)
                or isinstance(revision, bool) or not isinstance(revision, int) or revision <= 0):
            return {"selected": False}
        resumed = subprocess.run(
            base + ["resume"] + scope + ["--ticket", ticket, "--revision", str(revision)],
            cwd=project, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, timeout=10, check=False, close_fds=True, shell=False,
        )
        if resumed.returncode != 0 or len(resumed.stdout) > 16 * 1024:
            return {"selected": False}
        result = json.loads(resumed.stdout)
        selected = (isinstance(result, dict) and result.get("ok") is True
                    and result.get("state") == "selected_and_saved"
                    and result.get("selected_confirmed") is True
                    and result.get("revision") == revision
                    and result.get("automatic_resume") is False)
        return {"selected": selected, "revision": revision if selected else None}
    except (OSError, subprocess.SubprocessError, ValueError, UnicodeDecodeError):
        return {"selected": False}


def managed_acceptance(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    """Start two real managed TUI children and verify snapshots and durability."""
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
        "explicit_navigation_fallback_used": False,
        "passive_b_unchanged": False,
        "home_stale_and_returned": False,
        "historical_after_stop": False,
        "historical_identity_matches": False,
        "historical_revision_advanced": False,
        "current_at_crash": False,
        "automatic_resume": False,
        "prompt_submissions": 0,
        "attempted_control_count": 0,
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
        "return_a_binding_verified": False,
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
    child_env.update({"TERM": "xterm-256color",
                      "OPENCODE_SERVER_PASSWORD": context["password"]})
    child_env.pop("TERMINFO", None)
    server_dirs = [root / "serve-a", root / "serve-b"]
    for path in server_dirs:
        path.mkdir(mode=0o700)
    handles: list[ServeProcess] = []
    attach: PrivatePty | None = None
    started: list[tuple[int, int, int, int, int]] = []
    try:
        for directory, identifier in zip(server_dirs, (session_a, session_b)):
            argv = [str(binary), "serve", "--directory", str(directory), "--terminfo",
                    str(terminfopath), "--initialize", "--allow-input", "--opencode-bridge",
                    "--", context["executable"], "--server", context["base"],
                    "--session", identifier, str(project)]
            # Targets use the first generation/workspace/pane/session/occupant tuple.
            handle = ServeProcess(argv, project, child_env, expected_target="")
            handles.append(handle)
        result["managed_serve_count"] = len(handles)
        serve_deadline = min(context["deadline"], time.monotonic() + CHILD_DEADLINE)
        while time.monotonic() < serve_deadline and any(handle.startup is None for handle in handles):
            for handle in handles:
                if handle.startup is None and handle.process.poll() is None:
                    handle.poll(0.0)
            time.sleep(0.05)
        for handle in handles:
            if handle.startup is not None:
                started.append(parse_target(handle.startup["target"]))  # type: ignore[arg-type]
            if handle.startup is None or handle.invalid_line:
                failure = handle.failure
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
        if not wait_for(lambda: attach.alive() and attach.raw_mode() and bool(attach.output), attach_deadline,
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
            labels=labels, target_label=labels[1], other_label=labels[0],
        )
        switch_to_b = select_session(
            attach, labels[1], labels[0], labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            switch_trace, pre_read=pre_switch_screen, fresh_child_read=switch_child_read,
        )
        result["switch_trace"] = switch_trace
        observed_b = (confirmation_after(handles[0], first["last_confirmed_revision"],
                                         min(context["deadline"], time.monotonic() + 5.0))
                      if switch_to_b else None)
        live_b = (wait_live_managed_binding(
            server_dirs[0], handles[0].expected_target, project, session_b,
            observed_b["last_confirmed_revision"],
            min(context["deadline"], time.monotonic() + 3.0),
        ) if observed_b is not None else None)
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
                labels=labels, target_label=labels[0], other_label=labels[1],
            )
            pre_return_screen = managed_pane_screen_markers(
                binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
            )
            switch_back_a = select_session(
                attach, labels[0], labels[1], labels,
                min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
                return_trace, pre_read=pre_return_screen, fresh_child_read=return_child_read,
            )
            result["return_trace"] = return_trace
            if not switch_back_a:
                navigation_a = explicit_fixture_navigation(
                    binary, server_dirs[0], handles[0].expected_target, project, session_a,
                )
                switch_back_a = navigation_a.get("selected") is True
                result["explicit_navigation_fallback_used"] = (
                    result["explicit_navigation_fallback_used"] or switch_back_a
                )
                result["explicit_navigation_a_revision"] = navigation_a.get("revision")
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

        # Re-open the exact current project from the public Ctrl-O
        # session/project menu. This exercises its Home route without creating
        # a session or typing into the prompt.
        pre_home_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
        )
        if (pre_home_screen.get("status") != "known"
                or pre_home_screen.get("contains_opencode_title") is not True):
            result["reason"] = "fresh_home_pre_action_read_unverified"
            return result
        attach.drain(0.05)
        home_pre_sequence = getattr(attach, "total_output_bytes", len(attach.output))
        if not attach.screen_text():
            result["reason"] = "home_pre_action_screen_unavailable"
            return result
        result["attempted_control_count"] += 1
        attach.send(b"\x0f")
        home_trace: dict[str, Any] = {}
        home_deadline = min(context["deadline"], time.monotonic() + SWITCH_DEADLINE)
        if not visible_session_menu(attach, labels, home_deadline, home_trace,
                                    after_output_sequence=home_pre_sequence):
            result["reason"] = "open_project_menu_not_safely_identified"
            result["home_trace"] = home_trace
            return result
        attach.send(str(project).encode("utf-8"))
        project_filter_sequence = getattr(attach, "total_output_bytes", len(attach.output))
        project_visible = False
        filter_deadline = min(context["deadline"], time.monotonic() + 1.5)
        while time.monotonic() < filter_deadline and attach.alive():
            attach.drain(0.05)
            screen = attach.screen_text()
            picker_screen = open_project_picker_region(screen)
            output_sequence = getattr(attach, "total_output_bytes", len(attach.output))
            home_trace["project_path_visible_anywhere"] = str(project) in screen
            home_trace["project_path_visible_in_modal"] = str(project) in picker_screen
            home_trace["project_filter_output_advanced"] = output_sequence > project_filter_sequence
            home_trace["project_path_occurrences_anywhere"] = screen.count(str(project))
            home_trace["project_path_occurrences_in_modal"] = picker_screen.count(str(project))
            if (session_picker_heading(screen.lower()) and str(project) in picker_screen
                    and output_sequence > project_filter_sequence):
                project_visible = True
                break
        if not project_visible:
            result["reason"] = "project_home_row_not_safely_identified"
            result["home_trace"] = home_trace
            return result
        attach.drain(0.02)
        attach.send(b"\r")
        stale = status_after(handles[0], "stale", min(context["deadline"], time.monotonic() + 5.0))
        # Return only by selecting exact root A from the same visible picker.
        pre_home_return_screen = managed_pane_screen_markers(
            binary, server_dirs[0], screen_observer_context, handles[0].expected_target,
        )
        returned = select_session(
            attach, labels[0], labels[1], labels,
            min(context["deadline"], time.monotonic() + SWITCH_DEADLINE),
            pre_read=pre_home_return_screen,
        )
        if not returned:
            navigation_home_return = explicit_fixture_navigation(
                binary, server_dirs[0], handles[0].expected_target, project, session_a,
            )
            returned = navigation_home_return.get("selected") is True
            result["explicit_navigation_fallback_used"] = (
                result["explicit_navigation_fallback_used"] or returned
            )
            result["explicit_navigation_home_return_revision"] = navigation_home_return.get("revision")
        restored = status_after(handles[0], "last_confirmed",
                                min(context["deadline"], time.monotonic() + 5.0)) if returned else None
        result["home_stale_and_returned"] = bool(stale and restored and restored["last_confirmed_revision"] > first["last_confirmed_revision"])
        if not result["home_stale_and_returned"]:
            result["reason"] = "home_stale_return_not_proven"
            return result

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
                result["current_at_crash"] = historical[0]["current_at_crash"] is False
                result["automatic_resume"] = historical[0]["automatic_resume"] is False
        if result["initial_last_confirmed"]:
            no_user_messages, _message_statuses = verify_no_user_messages(
                context["base"], context["password"], (session_a, session_b),
            )
            result["user_message_absence_after"] = no_user_messages
            result["user_message_absence_verified"] = (
                result.get("user_message_absence_before") is True and no_user_messages
            )
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
        result = {
            "classification": ("proven" if result.get("classification") == "selected_tui_identity_observed"
                               and result.get("tui", {}).get("classification") == "proven"
                               and result.get("tui", {}).get("user_message_absence_verified") is True
                               else result.get("tui", {}).get("classification", result.get("classification", "blocked"))),
            "reason": result.get("tui", {}).get("reason", result.get("reason", "worker_failed")),
            "opencode_version": result.get("version"),
            "pinned_binary_match": result.get("binary_sha256_matches") is True,
            "sandbox": "worker", "network_scope": result.get("network_scope"),
            "prompt_submissions": result.get("prompt_submissions", 0),
            "user_message_absence_verified": result.get("tui", {}).get("user_message_absence_verified") is True,
            "cleanup_completed": result.get("cleanup_completed") is True
                and result.get("tui", {}).get("cleanup_completed") is True,
            **{key: result.get("tui", {}).get(key) for key in (
                "managed_serve_count", "initial_last_confirmed", "root_a_to_b_to_a",
                "keyboard_route_proven", "explicit_navigation_fallback_used",
                "explicit_navigation_b_revision", "explicit_navigation_a_revision",
                "explicit_navigation_home_return_revision",
                "initial_a_binding_verified",
                "passive_b_unchanged", "home_stale_and_returned", "historical_after_stop",
                "historical_identity_matches", "historical_revision_advanced",
                "current_at_crash", "automatic_resume", "targets", "final_revisions",
                "bridge_snapshots", "serve_lines_valid", "historical_lookup",
                "pane_status", "pane_screen", "tui_markers", "switch_trace", "return_trace",
                "home_trace",
                "binding_revision_advanced_for_b", "binding_revision_advanced_for_a",
                "interim_b_binding_verified", "return_a_binding_verified",
                "live_binding_checkpoints", "user_message_absence_before",
                "user_message_absence_after", "user_message_absence_verified",
            )},
        }
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        return 0 if result["classification"] == "proven" and result["cleanup_completed"] else 2
    code, result = run(args.panelink_binary, args.opencode)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
