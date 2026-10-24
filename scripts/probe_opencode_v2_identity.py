#!/usr/bin/env python3
"""Bounded, loopback-only OpenCode 2.0.32 API bootstrap preflight.

This establishes pinned binary/version, ownership of a fresh local server, and
creation/listing of two empty top-level sessions. It does not claim TUI identity
or session-switch proof. It never submits prompts or makes provider/model calls.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import selectors
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable
import urllib.error
import urllib.request
from urllib.parse import quote, urlsplit

try:
    from scripts.probe_opencode_identity import (
        bounded_command, environment, parse_version, resolve_executable,
    )
except ModuleNotFoundError:
    from probe_opencode_identity import (
        bounded_command, environment, parse_version, resolve_executable,
    )


EXPECTED_VERSION = "2.0.32"
EXPECTED_SHA256 = "e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf"
TOTAL_DEADLINE_SECONDS = 60.0
SERVER_DEADLINE_SECONDS = 12.0
WORKER_ENV = "PANELINK_OPENCODE_V2_SANDBOX_WORKER"
SANDBOX_PROFILE = '''(version 1)
(allow default)
(deny network*)
(allow network-outbound (remote ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
'''
MAX_RESPONSE_BYTES = 16 * 1024
TUI_SCENARIOS = ("baseline", "reverse-new", "bridge-health", "child-routes", "project-isolation")


def launcher_argv(executable: str, launcher: str, args: list[str]) -> list[str]:
    if launcher == "tescode":
        return [executable, "workflow", "custom", *args]
    return [executable, *args]


def sha256_file(path: Path) -> str | None:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as stream:
            while block := stream.read(1024 * 1024):
                digest.update(block)
    except OSError:
        return None
    return digest.hexdigest()


def free_loopback_port() -> int | None:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", 0))
            return sock.getsockname()[1]
    except OSError:
        return None


def server_owns_port(port: int, expected_pid: int) -> bool:
    """Fail closed unless the listener is the exact serve child we launched."""
    try:
        result = subprocess.run(
            ["/usr/sbin/lsof", "-nP", "-t", f"-iTCP:{port}", "-sTCP:LISTEN"],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=1.0, check=False, close_fds=True, shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    if result.returncode != 0:
        return False
    try:
        pids = {int(line) for line in result.stdout.splitlines()}
    except ValueError:
        return False
    return pids == {expected_pid}


class _NoRedirectHandler(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def request(base: str, method: str, path: str, *, password: str,
            body: bytes | None = None, timeout: float = 2.0) -> tuple[int | None, bytes]:
    """Make bounded loopback HTTP calls with explicit Basic auth and no redirects."""
    parsed = urlsplit(base)
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.port is None
            or parsed.username is not None or parsed.password is not None
            or not path.startswith("/") or path.startswith("//")):
        return None, b""
    token = base64.b64encode(f"opencode:{password}".encode("utf-8")).decode("ascii")
    headers = {"Authorization": f"Basic {token}"}
    if body is not None:
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(base + path, data=body, method=method, headers=headers)
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), _NoRedirectHandler(),
    )
    try:
        with opener.open(req, timeout=timeout) as response:
            payload = response.read(MAX_RESPONSE_BYTES + 1)
            if len(payload) > MAX_RESPONSE_BYTES:
                return response.status, b""
            return response.status, payload
    except urllib.error.HTTPError as exc:
        status = exc.code
        exc.close()
        return status, b""
    except (urllib.error.URLError, TimeoutError, OSError):
        return None, b""


def session_payload_id(payload: bytes) -> str | None:
    """Accept only a v2 response carrying a bounded top-level session ID."""
    try:
        result = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return None
    data = result.get("data") if isinstance(result, dict) else None
    identifier = data.get("id") if isinstance(data, dict) else None
    if (isinstance(data, dict) and data.get("parentID") is None and data.get("parentId") is None
            and isinstance(identifier, str) and 1 <= len(identifier) <= 128
            and all(char.isascii() and (char.isalnum() or char in "_.-") for char in identifier)):
        return identifier
    return None


def verify_no_user_messages(base: str, password: str,
                            session_ids: tuple[str, ...]) -> tuple[bool, list[int | None]]:
    """Read only fixture sessions; retain status codes, never message bodies."""
    statuses: list[int | None] = []
    empty = 2 <= len(session_ids) <= 3 and len(set(session_ids)) == len(session_ids)
    if not empty:
        return False, statuses
    for identifier in session_ids:
        status, body = request(base, "GET", f"/api/session/{identifier}/message?type=user&limit=1",
                               password=password)
        statuses.append(status)
        try:
            value = json.loads(body) if status == 200 else None
        except (ValueError, UnicodeDecodeError):
            value = None
        empty = empty and isinstance(value, dict) and value.get("data") == []
    return empty, statuses


def lookup_route_hash(context: dict[str, Any], route_hash: str,
                      verified_ids: list[str]) -> dict[str, Any]:
    """Validate an observed hash by exact equality, never recency or ordering."""
    result = {"exact_session": False, "project_matches": False,
              "top_level": False, "status": "error", "http_status": None}
    if (not isinstance(route_hash, str) or len(route_hash) != 64
            or any(char not in "0123456789abcdef" for char in route_hash)
            or not server_owns_port(context["port"], context["server_pid"])):
        return result
    query = "/api/session?directory=" + quote(str(context["project"]), safe="") + "&parentID=null&limit=20"
    status, body = request(context["base"], "GET", query, password=context["password"])
    result["http_status"] = status
    try:
        value = json.loads(body) if status == 200 else None
    except (ValueError, UnicodeDecodeError):
        return result
    rows = value.get("data") if isinstance(value, dict) else None
    if not isinstance(rows, list) or len(rows) > 20:
        return result
    result["status"] = "missing"
    matches = []
    for row in rows:
        identifier = row.get("id") if isinstance(row, dict) else None
        if isinstance(identifier, str) and hashlib.sha256(identifier.encode()).hexdigest() == route_hash:
            matches.append(row)
    if len(matches) != 1:
        return result
    row = matches[0]
    identifier = session_payload_id(json.dumps({"data": row}).encode())
    location = row.get("location")
    directory = location.get("directory") if isinstance(location, dict) else None
    result["exact_session"] = identifier is not None
    result["top_level"] = row.get("parentID") is None and row.get("parentId") is None
    if isinstance(directory, str):
        try:
            result["project_matches"] = Path(directory).resolve(strict=True) == context["project"].resolve(strict=True)
        except (OSError, ValueError):
            pass
    if all(result[key] for key in ("exact_session", "project_matches", "top_level")):
        if identifier not in verified_ids and len(verified_ids) < 3:
            verified_ids.append(identifier)
        if identifier not in verified_ids:
            result.update(exact_session=False, status="error")
        else:
            result["status"] = "found"
    return result


def scenario_callback(scenario: str) -> Callable[[dict[str, Any]], dict[str, Any]]:
    def observe(context: dict[str, Any]) -> dict[str, Any]:
        try:
            from scripts.probe_opencode_v2_tui_identity import after_bootstrap
        except ModuleNotFoundError:
            from probe_opencode_v2_tui_identity import after_bootstrap
        context = {**context, "scenario": scenario,
                   "new_session_key_verified": scenario == "reverse-new"}
        if scenario == "bridge-health":
            try:
                from scripts.probe_opencode_v2_bridge_health import after_routes
            except ModuleNotFoundError:
                from probe_opencode_v2_bridge_health import after_routes
            context["after_routes"] = after_routes
        elif scenario == "child-routes":
            try:
                from scripts.probe_opencode_v2_child_routes import after_routes
            except ModuleNotFoundError:
                from probe_opencode_v2_child_routes import after_routes
            context.update(after_routes=after_routes, hook_result_key="child_routes")
        elif scenario == "project-isolation":
            try:
                from scripts.probe_opencode_v2_project_isolation import after_routes
            except ModuleNotFoundError:
                from probe_opencode_v2_project_isolation import after_routes
            context.update(after_routes=after_routes, hook_result_key="project_isolation")
        return after_bootstrap(context)
    return observe


def _kill_group(child: subprocess.Popen[bytes]) -> bool:
    """Retire the full server group, including descendants of an exited leader."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return True
    except OSError:
        return child.poll() is not None
    try:
        child.wait(timeout=1)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(child.pid, 0)
    except ProcessLookupError:
        return child.poll() is not None
    except OSError:
        return False
    try:
        os.killpg(child.pid, signal.SIGKILL)
    except ProcessLookupError:
        return child.poll() is not None
    except OSError:
        return False
    try:
        child.wait(timeout=1)
    except subprocess.TimeoutExpired:
        return False
    try:
        os.killpg(child.pid, 0)
        return False
    except ProcessLookupError:
        return True
    except OSError:
        return False


