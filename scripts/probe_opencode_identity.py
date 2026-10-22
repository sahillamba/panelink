#!/usr/bin/env python3
"""Offline, isolated, bounded OpenCode identity-surface experiment.

No provider request is made. Output is an allowlisted summary, never CLI/API text.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import selectors
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from typing import Callable, cast, IO
import urllib.error
import urllib.request

try:
    from scripts.probe_opencode import CaptureResult, _kill_group, VERSION
except ModuleNotFoundError:
    from probe_opencode import CaptureResult, _kill_group, VERSION


TIMEOUT = 8.0
LIMIT = 16384


def resolve_executable(
    explicit: str | None = None, *,
    which: Callable[[str], str | None] = shutil.which,
    home: Path | None = None,
    executable: Callable[[Path], bool] | None = None,
) -> tuple[str | None, str | None]:
    """Resolve CLI without executing a candidate or reading user configuration."""
    def default_usable(path: Path) -> bool:
        return path.is_file() and os.access(path, os.X_OK)

    usable = executable or default_usable
    if explicit is not None:
        path = Path(explicit).expanduser()
        if not path.is_absolute() and path.parent == Path("."):
            found = which(explicit)
            path = Path(found) if found else path
        if usable(path):
            return str(path), "tescode" if path.name == "tescode" else "opencode"
        return None, None
    candidate = which("opencode")
    if candidate and usable(Path(candidate)):
        return candidate, "opencode"
    candidate_path = (home if home is not None else Path.home()) / ".opencode" / "bin" / "opencode"
    if usable(candidate_path):
        return str(candidate_path), "opencode"
    wrapper = which("tescode")
    if wrapper and usable(Path(wrapper)):
        return wrapper, "tescode"
    return None, None


def wrapper_only_report(report: dict[str, object]) -> dict[str, object]:
    """Wrapper help/version cannot prove native OpenCode identity endpoints."""
    report.update(server="blocked_wrapper_only", serve_help="blocked_wrapper_only",
                  session_list="blocked_wrapper_only", cli_session_list="blocked_wrapper_only",
                  cli_export="blocked_wrapper_only", event_sse="blocked_wrapper_only",
                  selected_route="blocked_wrapper_only", two_sessions="blocked_wrapper_only",
                  switch="not_proven")
    return report


def environment(root: Path) -> dict[str, str]:
    """Fresh private HOME and all relevant XDG roots, no inherited auth/config."""
    result = {"HOME": str(root), "TMPDIR": str(root), "PATH": os.defpath,
              "LANG": "C", "LC_ALL": "C", "OPENCODE_DISABLE_MODELS_FETCH": "1",
              "OPENCODE_DISABLE_UPDATE_CHECK": "1"}
    for name, folder in (("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"),
                         ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache")):
        directory = root / folder
        directory.mkdir(mode=0o700)
        result[name] = str(directory)
    return result


def parse_version(output: bytes) -> str | None:
    match = VERSION.search(output.decode("utf-8", "replace"))
    return match.group(1) if match else None


def session_id(payload: bytes) -> str | None:
    """Accept only a top-level session object's bounded opaque ID."""
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return None
    if not isinstance(value, dict) or value.get("parentID") or value.get("parentId"):
        return None
    identifier = value.get("id")
    if (isinstance(identifier, str) and 1 <= len(identifier) <= 128
            and all(ch.isascii() and (ch.isalnum() or ch in "_.-") for ch in identifier)):
        return identifier
    return None


def classify(first: str | None, second: str | None, selected: str | None) -> str:
    """Creation and a selected-session observation are independent gates."""
    if not first or not second or first == second:
        return "two_sessions_not_proven"
    if selected is None:
        return "two_sessions_created_selection_not_proven"
    if selected not in (first, second):
        return "two_sessions_created_selection_unattributable"
    return "selected_snapshot_only_switch_not_proven"


