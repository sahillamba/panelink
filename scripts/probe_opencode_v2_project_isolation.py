#!/usr/bin/env python3
"""Prove exact selected-session scoping across two owned private projects."""

from __future__ import annotations

import json
import secrets
import time
from pathlib import Path
from typing import Any

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts.probe_opencode_v2_tui_identity import (
        PrivatePty, _records_with_policy, hash_value, current_route, _tick,
    )
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    from probe_opencode_v2_tui_identity import (
        PrivatePty, _records_with_policy, hash_value, current_route, _tick,
    )


def scope_proof(records: list[dict[str, Any]], *, session_hash: str,
                project_hash: str) -> bool:
    """Ignore unresolved startup cache, then require every row to keep exact identity."""
    resolved = False
    for row in records:
        row_project = row.get("project_hash")
        row_hash = row.get("session_id_hash")
        top_level = row.get("session_top_level")
        if row_project is not None and row_project != project_hash:
            return False
        if top_level is False:
            return False
        exact = (row.get("route_name") == "session" and row_hash == session_hash
                 and top_level is True and row_project == project_hash)
        if exact:
            resolved = True
            continue
        if resolved or (row_hash is not None and row_hash != session_hash):
            return False
    return resolved and current_route(records, session_hash, project_hash)


def _owned(context: dict[str, Any]) -> bool:
    pid, port = context.get("server_pid"), context.get("port")
    return isinstance(pid, int) and not isinstance(pid, bool) and isinstance(port, int) \
        and bootstrap.server_owns_port(port, pid)


def _verify_extra_empty(context: dict[str, Any], identifier: str) -> tuple[bool, int | None]:
    """Read the single extra fixture session only while its exact server owns the listener."""
    if not _owned(context):
        return False, None
    status, body = bootstrap.request(
        context["base"], "GET", f"/api/session/{identifier}/message?type=user&limit=1",
        password=context["password"], timeout=0.5)
    try:
        value = json.loads(body) if status == 200 else None
    except (ValueError, UnicodeDecodeError):
        value = None
    return status == 200 and isinstance(value, dict) and value.get("data") == [], status


def _originals_advanced(context: dict[str, Any], watermarks: list[int]) -> bool:
    paths, nonces = context["record_paths"], context["nonces"]
    hashes, project_hash = context["hashes"], context["project_hash"]
    if not (len(paths) == len(nonces) == len(hashes) == len(watermarks) == 2):
        return False
    for path, nonce, expected, watermark in zip(paths, nonces, hashes, watermarks):
        records, rejected = _records_with_policy(Path(path), nonce, project_hash,
                                                  enforce_gap_bound=True)
        tail = [row for row in records if int(row["seq"]) > watermark]
        if (rejected or not tail
                or not all(row.get("route_name") == "session"
                           and row.get("session_top_level") is True
                           and row.get("session_id_hash") == expected
                           and row.get("project_hash") == project_hash for row in tail)
                or not current_route(records, expected, project_hash)):
            return False
    return True