def _run_worker(
    executable: str,
    launcher: str,
    *,
    after_bootstrap: Callable[[dict[str, Any]], dict[str, Any]] | None = None,
    total_deadline_seconds: float = TOTAL_DEADLINE_SECONDS,
    scenario: str = "baseline",
) -> dict[str, Any]:
    report: dict[str, Any] = {
        "scenario": scenario,
        "launcher": launcher, "binary_sha256_matches": False, "version": None,
        "version_status": "blocked", "local_server": "not_attempted",
        "server_ownership": "not_attempted", "empty_sessions": "not_attempted",
        "server_process": "not_started", "server_returncode": None,
        "listener_owned_at_deadline": "not_checked",
        "readiness_http_status": None,
        "sessions_created_count": 0, "session_create_statuses": [],
        "session_list_status": None, "switch_A_B_A_observed": False,
        "identity_proof": "not_attempted", "classification": "blocked",
        "reason": "not_started", "prompt_submissions": 0,
        "network_scope": "loopback_only_sandbox", "cleanup_completed": False,
    }
    if launcher == "tescode":
        report["reason"] = "wrapper_only_cannot_match_pinned_native_binary"
        return report
    binary = Path(executable)
    report["binary_sha256_matches"] = sha256_file(binary) == EXPECTED_SHA256
    if not report["binary_sha256_matches"]:
        report["reason"] = "pinned_binary_hash_mismatch"
        return report

    with tempfile.TemporaryDirectory(prefix="panelink-opencode-v2-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        env = environment(root)
        env.update({"OPENCODE_DISABLE_AUTOUPDATE": "1", "OPENCODE_DISABLE_MODELS_FETCH": "1"})
        password = secrets.token_urlsafe(32)
        env["OPENCODE_PASSWORD"] = password
        project = root / "project"
        project.mkdir(mode=0o700)
        started = time.monotonic()
        deadline = started + min(TOTAL_DEADLINE_SECONDS, max(1.0, total_deadline_seconds))
        version = bounded_command(launcher_argv(executable, launcher, ["--version"]), project, env)
        parsed = parse_version(version.stdout + b"\n" + version.stderr)
        report["version"] = parsed
        if version.reason or version.returncode != 0 or parsed != EXPECTED_VERSION:
            report["version_status"] = "execution_failed" if version.reason or version.returncode else "version_mismatch"
            report["reason"] = "pinned_version_mismatch"
            return report
        report["version_status"] = "proven"

        port = free_loopback_port()
        if port is None:
            report.update(reason="loopback_port_unavailable", local_server="blocked")
            return report
        base = f"http://127.0.0.1:{port}"
        server: subprocess.Popen[bytes] | None = None
        try:
            server = subprocess.Popen(
                launcher_argv(executable, launcher,
                              ["serve", "--hostname", "127.0.0.1", "--port", str(port)]),
                cwd=project, env=env, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                close_fds=True, shell=False, start_new_session=True,
            )
            report["server_process"] = "running"
            ready_deadline = min(deadline, time.monotonic() + SERVER_DEADLINE_SECONDS)
            ready = False
            status = None
            info = b""
            while time.monotonic() < ready_deadline and server.poll() is None:
                if not server_owns_port(port, server.pid):
                    time.sleep(min(0.1, max(0.0, ready_deadline - time.monotonic())))
                    continue
                status, info = request(base, "GET", "/api/info", password=password, timeout=0.35)
                report["readiness_http_status"] = status
                if status == 200:
                    ready = True
                    break
                if status in (401, 403):
                    break
                time.sleep(min(0.1, max(0.0, ready_deadline - time.monotonic())))
            if not ready:
                returncode = server.poll()
                report.update(reason="local_server_not_ready", local_server="blocked",
                              server_process="exited" if returncode is not None else "running",
                              server_returncode=returncode,
                              readiness_http_status=status,
                              listener_owned_at_deadline=server_owns_port(port, server.pid))
                if status in (401, 403) and report["listener_owned_at_deadline"]:
                    report["reason"] = "local_server_authentication_required"
                return report
            if server.poll() is not None or not server_owns_port(port, server.pid):
                report.update(reason="local_server_ownership_unproven", local_server="blocked",
                              server_ownership="blocked")
                return report
            report["server_ownership"] = "exact_child_pid"
            report["listener_owned_at_deadline"] = True
            try:
                info_data = json.loads(info) if status == 200 else {}
            except (ValueError, UnicodeDecodeError):
                info_data = {}
            info_pid = info_data.get("pid") if isinstance(info_data, dict) else None
            if (not isinstance(info_data, dict) or info_data.get("version") != EXPECTED_VERSION
                    or not isinstance(info_pid, int) or isinstance(info_pid, bool)
                    or info_pid != server.pid):
                report.update(reason="local_server_identity_mismatch", local_server="blocked")
                return report
            report["local_server"] = "ready_exact_version_owned_loopback"

            ids: list[str] = []
            labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")
            for label in labels:
                if server.poll() is not None or not server_owns_port(port, server.pid):
                    report.update(reason="local_server_ownership_lost", empty_sessions="blocked",
                                  server_ownership="blocked")
                    return report
                payload = json.dumps({"title": label, "location": {"directory": str(project)}}).encode()
                code, body = request(base, "POST", "/api/session", password=password, body=payload)
                report["session_create_statuses"].append(code)
                identifier = session_payload_id(body) if code == 200 else None
                if identifier is None or identifier in ids:
                    report.update(reason="empty_top_level_session_create_failed", empty_sessions="blocked")
                    return report
                ids.append(identifier)
                report["sessions_created_count"] = len(ids)
            query = ("/api/session?directory=" + quote(str(project), safe="")
                     + "&parentID=null&limit=20&archived=false")
            list_status, list_body = request(base, "GET", query, password=password)
            report["session_list_status"] = list_status
            try:
                listed_value = json.loads(list_body) if list_status == 200 else {}
            except (ValueError, UnicodeDecodeError):
                listed_value = {}
            listed_data = listed_value.get("data") if isinstance(listed_value, dict) else None
            listed_ids = {
                item.get("id") for item in listed_data if isinstance(item, dict)
                and item.get("parentID") is None and item.get("parentId") is None
                and item.get("title") in labels
                and isinstance(item.get("id"), str)
            } if isinstance(listed_data, list) else set()
            if list_status != 200 or not set(ids).issubset(listed_ids):
                report.update(reason="empty_top_level_sessions_not_verified", empty_sessions="blocked")
                return report
            report.update(empty_sessions="two_top_level_empty_sessions_created", sessions_created_count=2,
                          classification="preflight_passed_identity_not_proven",
                          identity_proof="not_attempted_tui_switch_unverified",
                          reason="tui_switch_not_attempted_in_preflight")
            if after_bootstrap is not None:
                verified_ids = list(ids)
                try:
                    context = {
                        "root": root,
                        "project": project,
                        "env": env,
                        "base": base,
                        "password": password,
                        "session_ids": tuple(ids),
                        "deadline": deadline,
                        "executable": executable,
                        "launcher": launcher,
                        "version": EXPECTED_VERSION,
                        "server_pid": server.pid,
                        "port": port,
                    }
                    context["lookup_route_hash"] = lambda route_hash: lookup_route_hash(context, route_hash, verified_ids)
                    tui_result = after_bootstrap(context)
                except (OSError, subprocess.SubprocessError, TimeoutError):
                    tui_result = {"classification": "blocked", "reason": "tui_adapter_execution_failed"}
                if isinstance(tui_result, dict):
                    if tui_result.get("attempted_control_count", 0) > 0:
                        no_messages, statuses = (False, [])
                        if server.poll() is None and server_owns_port(port, server.pid):
                            no_messages, statuses = verify_no_user_messages(base, password, tuple(verified_ids))
                        tui_result["user_message_absence_verified"] = no_messages
                        tui_result["user_message_read_statuses"] = statuses
                        if not no_messages:
                            tui_result.update(classification="not_proven",
                                              reason="fixture_user_message_absence_unverified")
                    report["tui"] = tui_result
                    if tui_result.get("classification") == "proven":
                        report.update(classification="selected_tui_identity_observed",
                                      switch_A_B_A_observed=tui_result.get("first_tui_A_B_A_observed") is True,
                                      controlled_round_trip_observed=tui_result.get("active_round_trip_observed", tui_result.get("first_tui_A_B_A_observed")) is True,
                                      identity_proof="selected_route_and_ordered_switches_observed",
                                      reason="selected_tui_identity_observed")
                    else:
                        report.update(classification="preflight_passed_identity_not_proven",
                                      identity_proof="selected_tui_identity_not_proven",
                                      reason=tui_result.get("reason", "tui_identity_not_proven"))
            return report
        except (OSError, subprocess.SubprocessError):
            report.update(reason="bounded_local_probe_execution_failed", classification="blocked")
            return report
        finally:
            cleanup_ok = True
            if server is not None:
                cleanup_ok = _kill_group(server)
            report["cleanup_completed"] = cleanup_ok
            if (after_bootstrap is not None and not cleanup_ok
                    and report.get("classification") == "selected_tui_identity_observed"):
                report.update(classification="blocked", identity_proof="not_proven",
                              reason="fixture_server_cleanup_unverified")
            report["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)


def bounded_worker(argv: list[str], cwd: Path, env: dict[str, str],
                   timeout: float = 70.0) -> tuple[int, bytes, str | None]:
    """Capture tiny allowlisted worker output without buffering unbounded data."""
    child = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             close_fds=True, shell=False, start_new_session=True, bufsize=0)
    assert child.stdout is not None and child.stderr is not None
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ, "stdout")
    selector.register(child.stderr, selectors.EVENT_READ, "stderr")
    output = bytearray()
    stderr = bytearray()
    started = time.monotonic()
    try:
        while selector.get_map() and time.monotonic() - started < timeout:
            remaining = max(0.0, timeout - (time.monotonic() - started))
            for key, _ in selector.select(timeout=min(0.1, remaining)):
                chunk = os.read(key.fileobj.fileno(), 4096)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                target = output if key.data == "stdout" else stderr
                limit = 16 * 1024 if key.data == "stdout" else 4 * 1024
                if len(target) + len(chunk) > limit:
                    _terminate_worker(child)
                    return 2, b"", "sandbox_worker_output_limit"
                target.extend(chunk)
        if time.monotonic() - started >= timeout and child.poll() is None:
            _terminate_worker(child)
            return 2, b"", "sandbox_worker_timeout"
        child.wait(timeout=1)
        error_text = bytes(stderr).decode("utf-8", "replace").lower()
        if "sandbox_apply: operation not permitted" in error_text:
            error_tag = "sandbox_enforcement_unavailable"
        elif "sandbox-exec:" in error_text:
            error_tag = "sandbox_policy_rejected"
        else:
            error_tag = None
        return child.returncode or 0, bytes(output), error_tag
    except (OSError, subprocess.SubprocessError):
        _terminate_worker(child)
        return 2, b"", "sandbox_worker_io_failed"
    finally:
        selector.close()
        child.stdout.close()
        child.stderr.close()