def bounded_command(argv: list[str], cwd: Path, env: dict[str, str]):
    """Use the preflight's capture semantics, always retiring the whole group."""
    child = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             shell=False, start_new_session=True, bufsize=0)
    assert child.stdout is not None and child.stderr is not None
    streams = (child.stdout, child.stderr)
    captured = {stream: bytearray() for stream in streams}
    selector = selectors.DefaultSelector()
    deadline = time.monotonic() + TIMEOUT
    reason = None
    try:
        for stream in streams:
            assert stream is not None
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ)
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                reason = "timed_out"
                break
            for key, _ in selector.select(min(remaining, 0.1)):
                stream = cast(IO[bytes], key.fileobj)
                allowance = 4096 - sum(map(len, captured.values()))
                try:
                    chunk = os.read(stream.fileno(), min(4096, allowance + 1))
                except BlockingIOError:
                    continue
                if not chunk:
                    selector.unregister(stream)
                    continue
                captured[stream].extend(chunk[:allowance])
                if len(chunk) > allowance:
                    reason = "output_limit"
                    break
            if reason:
                break
        try:
            child.wait(timeout=max(0, deadline - time.monotonic()) if not reason else 0.01)
        except subprocess.TimeoutExpired:
            reason = reason or "timed_out"
        return CaptureResult(child.returncode if child.returncode is not None else -1,
                             bytes(captured[child.stdout]), bytes(captured[child.stderr]), reason)
    finally:
        # Descendants may keep running even after the direct process exits.
        _kill_group(child)
        try:
            child.wait(timeout=1)
        finally:
            selector.close()
            for stream in streams:
                if stream is not None:
                    stream.close()


