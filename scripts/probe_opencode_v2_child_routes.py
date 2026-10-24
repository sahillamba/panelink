#!/usr/bin/env python3
"""Bounded no-prompt linked-child route probe for native OpenCode 2.0.32.

The owned fixture starts with empty sessions. OpenCode's native fork operation
rejects an empty parent, so this probe records that outcome and exercises the
documented parentID child-creation route instead. It never submits a prompt.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import time
from typing import Any
from urllib.parse import quote

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts import probe_opencode_v2_tui_identity as tui_probe
    from scripts.probe_opencode_tui_identity import (
        PrivatePty,
        filtered_session_row_visible,
        session_list_visible,
    )
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    import probe_opencode_v2_tui_identity as tui_probe
    from probe_opencode_tui_identity import (
        PrivatePty,
        filtered_session_row_visible,
        session_list_visible,
    )


PINNED_VERSION = "2.0.32"
PINNED_SHA256 = "e43ec2df6f36842fd17d1a71a866acb9a84884b489988376c19bf9a7250dabbf"
CASE_SECONDS = 10.0
MAX_RECORD_BYTES = 2 * 1024 * 1024
MAX_RECORDS = 1000
CHILD_TITLE = "Panelink V2 Linked Child"
PARENT_TITLE = "Panelink V2 Identity A"
ID_RE = re.compile(r"^[A-Za-z0-9_.-]{1,128}$")


def _data(payload: bytes) -> dict[str, Any] | None:
    if len(payload) > bootstrap.MAX_RESPONSE_BYTES:
        return None
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return None
    data = value.get("data") if isinstance(value, dict) else None
    return data if isinstance(data, dict) else None


def _identifier(value: Any) -> str | None:
    return value if isinstance(value, str) and ID_RE.fullmatch(value) else None


def _child_metadata_matches(data: dict[str, Any] | None, *, parent_id: str,
                            project: Path) -> str | None:
    if not isinstance(data, dict):
        return None
    child_id = _identifier(data.get("id"))
    parent = data.get("parentID", data.get("parentId"))
    location = data.get("location")
    directory = location.get("directory") if isinstance(location, dict) else None
    if child_id is None or child_id == parent_id or parent != parent_id:
        return None
    if not isinstance(directory, str) or len(directory) > 4096:
        return None
    try:
        if Path(directory).resolve(strict=True) != project.resolve(strict=True):
            return None
    except OSError:
        return None
    return child_id


def _top_level_excludes_child(payload: bytes, *, child_id: str,
                              project: Path, expected_parents: set[str]) -> bool:
    if len(payload) > bootstrap.MAX_RESPONSE_BYTES:
        return False
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return False
    data = value.get("data") if isinstance(value, dict) else None
    if not isinstance(data, list) or len(data) > 20:
        return False
    ids: set[str] = set()
    for row in data:
        if not isinstance(row, dict):
            return False
        identifier = _identifier(row.get("id"))
        location = row.get("location")
        directory = location.get("directory") if isinstance(location, dict) else None
        if (identifier is None or identifier in ids or identifier == child_id
                or (row.get("parentID") is not None or row.get("parentId") is not None)
                or not isinstance(directory, str)):
            return False
        try:
            if Path(directory).resolve(strict=True) != project.resolve(strict=True):
                return False
        except OSError:
            return False
        ids.add(identifier)
    return ids == expected_parents


def _read_records(path: Path, nonce: str, project_hash: str
                  ) -> tuple[list[dict[str, Any]], int]:
    try:
        with path.open("rb") as stream:
            payload = stream.read(MAX_RECORD_BYTES + 1)
    except OSError:
        return [], 1
    return tui_probe.parse_client_records(payload, client_nonce=nonce,
                                          project_hash=project_hash)


def _child_history_matches(records: list[dict[str, Any]], *, child_hash: str,
                           parent_hash: str, project_hash: str,
                           require_return: bool = False) -> bool:
    selected_at = next((index for index, record in enumerate(records)
                        if record["route_name"] == "session"
                        and record["session_id_hash"] == child_hash
                        and record["session_top_level"] is False
                        and record["project_hash"] == project_hash), None)
    if selected_at is None:
        return False
    startup = records[:selected_at]
    if any(record["project_hash"] not in (None, project_hash)
           or record["route_name"] not in ("plugin", "home", "other", "session")
           or record["session_id_hash"] not in (None, child_hash)
           or record["session_top_level"] is not None for record in startup):
        return False
    observed = records[selected_at:]
    if any(record["route_name"] != "session"
           or record["project_hash"] != project_hash
           or (record["session_id_hash"] == child_hash
               and record["session_top_level"] is not False)
           or (record["session_id_hash"] == parent_hash
               and record["session_top_level"] is not True)
           or record["session_id_hash"] not in (child_hash, parent_hash)
           for record in observed):
        return False
    changes: list[str] = []
    for record in observed:
        if not changes or changes[-1] != record["session_id_hash"]:
            changes.append(record["session_id_hash"])
    return (changes == [child_hash, parent_hash] if require_return
            else changes == [child_hash])


def _cleanup_tui(child_tui: PrivatePty | None) -> bool:
    if child_tui is None:
        return True
    try:
        reaped = bool(child_tui.child is not None and bootstrap._kill_group(child_tui.child))
        child_tui.close()
        return reaped
    except (OSError, RuntimeError):
        return False


def _finish_result(result: dict[str, Any], child_tui: PrivatePty | None,
                   context: dict[str, Any], identifiers: tuple[str, str],
                   initial_sequences: list[int], hashes: list[str],
                   project_hash: str) -> None:
    cleanup_ok = _cleanup_tui(child_tui)
    result["extra_tui_cleanup_completed"] = cleanup_ok
    if not cleanup_ok:
        result.update(classification="blocked", reason="child_tui_cleanup_unverified")
    elif result["classification"] == "proven":
        final_messages_empty = _empty_messages(context, identifiers)
        final_observers_fresh = _originals_advanced(
            context, initial_sequences, hashes, project_hash)
        result["user_messages_empty"] = final_messages_empty
        result["original_observers_unchanged"] = final_observers_fresh
        result["passive_identity_unchanged"] = (
            result["passive_identity_unchanged"] and final_observers_fresh)
        if not final_messages_empty or not final_observers_fresh:
            result.update(classification="not_proven",
                          reason="post_cleanup_acceptance_not_proven")


def _passive_fresh(path: Path, nonce: str, project_hash: str,
                   expected_hash: str, after_seq: int) -> tuple[bool, int]:
    records, rejected = _read_records(path, nonce, project_hash)
    return (not rejected and bool(records)
            and tui_probe.passive_history_matches(
                records, after_seq, expected_hash, project_hash),
            int(records[-1]["seq"]) if records else after_seq)


def _owned_request(context: dict[str, Any], method: str, path: str,
                   *, body: bytes | None = None, timeout: float = 1.0
                   ) -> tuple[int | None, bytes]:
    """Authenticate only while the exact fixture process owns its listener."""
    try:
        if not bootstrap.server_owns_port(int(context["port"]), int(context["server_pid"])):
            return None, b""
        return bootstrap.request(context["base"], method, path,
                                 password=context["password"], body=body, timeout=timeout)
    except (KeyError, TypeError, ValueError, OSError):
        return None, b""


def _empty_messages(context: dict[str, Any], identifiers: tuple[str, ...]) -> bool:
    if not 2 <= len(identifiers) <= 3 or len(set(identifiers)) != len(identifiers):
        return False
    for identifier in identifiers:
        if _identifier(identifier) is None:
            return False
        status, payload = _owned_request(
            context, "GET", f"/api/session/{quote(identifier, safe='')}/message?type=user&limit=1")
        if status != 200 or len(payload) > bootstrap.MAX_RESPONSE_BYTES:
            return False
        try:
            response = json.loads(payload)
        except (ValueError, UnicodeDecodeError):
            return False
        data = response.get("data") if isinstance(response, dict) else None
        if not isinstance(data, list) or data:
            return False
    return True


def _originals_advanced(context: dict[str, Any], start_sequences: list[int],
                        expected_hashes: list[str], project_hash: str) -> bool:
    paths = context["record_paths"]
    nonces = context["nonces"]
    if len(paths) != 2 or len(nonces) != 2:
        return False
    for index in range(2):
        records, rejected = _read_records(Path(paths[index]), nonces[index], project_hash)
        fresh = [row for row in records if row["seq"] > start_sequences[index]]
        if (rejected or not fresh or any(
                row["route_name"] != "session"
                or row["session_id_hash"] != expected_hashes[index]
                or row["session_top_level"] is not True
                or row["project_hash"] != project_hash for row in fresh)):
            return False
    return True


def after_routes(context: dict[str, Any]) -> dict[str, Any]:
    """Create and select one linked child, then return to its top-level parent."""
    started = time.monotonic()
    result: dict[str, Any] = {
        "classification": "not_proven",
        "reason": "child_route_case_not_completed",
        "fork_route_status": None,
        "fork_outcome": "not_attempted",
        "child_creation_path": "not_attempted",
        "child_selected": False,
        "child_not_top_level": False,
        "top_level_list_excludes_child": False,
        "passive_identity_unchanged": False,
        "original_observers_unchanged": False,
        "returned_to_parent": False,
        "user_messages_empty": False,
        "prompt_submissions": 0,
        "extra_tui_cleanup_completed": False,
        "elapsed_ms": 0.0,
    }
    child_tui: PrivatePty | None = None
    cleanup_ok = True
    try:
        required = ("root", "project", "base", "password", "port", "server_pid",
                    "executable", "launcher", "deadline", "session_ids", "tuis",
                    "record_paths", "nonces", "project_hash", "hashes", "env")
        if any(key not in context for key in required):
            result["reason"] = "fixture_scope_unavailable"
            return result
        root = Path(context["root"])
        project = Path(context["project"])
        ids = tuple(context["session_ids"])
        tuis = context["tuis"]
        record_paths = [Path(value) for value in context["record_paths"]]
        nonces = context["nonces"]
        hashes = context["hashes"]
        deadline = min(float(context["deadline"]), time.monotonic() + CASE_SECONDS)
        executable = Path(context["executable"])
        if (context.get("version") != PINNED_VERSION
                or len(ids) != 2 or len(tuis) != 2
                or len(record_paths) != 2 or len(nonces) != 2 or len(hashes) != 2
                or bootstrap.sha256_file(executable) != PINNED_SHA256
                or not project.is_dir() or root.stat().st_mode & 0o077
                or project.stat().st_mode & 0o077
                or not bootstrap.server_owns_port(int(context["port"]), int(context["server_pid"]))):
            result["reason"] = "fixture_scope_unverified"
            return result
        parent_id = ids[0]
        parent_hash = hashes[0]
        child_label = CHILD_TITLE
        if (not _identifier(parent_id) or parent_hash != hashlib.sha256(
                parent_id.encode("utf-8")).hexdigest()
                or not re.fullmatch(r"[0-9a-f]{64}", context["project_hash"])):
            result["reason"] = "fixture_identity_invalid"
            return result

        initial_sequences: list[int] = []
        for index in range(2):
            original, rejected = _read_records(record_paths[index], nonces[index],
                                               context["project_hash"])
            if (rejected or not original or not tui_probe.current_route(
                    original, hashes[index], context["project_hash"])):
                result["reason"] = "original_observers_not_fresh_at_hook_entry"
                return result
            initial_sequences.append(int(original[-1]["seq"]))

        # The fork route is the native 2.0.32 operation. Fresh fixture sessions
        # have no messages, so this records its empty-parent behavior first.
        fork_status, fork_body = _owned_request(
            context, "POST", f"/api/session/{quote(parent_id, safe='')}/fork",
            body=b"{}", timeout=1.0)
        result["fork_route_status"] = fork_status
        if fork_status == 200:
            fork_data = _data(fork_body)
            child_id = _child_metadata_matches(fork_data, parent_id=parent_id, project=project)
            if child_id is None:
                result["reason"] = "fork_response_scope_invalid"
                return result
            result["fork_outcome"] = "child_created"
            result["child_creation_path"] = "fork"
        else:
            if fork_status is None:
                result["reason"] = "fork_route_unreachable"
                return result
            result["fork_outcome"] = ("empty_parent_rejected_status_400" if fork_status == 400
                                      else "fork_route_rejected")
            create_body = json.dumps({"parentID": parent_id, "title": child_label},
                                     separators=(",", ":")).encode("utf-8")
            create_status, create_reply = _owned_request(
                context, "POST", "/api/session", body=create_body, timeout=1.0)
            created = _child_metadata_matches(_data(create_reply), parent_id=parent_id,
                                              project=project) if create_status == 200 else None
            if created is None:
                result["reason"] = "linked_child_create_not_proven"
                return result
            child_id = created
            result["child_creation_path"] = "parent_id"

        get_status, get_reply = _owned_request(
            context, "GET", f"/api/session/{quote(child_id, safe='')}", timeout=1.0)
        child_id = (_child_metadata_matches(_data(get_reply), parent_id=parent_id,
                                            project=project) if get_status == 200 else None)
        if child_id is None:
            result["reason"] = "child_metadata_not_proven"
            return result
        result["child_not_top_level"] = True

        top_query = ("/api/session?directory=" + quote(str(project), safe="")
                     + "&parentID=null&limit=20&archived=false")
        list_status, list_reply = _owned_request(context, "GET", top_query, timeout=1.0)
        result["top_level_list_http_status"] = list_status
        try:
            listing = json.loads(list_reply).get("data", [])
            result["top_level_list_row_count"] = len(listing) if isinstance(listing, list) else 0
            if isinstance(listing, list):
                result["top_level_list_expected_count"] = sum(
                    isinstance(row, dict) and row.get("id") in ids for row in listing)
                result["top_level_list_child_count"] = sum(
                    isinstance(row, dict) and row.get("id") == child_id for row in listing)
                result["top_level_list_nonroot_count"] = sum(
                    isinstance(row, dict) and row.get("parentID") is not None for row in listing)
        except (ValueError, UnicodeDecodeError, AttributeError):
            pass
        if list_status != 200 or not _top_level_excludes_child(
                list_reply, child_id=child_id, project=project,
                expected_parents=set(ids)):
            result["reason"] = "top_level_child_isolation_not_proven"
            return result
        result["top_level_list_excludes_child"] = True

        root_metadata = root.stat()
        config_dir = root / "opencode-config"
        plugin_dir = root / "identity-plugin"
        fixture_plugin = (Path(__file__).resolve().parent.parent / "docs" / "progress"
                          / "p4-v2-tui-plugin-fixture" / "plugin.mjs")
        record_path = root / "child-route.jsonl"
        control_path = root / "child-route.control"
        if (root_metadata.st_mode & 0o077 or not config_dir.is_dir()
                or config_dir.stat().st_mode & 0o077 or not plugin_dir.is_dir()
                or plugin_dir.stat().st_mode & 0o077 or not fixture_plugin.is_file()
                or record_path.exists() or control_path.exists()):
            result["reason"] = "private_child_tui_fixture_unavailable"
            return result
        record_path.touch(mode=0o600)
        record_path.chmod(0o600)
        nonce = os.urandom(16).hex()
        client_env = dict(context["env"])
        client_env.update({
            "TERM": "xterm-256color",
            "OPENCODE_CONFIG_DIR": str(config_dir),
            "PANELINK_PROBE_RECORDS": str(record_path),
            "PANELINK_PROBE_CLIENT_NONCE": nonce,
            "PANELINK_PROBE_CONTROL": str(control_path),
        })
        argv = bootstrap.launcher_argv(
            str(executable), context["launcher"],
            ["--server", context["base"], "--session", child_id, str(project)],
        )
        child_tui = PrivatePty(argv, project, client_env)
        child_hash = hashlib.sha256(child_id.encode("utf-8")).hexdigest()
        project_hash = context["project_hash"]

        def child_selected() -> bool:
            records, rejected = _read_records(record_path, nonce, project_hash)
            return (not rejected and bool(records)
                    and any(row["route_name"] == "session"
                            and row["session_id_hash"] == child_hash
                            and row["session_top_level"] is False
                            and row["project_hash"] == project_hash for row in records))

        child_ready_deadline = min(deadline, time.monotonic() + 5.0)
        if not tui_probe._wait_until(child_selected, [*tuis, child_tui], child_ready_deadline):
            result["reason"] = "child_tui_selected_route_not_observed"
            return result
        child_records, child_rejected = _read_records(record_path, nonce, project_hash)
        result["child_selected"] = (not child_rejected and _child_history_matches(
            child_records, child_hash=child_hash, parent_hash=parent_hash,
            project_hash=project_hash))
        if not result["child_selected"]:
            result["reason"] = "child_route_history_invalid"
            return result

        passive_index = 1
        passive_path = record_paths[passive_index]
        passive_nonce = nonces[passive_index]
        passive_start, rejected = _read_records(passive_path, passive_nonce, project_hash)
        passive_seq = int(passive_start[-1]["seq"]) if passive_start else 0
        if rejected or not passive_start or not tui_probe.current_route(
                passive_start, hashes[passive_index], project_hash):
            result["reason"] = "passive_initial_identity_not_fresh"
            return result

        if not tui_probe._send_control(child_tui, b"\x18l", deadline):
            result["reason"] = "child_picker_open_failed"
            return result
        if not tui_probe._wait_until(
                lambda: session_list_visible(tui_probe._picker_screen_text(
                    child_tui.screen_text()), (PARENT_TITLE,)), [*tuis, child_tui],
                min(deadline, time.monotonic() + 3.0)):
            result["reason"] = "parent_picker_row_not_observed"
            return result
        if not tui_probe._send_control(child_tui, PARENT_TITLE.encode("ascii"), deadline):
            result["reason"] = "parent_picker_filter_failed"
            return result
        if not tui_probe._wait_until(
                lambda: filtered_session_row_visible(
                    tui_probe._picker_screen_text(child_tui.screen_text()), PARENT_TITLE,
                    CHILD_TITLE), [*tuis, child_tui], min(deadline, time.monotonic() + 3.0)):
            result["reason"] = "parent_picker_filter_not_proven"
            return result
        if not tui_probe._send_control(child_tui, b"\r", deadline):
            result["reason"] = "parent_picker_select_failed"
            return result

        def returned() -> bool:
            nonlocal child_records
            child_records, rejected_child = _read_records(record_path, nonce, project_hash)
            passive_ok, passive_latest = _passive_fresh(
                passive_path, passive_nonce, project_hash, hashes[passive_index], passive_seq)
            result["passive_identity_unchanged"] = passive_ok and passive_latest > passive_seq
            result["original_observers_unchanged"] = _originals_advanced(
                context, initial_sequences, hashes, project_hash)
            return (not rejected_child and _child_history_matches(
                child_records, child_hash=child_hash, parent_hash=parent_hash,
                project_hash=project_hash, require_return=True)
                and result["passive_identity_unchanged"]
                and result["original_observers_unchanged"])

        returned_ok = tui_probe._wait_until(returned, [*tuis, child_tui], deadline)
        if not returned_ok:
            result["reason"] = "return_to_original_identity_not_observed"
            return result
        result["returned_to_parent"] = True
        result["user_messages_empty"] = _empty_messages(context, (parent_id, child_id))
        if not result["user_messages_empty"]:
            result["reason"] = "fixture_user_message_read_not_empty"
            return result
        result.update(classification="proven", reason="linked_child_selected_and_parent_restored")
        return result
    except (KeyError, OSError, RuntimeError, TypeError, ValueError):
        result.update(classification="blocked", reason="child_route_driver_failed")
        return result
    finally:
        if "initial_sequences" in locals() and "ids" in locals() and "child_id" in locals():
            _finish_result(result, child_tui, context, (ids[0], child_id),
                           initial_sequences, hashes, context["project_hash"])
        else:
            cleanup_ok = _cleanup_tui(child_tui)
            result["extra_tui_cleanup_completed"] = cleanup_ok
            if not cleanup_ok:
                result.update(classification="blocked", reason="child_tui_cleanup_unverified")
        result["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)