def after_routes(context: dict[str, Any]) -> dict[str, Any]:
    """Attach one owned TUI to a second project; never submits a prompt."""
    result: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "second_project_created": False, "second_session_verified": False,
        "second_tui_route_observed": False, "cross_project_rejected": False,
        "first_project_observers_fresh": False, "extra_tui_cleanup_completed": False,
        "extra_session_empty": False, "prompt_submissions": 0,
    }
    tui: PrivatePty | None = None
    session_id: str | None = None
    watermarks: list[int] = []
    try:
        root: Path = context["root"]
        project: Path = context["project"]
        deadline: float = context["deadline"]
        if not _owned(context):
            result["reason"] = "owned_server_listener_unverified"
            return result

        # Save the existing observation boundary before the extra client starts.
        for path, nonce, expected in zip(context["record_paths"], context["nonces"],
                                        context["hashes"]):
            rows, rejected = _records_with_policy(Path(path), nonce, context["project_hash"],
                                                   enforce_gap_bound=True)
            if rejected or not current_route(rows, expected, context["project_hash"]):
                result["reason"] = "original_observer_not_fresh_at_hook_start"
                return result
            watermarks.append(int(rows[-1]["seq"]))

        second_project = root / "project-isolation"
        second_project.mkdir(mode=0o700)
        if second_project.stat().st_mode & 0o077:
            result["reason"] = "private_project_permissions_unverified"
            return result
        result["second_project_created"] = True
        if not _owned(context):
            result["reason"] = "owned_server_listener_lost"
            return result
        payload = json.dumps({"title": "Panelink V2 Project Isolation",
                              "location": {"directory": str(second_project)}}).encode()
        status, body = bootstrap.request(context["base"], "POST", "/api/session",
                                         password=context["password"], body=payload, timeout=0.5)
        session_id = bootstrap.session_payload_id(body) if status == 200 else None
        if not session_id:
            result["reason"] = "second_project_session_create_failed"
            return result

        query = ("/api/session?directory=" + bootstrap.quote(str(second_project), safe="")
                 + "&parentID=null&limit=20&archived=false")
        if not _owned(context):
            result["reason"] = "owned_server_listener_lost"
            return result
        list_status, list_body = bootstrap.request(context["base"], "GET", query,
                                                   password=context["password"], timeout=0.5)
        try:
            listed = json.loads(list_body) if list_status == 200 else None
        except (ValueError, UnicodeDecodeError):
            listed = None
        rows = listed.get("data") if isinstance(listed, dict) else None
        if not isinstance(rows, list) or len(rows) > 20:
            result["reason"] = "second_project_session_scope_unverified"
            return result
        expected_directory = second_project.resolve(strict=True)
        matching = []
        for row in rows:
            if not isinstance(row, dict) or row.get("id") != session_id:
                continue
            location = row.get("location")
            directory = location.get("directory") if isinstance(location, dict) else None
            if not isinstance(directory, str):
                continue
            try:
                same_project = Path(directory).resolve(strict=True) == expected_directory
            except (OSError, ValueError):
                same_project = False
            if (row.get("parentID") is None and row.get("parentId") is None
                    and same_project):
                matching.append(row)
        if len(matching) != 1:
            result["reason"] = "second_project_session_scope_unverified"
            return result
        result["second_session_verified"] = True

        extra_path = root / "tui-project-isolation.jsonl"
        extra_path.touch(mode=0o600)
        extra_path.chmod(0o600)
        nonce = secrets.token_hex(16)
        extra_env = dict(context["env"])
        extra_env.update({
            "TERM": "xterm-256color",
            "OPENCODE_CONFIG_DIR": str(root / "opencode-config"),
            "PANELINK_PROBE_RECORDS": str(extra_path),
            "PANELINK_PROBE_CLIENT_NONCE": nonce,
            "PANELINK_PROBE_CONTROL": str(root / "tui-project-isolation.control"),
        })
        argv = bootstrap.launcher_argv(
            context["executable"], context["launcher"],
            ["--server", context["base"], "--session", session_id, str(second_project)],
        )
        tui = PrivatePty(argv, second_project, extra_env)
        project_hash = hash_value(str(second_project.resolve()))
        expected_hash = hash_value(session_id)

        def load_extra() -> tuple[list[dict[str, Any]], int]:
            return _records_with_policy(extra_path, nonce, project_hash,
                                        enforce_gap_bound=True)

        def observed() -> bool:
            records, rejected = load_extra()
            result["rejected_record_count"] = max(result.get("rejected_record_count", 0), rejected)
            return rejected == 0 and scope_proof(
                records, session_hash=expected_hash, project_hash=project_hash)

        observe_deadline = min(deadline, time.monotonic() + 10.0)
        live_tuis = list(context["tuis"]) + [tui]
        while time.monotonic() < observe_deadline and not observed():
            if result.get("rejected_record_count", 0):
                result["reason"] = "invalid_or_rejected_extra_tui_records"
                break
            _tick(live_tuis, min(deadline, time.monotonic() + 0.1))
            if not all(active.alive() for active in live_tuis):
                result["reason"] = "tui_exited_during_project_observation"
                return result
        result["second_tui_route_observed"] = observed()
        records, rejected = load_extra()
        result["rejected_record_count"] = max(result.get("rejected_record_count", 0), rejected)
        result["cross_project_rejected"] = (
            rejected == 0 and not scope_proof(
                records, session_hash=expected_hash, project_hash=context["project_hash"]))
        result["first_project_observers_fresh"] = _originals_advanced(context, watermarks)
        empty, status = _verify_extra_empty(context, session_id)
        result["extra_session_empty"] = empty
        result["extra_session_read_status"] = status
        if (result["rejected_record_count"] == 0 and all(result[key] for key in (
                "second_session_verified", "second_tui_route_observed", "cross_project_rejected",
                "first_project_observers_fresh", "extra_session_empty"))):
            result.update(classification="proven", reason="separate_project_identity_isolation_observed")
        else:
            result["reason"] = "project_identity_isolation_not_proven"
        return result
    except Exception:
        result["reason"] = "project_isolation_probe_failed"
        return result
    finally:
        if tui is not None:
            try:
                reaped = bool(tui.child is not None and bootstrap._kill_group(tui.child))
                tui.close()
                result["extra_tui_cleanup_completed"] = reaped
            except Exception:
                result["extra_tui_cleanup_completed"] = False
        if session_id:
            empty, status = _verify_extra_empty(context, session_id)
            result["extra_session_empty"] = bool(result.get("extra_session_empty") and empty)
            result["extra_session_read_status"] = status
            if result.get("classification") == "proven" and not result["extra_session_empty"]:
                result.update(classification="not_proven",
                              reason="extra_session_empty_verification_failed")
        if watermarks:
            result["first_project_observers_fresh"] = _originals_advanced(context, watermarks)
            if (result.get("classification") == "proven"
                    and not result["first_project_observers_fresh"]):
                result.update(classification="not_proven",
                              reason="original_observer_drift_after_cleanup")
        if result.get("classification") == "proven" and not result["extra_tui_cleanup_completed"]:
            result.update(classification="not_proven", reason="extra_tui_cleanup_failed")