def request(base: str, method: str, path: str, body: bytes | None = None,
            timeout: float = 2.0) -> tuple[int | None, bytes]:
    """Loopback only; disable inherited proxy settings and bound response bytes."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    headers = {"Content-Type": "application/json"} if body is not None else {}
    req = urllib.request.Request(base + path, data=body, method=method, headers=headers)
    try:
        with opener.open(req, timeout=timeout) as response:
            return response.status, response.read(LIMIT + 1)[:LIMIT]
    except urllib.error.HTTPError as exc:
        exc.close()
        return exc.code, b""
    except (urllib.error.URLError, TimeoutError, OSError):
        return None, b""


def sse_probe(base: str) -> str:
    """Observe stream headers only; never wait for an event or expose payload."""
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(base + "/event", timeout=1.5) as response:
            return ("http_200_text_event_stream" if response.status == 200 and
                    "text/event-stream" in response.headers.get("Content-Type", "")
                    else f"http_{response.status}_other_content")
    except urllib.error.HTTPError as exc:
        exc.close()
        return f"http_{exc.code}"
    except (urllib.error.URLError, TimeoutError, OSError):
        return "blocked_stream_timeout_or_disconnected"


def free_loopback_port() -> int | None:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            return sock.getsockname()[1]
    except OSError:
        return None


def run(executable: str | None = None) -> dict[str, object]:
    report: dict[str, object] = {"version": None, "version_status": "blocked",
                                 "server": "not_attempted", "session_list": "not_attempted",
                                 "event_sse": "not_attempted", "selected_route": "not_attempted",
                                 "two_sessions": "not_attempted", "switch": "not_proven",
                                 "launcher": None}
    chosen, launcher = resolve_executable(executable)
    if chosen is None:
        report["version_status"] = "missing_executable"
        return report
    report["launcher"] = launcher
    with tempfile.TemporaryDirectory(prefix="panelink-identity-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        env = environment(root)
        project = root / "project"
        project.mkdir(mode=0o700)
        try:
            version = bounded_command([chosen, "version" if launcher == "tescode" else "--version"],
                                      project, env)
        except OSError:
            report["version_status"] = "execution_failed"
            return report
        if version.reason or version.returncode != 0:
            report["version_status"] = version.reason or "execution_failed"
            return report
        if launcher == "tescode":
            report["version_status"] = "wrapper_version_command_passed"
            wrapper_only_report(report)
            try:
                help_result = bounded_command([chosen, "sessions", "--help"], project, env)
            except OSError:
                report["wrapper_sessions_help"] = "execution_failed"
            else:
                report["wrapper_sessions_help"] = ("exit_0" if help_result.returncode == 0
                                                   and not help_result.reason else
                                                   help_result.reason or "nonzero_exit")
            return report
        pinned = parse_version(version.stdout + b"\n" + version.stderr)
        if not pinned:
            report["version_status"] = "unrecognized_version"
            return report
        report.update(version=pinned, version_status="proven")
        # Help is a local CLI command. Do not expose its text or infer route support from it.
        help_result = bounded_command([chosen, "serve", "--help"], project, env)
        report["serve_help"] = ("available" if help_result.returncode == 0 and not help_result.reason
                                else help_result.reason or "failed")
        port = free_loopback_port()
        if port is None:
            report["server"] = "blocked_loopback_bind"
            return report
        base = f"http://127.0.0.1:{port}"
        try:
            child = subprocess.Popen([chosen, "serve", "--hostname", "127.0.0.1",
                                      "--port", str(port)], cwd=project, env=env,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL, start_new_session=True)
        except OSError:
            report["server"] = "launch_failed"
            return report
        try:
            deadline = time.monotonic() + 8.0
            status = None
            while time.monotonic() < deadline and child.poll() is None:
                status, _ = request(base, "GET", "/session", timeout=0.35)
                if status is not None:
                    break
                time.sleep(0.1)
            if status is None:
                report["server"] = "exited_before_ready" if child.poll() is not None else "readiness_timeout"
                return report
            report["server"] = "responded"
            report["session_list"] = "http_200" if status == 200 else f"http_{status}"
            report["event_sse"] = sse_probe(base)
            # These are candidate read routes only; no focus-changing commands are issued.
            routes = ("/tui/session", "/tui/session/current", "/session/current")
            observed = None
            observations = {}
            for route in routes:
                code, payload = request(base, "GET", route)
                parsed = session_id(payload) if code == 200 else None
                observations[route] = ("selected_session_object" if parsed else
                                       "unvalidated_http_200" if code == 200 else
                                       "unavailable" if code is None else f"http_{code}")
                if code == 200:
                    observed = parsed or observed
            report["selected_route"] = observations
            if status != 200:
                report["two_sessions"] = "blocked_session_list_unavailable"
                return report
            ids = []
            for _ in range(2):
                code, payload = request(base, "POST", "/session", b"{}")
                ids.append(session_id(payload) if code == 200 else None)
            report["two_sessions"] = classify(ids[0], ids[1], observed)
            code, payload = request(base, "GET", "/tui/session/current")
            report["selected_after_creation"] = ("selected_session_object" if code == 200 and
                                                 session_id(payload) else
                                                 "unvalidated_http_200" if code == 200 else
                                                 "unavailable" if code is None else f"http_{code}")
            code, payload = request(base, "GET", "/session")
            if code == 200:
                try:
                    listed = json.loads(payload)
                    report["sessions_listed"] = (isinstance(listed, list) and
                                                  all(any(isinstance(item, dict) and item.get("id") == sid
                                                          for item in listed) for sid in ids if sid))
                except ValueError:
                    report["sessions_listed"] = False
            listed_cli = bounded_command([chosen, "session", "list"], project, env)
            report["cli_session_list"] = ("exit_0" if listed_cli.returncode == 0 and not listed_cli.reason
                                          else listed_cli.reason or "nonzero_exit")
            if ids[0]:
                exported = bounded_command([chosen, "export", ids[0]], project, env)
                report["cli_export"] = ("exit_0" if exported.returncode == 0 and not exported.reason
                                        else exported.reason or "nonzero_exit")
            return report
        finally:
            _kill_group(child)
            try:
                child.wait(timeout=1)
            except subprocess.TimeoutExpired:
                _kill_group(child)
                child.wait(timeout=1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", help="explicit CLI path (otherwise resolve OpenCode, then tescode)")
    args = parser.parse_args()
    result = run(args.executable)
    print(json.dumps(result, sort_keys=True))
    return 0 if result["version_status"] in ("proven", "wrapper_version_command_passed") else 2


if __name__ == "__main__":
    sys.exit(main())