def _terminate_worker(child: subprocess.Popen[bytes]) -> bool:
    """Give the worker a TERM/grace interval to unwind its cleanup finally."""
    if child.poll() is not None:
        return True
    try:
        os.killpg(child.pid, signal.SIGTERM)
        child.wait(timeout=2)
        return True
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            child.wait(timeout=1)
            return True
        except subprocess.SubprocessError:
            return False
    except OSError:
        return child.poll() is not None


def run(executable: str | None = None, *, which=shutil.which, home: Path | None = None,
        tui: bool = False, scenario: str = "baseline") -> tuple[int, dict[str, Any]]:
    report: dict[str, Any] = {"classification": "blocked", "reason": "not_started",
                              "version_status": "blocked", "sandbox": "not_started",
                              "network_scope": "loopback_only_sandbox", "prompt_submissions": 0,
                              "switch_A_B_A_observed": False, "identity_proof": "not_attempted"}
    if scenario not in TUI_SCENARIOS or (not tui and scenario != "baseline"):
        return 2, {**report, "reason": "unsupported_scenario"}
    chosen, launcher = resolve_executable(executable, which=which, home=home)
    if chosen is None or launcher is None:
        report["reason"] = "launcher_not_found"
        return 2, report
    if sys.platform != "darwin":
        report.update(reason="macos_sandbox_required", launcher=launcher)
        return 2, report
    sandbox = shutil.which("sandbox-exec")
    if sandbox is None:
        report.update(reason="sandbox_exec_unavailable", launcher=launcher)
        return 2, report
    cwd = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="panelink-v2-sandbox-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        profile = root / "loopback.sb"
        profile.write_text(SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        worker_env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C", WORKER_ENV: "1"}
        argv = [sandbox, "-f", str(profile), sys.executable, str(Path(__file__).resolve()),
                "--sandbox-worker", chosen, launcher]
        if tui:
            argv.append("--tui-worker")
            argv.extend(["--tui-scenario", scenario])
        code, raw, sandbox_error = bounded_worker(argv, cwd, worker_env,
                                                  timeout=60.0 if tui else 70.0)
    try:
        result = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        result = {"classification": "blocked", "reason": sandbox_error or "sandbox_worker_invalid_output"}
    if not isinstance(result, dict):
        result = {"classification": "blocked", "reason": "sandbox_worker_invalid_output"}
    result["sandbox"] = "not_enforced" if sandbox_error else "enforced_loopback_only"
    result["sandbox_profile"] = "deny_network_except_loopback"
    successful_tui = (
        tui and result.get("classification") == "selected_tui_identity_observed"
        and result.get("cleanup_completed") is True
        and isinstance(result.get("tui"), dict)
        and result["tui"].get("tui_cleanup_completed") is True
    )
    successful_standard = not tui and code == 0 and result.get("classification") == "proven"
    return (0 if code == 0 and (successful_tui or successful_standard) else 2), result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sandbox-worker", nargs=2, metavar=("BINARY", "LAUNCHER"), help=argparse.SUPPRESS)
    parser.add_argument("--tui-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--tui-scenario", choices=TUI_SCENARIOS, default="baseline", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.sandbox_worker:
        if os.environ.get(WORKER_ENV) != "1":
            print(json.dumps({"classification": "blocked", "reason": "sandbox_worker_marker_missing"}))
            return 2
        def graceful_exit(_signum: int, _frame: Any) -> None:
            raise SystemExit(128 + signal.SIGTERM)
        signal.signal(signal.SIGTERM, graceful_exit)
        after_bootstrap = None
        if args.tui_worker:
            after_bootstrap = scenario_callback(args.tui_scenario)
        result = _run_worker(*args.sandbox_worker, after_bootstrap=after_bootstrap,
                             total_deadline_seconds=52.0 if args.tui_worker else TOTAL_DEADLINE_SECONDS,
                             scenario=args.tui_scenario)
        print(json.dumps(result, sort_keys=True, separators=(",", ":")))
        tui_success = (
            args.tui_worker
            and result.get("classification") == "selected_tui_identity_observed"
            and result.get("cleanup_completed") is True
            and isinstance(result.get("tui"), dict)
            and result["tui"].get("tui_cleanup_completed") is True
        )
        standard_success = not args.tui_worker and result.get("classification") == "proven"
        return 0 if tui_success or standard_success else 2
    code, result = run()
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
