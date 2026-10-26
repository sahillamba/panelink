#!/usr/bin/env python3
"""Bounded remaining OpenCode 2.0.32 identity acceptance fixtures.

This probe reuses the private pinned-server and TUI observers, then checks a
new API-created root selected in the TUI, a static imported transcript forked
without a model turn, and a same-client cross-project session switch. It never
uses a provider credential, submits a prompt, or permits non-loopback network.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any
from urllib.parse import quote

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts import probe_opencode_v2_tui_identity as tui_probe
    from scripts.probe_opencode_tui_identity import (
        filtered_session_row_visible,
        session_list_visible,
    )
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    import probe_opencode_v2_tui_identity as tui_probe
    from probe_opencode_tui_identity import filtered_session_row_visible, session_list_visible


EXPECTED_VERSION = bootstrap.EXPECTED_VERSION
EXPECTED_SHA256 = bootstrap.EXPECTED_SHA256
CASE_SECONDS = 18.0
MAX_EXPORT_BYTES = bootstrap.MAX_RESPONSE_BYTES
MAX_MESSAGES = 64
ROOT_TITLE = "Panelink V2 Newly Created Root"
SEED_TITLE = "Panelink V2 Import Seed"
PARENT_TITLE = "Panelink V2 Imported Parent"
FORK_TITLE = "Panelink V2 Imported Fork"
OTHER_PROJECT_TITLE = "Panelink V2 Other Project Session"
SEED_TEXT = "Panelink private identity fixture transcript seed; not a prompt."


def _data(payload: bytes) -> dict[str, Any] | None:
    if len(payload) > MAX_EXPORT_BYTES:
        return None
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return None
    data = value.get("data") if isinstance(value, dict) else None
    return data if isinstance(data, dict) else None


def _identifier(value: Any) -> str | None:
    if (not isinstance(value, str) or not 1 <= len(value) <= 128
            or not all(char.isascii() and (char.isalnum() or char in "_.-") for char in value)):
        return None
    return value


def _owned(context: dict[str, Any]) -> bool:
    try:
        return (context["server_pid"] > 0
                and bootstrap.server_owns_port(context["port"], context["server_pid"]))
    except (KeyError, TypeError, ValueError):
        return False


def _request(context: dict[str, Any], method: str, path: str,
             body: dict[str, Any] | None = None,
             timeout: float = 1.0) -> tuple[int | None, bytes]:
    if not _owned(context):
        return None, b""
    try:
        encoded = None if body is None else json.dumps(
            body, separators=(",", ":"), ensure_ascii=True).encode("utf-8")
        return bootstrap.request(context["base"], method, path,
                                 password=context["password"], body=encoded,
                                 timeout=timeout)
    except (KeyError, TypeError, ValueError, OSError):
        return None, b""


def _session_id(payload: bytes, *, top_level: bool | None = None) -> str | None:
    data = _data(payload)
    if data is None:
        return None
    identifier = _identifier(data.get("id"))
    parent_id = data.get("parentID", data.get("parentId"))
    if identifier is None:
        return None
    if top_level is True and parent_id is not None:
        return None
    if top_level is False and not isinstance(parent_id, str):
        return None
    return identifier


def _session_row_matches(row: Any, identifier: str, project: Path,
                         *, top_level: bool) -> bool:
    if not isinstance(row, dict) or row.get("id") != identifier:
        return False
    parent_id = row.get("parentID", row.get("parentId"))
    if (top_level and parent_id is not None) or (not top_level and not isinstance(parent_id, str)):
        return False
    location = row.get("location")
    directory = location.get("directory") if isinstance(location, dict) else None
    if not isinstance(directory, str) or len(directory) > 4096:
        return False
    try:
        return Path(directory).resolve(strict=True) == project.resolve(strict=True)
    except (OSError, ValueError):
        return False


def _list_sessions(context: dict[str, Any], project: Path,
                   *, top_level: bool) -> list[dict[str, Any]] | None:
    query = ("/api/session?directory=" + quote(str(project), safe="")
             + ("&parentID=null" if top_level else "")
             + "&limit=20&archived=false")
    status, payload = _request(context, "GET", query)
    if status != 200 or len(payload) > MAX_EXPORT_BYTES:
        return None
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return None
    rows = value.get("data") if isinstance(value, dict) else None
    if not isinstance(rows, list) or len(rows) > 20 or any(not isinstance(row, dict) for row in rows):
        return None
    return rows


def _lookup(context: dict[str, Any], identifier: str, project: Path,
            *, top_level: bool) -> bool:
    rows = _list_sessions(context, project, top_level=top_level)
    return rows is not None and sum(
        _session_row_matches(row, identifier, project, top_level=top_level)
        for row in rows) == 1


def _export(context: dict[str, Any], identifier: str) -> dict[str, Any] | None:
    status, payload = _request(
        context, "GET",
        f"/api/experimental/session/{quote(identifier, safe='')}/export?sanitize=false")
    data = _data(payload) if status == 200 else None
    if data is None:
        return None
    info, messages = data.get("info"), data.get("messages")
    if (not isinstance(info, dict) or info.get("id") != identifier
            or not isinstance(messages, list) or len(messages) > MAX_MESSAGES):
        return None
    return data


def _empty_user_messages(context: dict[str, Any], identifier: str) -> tuple[bool, int | None]:
    if _identifier(identifier) is None:
        return False, None
    status, payload = _request(
        context, "GET",
        f"/api/session/{quote(identifier, safe='')}/message?type=user&limit=1")
    if status != 200 or len(payload) > MAX_EXPORT_BYTES:
        return False, status
    try:
        value = json.loads(payload)
    except (ValueError, UnicodeDecodeError):
        return False, status
    data = value.get("data") if isinstance(value, dict) else None
    return isinstance(data, list) and data == [], status


def _messages_digest(messages: Any) -> str | None:
    if not isinstance(messages, list) or not 1 <= len(messages) <= MAX_MESSAGES:
        return None
    try:
        canonical = json.dumps(messages, sort_keys=True, separators=(",", ":"),
                               ensure_ascii=True).encode("utf-8")
    except (TypeError, ValueError):
        return None
    if len(canonical) > MAX_EXPORT_BYTES:
        return None
    return hashlib.sha256(canonical).hexdigest()


def _fork_history_digest(messages: Any) -> str | None:
    """Hash exact projected message payloads while excluding fork-remapped IDs."""
    if (not isinstance(messages, list) or not 1 <= len(messages) <= MAX_MESSAGES
            or any(not isinstance(message, dict) or _identifier(message.get("id")) is None
                   for message in messages)):
        return None
    payload = [{key: value for key, value in message.items() if key != "id"}
               for message in messages]
    try:
        canonical = json.dumps(payload, sort_keys=True, separators=(",", ":"),
                               ensure_ascii=True).encode("utf-8")
    except (TypeError, ValueError):
        return None
    if len(canonical) > MAX_EXPORT_BYTES:
        return None
    return hashlib.sha256(canonical).hexdigest()


def _read_records(path: Path, nonce: str, project_hash: str
                  ) -> tuple[list[dict[str, Any]], int]:
    try:
        with path.open("rb") as stream:
            payload = stream.read(tui_probe.MAX_RECORD_BYTES + 1)
    except OSError:
        return [], 1
    return tui_probe.parse_client_records(payload, client_nonce=nonce,
                                          project_hash=project_hash,
                                          enforce_gap_bound=False)


def _read_transition_records(path: Path, nonce: str,
                             allowed_project_hashes: set[str]
                             ) -> tuple[list[dict[str, Any]], list[str], int]:
    """Validate per-project segments while preserving global order/instance identity."""
    try:
        payload = path.read_bytes()
    except OSError:
        return [], [], 1
    if len(payload) >= tui_probe.MAX_RECORD_BYTES:
        return [], [], 1
    lines = payload.splitlines()
    if not lines or len(lines) >= tui_probe.MAX_RECORDS:
        return [], [], 1
    parsed: list[dict[str, Any]] = []
    instances: list[str] = []
    segment: list[dict[str, Any]] = []
    segment_project: str | None = None
    identified_project = False
    parser_project = sorted(allowed_project_hashes)[0]
    previous_seq = 0
    previous_ms = -1.0

    def flush_segment() -> bool:
        nonlocal segment, segment_project, parsed
        if not segment:
            return False
        normalized = []
        for seq, item in enumerate(segment, start=1):
            normalized.append({**item, "seq": seq})
        normalized_bytes = b"\n".join(
            json.dumps(item, separators=(",", ":")).encode("utf-8")
            for item in normalized)
        records, rejected = tui_probe.parse_client_records(
            normalized_bytes, client_nonce=nonce,
            project_hash=segment_project or parser_project,
            enforce_gap_bound=False)
        if rejected or len(records) != len(segment):
            return False
        for raw_item, parsed_item in zip(segment, records):
            parsed.append({**parsed_item, "seq": raw_item["seq"]})
        segment = []
        segment_project = None
        return True

    for raw in lines:
        try:
            item = json.loads(raw)
        except (ValueError, UnicodeDecodeError):
            return [], [], 1
        if not isinstance(item, dict):
            return [], [], 1
        seq = item.get("seq")
        observed_ms = item.get("monotonic_ms")
        project = item.get("project_hash")
        instance = item.get("tui_instance")
        if (not isinstance(seq, int) or isinstance(seq, bool) or seq != previous_seq + 1
                or not isinstance(observed_ms, (int, float)) or isinstance(observed_ms, bool)
                or observed_ms <= previous_ms
                or item.get("client_nonce") != nonce
                or (project is not None and
                    (not isinstance(project, str) or project not in allowed_project_hashes))
                or (project is None and identified_project)
                or not isinstance(instance, str) or len(instance) != 32
                or any(char not in "0123456789abcdef" for char in instance)):
            return [], [], 1
        if segment and project != segment_project:
            if not flush_segment():
                return [], [], 1
        segment_project = project
        segment.append(item)
        identified_project = identified_project or project is not None
        instances.append(instance)
        previous_seq = seq
        previous_ms = float(observed_ms)
    if not flush_segment() or len(parsed) != len(lines):
        return [], [], 1
    return parsed, instances, 0


def _instance_ids(path: Path, nonce: str, allowed_project_hashes: set[str]) -> list[str]:
    records, instances, rejected = _read_transition_records(
        path, nonce, allowed_project_hashes)
    return instances if records and not rejected and len(set(instances)) == 1 else []


def _passive_watermark(context: dict[str, Any]) -> int | None:
    records, rejected = _read_records(
        Path(context["record_paths"][1]), context["nonces"][1], context["project_hash"])
    initial = next((record for record in records
                    if record["route_name"] == "session"
                    and record["session_id_hash"] == context["hashes"][1]
                    and record["session_top_level"] is True
                    and record["project_hash"] == context["project_hash"]), None)
    if (rejected or not records or initial is None
            or not tui_probe.passive_history_matches(
                records, int(initial["seq"]), context["hashes"][1], context["project_hash"])):
        return None
    context["passive_initial_seq"] = int(initial["seq"])
    return int(records[-1]["seq"])


def _root_listing_contains_fork(context: dict[str, Any], project: Path,
                                child_id: str, expected_root_ids: set[str]) -> bool:
    rows = _list_sessions(context, project, top_level=True)
    if rows is None:
        return False
    ids = [row.get("id") for row in rows]
    return (len(ids) == len(set(ids)) and child_id in ids
            and expected_root_ids.issubset(set(ids))
            and all(isinstance(row.get("id"), str)
                    and _session_row_matches(row, row["id"], project, top_level=True)
                    for row in rows))


def _last(records: list[dict[str, Any]], *, project_hash: str,
          route: str = "session", session_hash: str | None = None,
          top_level: bool | None = True) -> bool:
    if not records:
        return False
    latest = records[-1]
    return (latest["route_name"] == route
            and latest["project_hash"] == project_hash
            and (session_hash is None or latest["session_id_hash"] == session_hash)
            and latest["session_top_level"] is top_level)


def _fresh_tail(records: list[dict[str, Any]], watermark: int,
                *, project_hash: str, session_hash: str,
                top_level: bool | None) -> bool:
    tail = [record for record in records if int(record["seq"]) > watermark]
    return bool(tail) and all(
        record["route_name"] == "session"
        and record["project_hash"] == project_hash
        and record["session_id_hash"] == session_hash
        and record["session_top_level"] is top_level for record in tail)


def _passive_fresh(context: dict[str, Any], watermark: int) -> bool:
    records, rejected = _read_records(
        Path(context["record_paths"][1]), context["nonces"][1], context["project_hash"])
    initial_seq = context.get("passive_initial_seq")
    if not isinstance(initial_seq, int) and records:
        initial = next((record for record in records
                        if record["route_name"] == "session"
                        and record["session_id_hash"] == context["hashes"][1]
                        and record["session_top_level"] is True
                        and record["project_hash"] == context["project_hash"]), None)
        initial_seq = int(initial["seq"]) if initial else None
    return (rejected == 0 and len(records) > 0
            and isinstance(initial_seq, int)
            and int(records[-1]["seq"]) > watermark
            and tui_probe.passive_history_matches(
                records, initial_seq, context["hashes"][1], context["project_hash"]))


def _wait_route(context: dict[str, Any], project_hash: str,
                session_hash: str | None, *, top_level: bool | None,
                after_seq: int, allowed_project_hashes: set[str] | None = None,
                seconds: float = 5.0) -> tuple[bool, int]:
    tui = context["tuis"][0]
    deadline = min(context["deadline"], time.monotonic() + seconds)
    while time.monotonic() < deadline:
        tui_probe._tick(context["tuis"], min(deadline, time.monotonic() + 0.1))
        if allowed_project_hashes is not None:
            records, _instances, rejected = _read_transition_records(
                Path(context["record_paths"][0]), context["nonces"][0],
                allowed_project_hashes)
        else:
            records, rejected = _read_records(
                Path(context["record_paths"][0]), context["nonces"][0], project_hash)
        if rejected:
            # Preserve only redacted, boolean diagnostics for the most recent
            # raw observer entry; IDs, paths and transcript fields stay out of
            # the report. This separates an unknown project hash from a bad
            # record schema or a route that never changed.
            last = None
            try:
                for raw in Path(context["record_paths"][0]).read_bytes().splitlines()[-1:]:
                    parsed = json.loads(raw)
                    if isinstance(parsed, dict):
                        last = parsed
            except (OSError, ValueError, UnicodeDecodeError):
                pass
            context["_target_route_diagnostic"] = {
                "records": len(records), "rejected": rejected,
                "last_route_name": last.get("route_name") if last else None,
                "last_project_allowed": (last.get("project_hash") in allowed_project_hashes
                                         if last and isinstance(last.get("project_hash"), str)
                                         else False),
                "last_project_matches_target": (last.get("project_hash") == project_hash
                                                 if last else False),
                "last_session_matches_target": (last.get("session_id_hash") == session_hash
                                                 if last else False),
                "last_seq_valid": (isinstance(last.get("seq"), int)
                                    and not isinstance(last.get("seq"), bool)
                                    if last else False),
            }
            return False, 0
        if (records and int(records[-1]["seq"]) > after_seq
                and _last(records, project_hash=project_hash,
                          session_hash=session_hash, top_level=top_level)):
            return True, int(records[-1]["seq"])
        if records:
            latest = records[-1]
            context["_target_route_diagnostic"] = {
                "records": len(records), "route_name": latest["route_name"],
                "session_matches": latest["session_id_hash"] == session_hash,
                "project_matches": latest["project_hash"] == project_hash,
                "top_level_matches": latest["session_top_level"] is top_level,
                "sequence_advanced": int(latest["seq"]) > after_seq,
            }
    return False, 0


def _wait_current_route(context: dict[str, Any], project_hash: str,
                        session_hash: str, *, top_level: bool,
                        seconds: float = 3.0) -> list[dict[str, Any]] | None:
    tui = context["tuis"][0]
    deadline = min(context["deadline"], time.monotonic() + seconds)
    latest: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        tui_probe._tick(context["tuis"], min(deadline, time.monotonic() + 0.1))
        latest, _instances, rejected = _read_transition_records(
            Path(context["record_paths"][0]), context["nonces"][0], {project_hash})
        if rejected:
            context["_route_wait_diagnostic"] = {"records": len(latest), "rejected": rejected}
            return None
        if _last(latest, project_hash=project_hash,
                 session_hash=session_hash, top_level=top_level):
            return latest
        if latest:
            current = latest[-1]
            context["_route_wait_diagnostic"] = {
                "records": len(latest), "route_name": current["route_name"],
                "session_matches": current["session_id_hash"] == session_hash,
                "project_matches": current["project_hash"] == project_hash,
                "top_level_matches": current["session_top_level"] is top_level,
            }
        else:
            context["_route_wait_diagnostic"] = {"records": 0, "rejected": 0}
    return None


def _menu_select(context: dict[str, Any], target: str, other: str | None,
                 *, before_project_hash: str, before_session_hash: str,
                 target_project_hash: str, target_session_hash: str,
                 target_top_level: bool | None = True,
                 before_top_level: bool | None = True) -> tuple[bool, int]:
    context["_menu_failure_stage"] = "before_route_unverified"
    tui = context["tuis"][0]
    records, _instances, rejected = _read_transition_records(
        Path(context["record_paths"][0]), context["nonces"][0],
        {before_project_hash, target_project_hash})
    if rejected or not _last(records, project_hash=before_project_hash,
                             session_hash=before_session_hash, top_level=before_top_level):
        return False, 0
    marker_seq = int(records[-1]["seq"])
    deadline = min(context["deadline"], time.monotonic() + CASE_SECONDS)
    context["_menu_failure_stage"] = "open_control_write_failed"
    if not tui_probe._send_control(tui, b"\x0f", deadline):
        return False, 0

    def exact_row_ready() -> bool:
        screen = tui.screen_text()
        lowered = screen.lower()
        heading = ("Open session or project" in screen
                   or "recent sessions" in lowered or "sessions" in lowered)
        passive = _passive_fresh(context, context["passive_watermark"])
        context["_menu_screen_flags"] = {
            "open_label": "Open session or project" in screen,
            "recent_label": "recent sessions" in lowered,
            "sessions_label": "sessions" in lowered,
            "passive_fresh": passive,
        }
        return heading and passive

    context["_menu_failure_stage"] = "exact_menu_rows_not_visible"
    if not tui_probe._wait_until(exact_row_ready, context["tuis"],
                                 min(deadline, time.monotonic() + 4.0)):
        context["_menu_failure_flags"] = context.get("_menu_screen_flags", {})
        return False, 0
    # Filter only after the menu is open; Enter is withheld until the exact
    # fixture title is visible and a competing fixture title is absent.
    context["_menu_failure_stage"] = "filter_control_write_failed"
    if not tui_probe._send_control(tui, target.encode("ascii"), deadline):
        return False, 0

    def filtered_row_ready() -> bool:
        screen = tui.screen_text()
        lowered = screen.lower()
        context["_menu_screen_flags"] = {
            "open_label": "Open session or project" in screen,
            "recent_label": "recent sessions" in lowered,
            "sessions_label": "sessions" in lowered,
            "target_visible": target in screen,
            "other_visible": other is not None and other in screen,
        }
        if not ("Open session or project" in screen
                or "recent sessions" in lowered or "sessions" in lowered):
            # Some terminal layouts render a shorter accessible title; the
            # known fixture row still must be unique in the dialog region.
            if "Sessions" not in screen:
                return False
        return (target in screen and (other is None or other not in screen)
                and _passive_fresh(context, context["passive_watermark"]))

    context["_menu_failure_stage"] = "filtered_exact_row_not_visible"
    if not tui_probe._wait_until(filtered_row_ready, context["tuis"],
                                 min(deadline, time.monotonic() + 4.0)):
        context["_menu_failure_flags"] = context.get("_menu_screen_flags", {})
        return False, 0
    context["_menu_failure_stage"] = "enter_write_failed"
    if not tui_probe._send_control(tui, b"\r", deadline):
        return False, 0
    context["_menu_failure_stage"] = "passive_observer_stale_after_selection"
    if not _passive_fresh(context, context["passive_watermark"]):
        return False, 0
    context["_menu_failure_stage"] = "target_route_not_observed"
    ok, seq = _wait_route(context, target_project_hash, target_session_hash,
                          top_level=target_top_level, after_seq=marker_seq,
                          allowed_project_hashes={before_project_hash, target_project_hash})
    if not ok or not _passive_fresh(context, context["passive_watermark"]):
        return False, 0
    context["_menu_failure_stage"] = ""
    return True, seq


def _create_root(context: dict[str, Any], title: str, project: Path) -> str | None:
    status, payload = _request(context, "POST", "/api/session", {
        "title": title, "location": {"directory": str(project)},
    })
    identifier = _session_id(payload, top_level=True) if status == 200 else None
    if identifier is None or not _lookup(context, identifier, project, top_level=True):
        return None
    return identifier


def _imported_fork(context: dict[str, Any], project: Path) -> dict[str, Any]:
    result: dict[str, Any] = {
        "classification": "not_proven", "reason": "not_started",
        "import_route_status": None, "fork_route_status": None,
        "seed_export_received": False,
        "static_message_schema": "pinned_bundle_Session.Message.Synthetic",
        "transcript_nonempty": False, "transcript_preserved_by_import": False,
        "imported_message_count": 0, "imported_message_fields": [],
        "imported_message_schema_fields_match": False,
        "imported_field_matches": {},
        "fork_export_received": False, "fork_message_count": 0,
        "fork_message_fields": [], "fork_message_schema_fields_match": False,
        "fork_field_matches": {},
        "fork_message_ids_remapped": False,
        "full_history_fork_equal": False, "parent_link_verified": False,
        "fork_response_has_identity": False, "fork_parent_lookup_status": None,
        "fork_title_update_status": None,
        "fork_child_listed_top_level": False, "child_selected": False,
        "parent_returned": False, "provider_actions": 0,
    }
    source = _create_root(context, SEED_TITLE, project)
    if source is None:
        result["reason"] = "seed_root_create_or_scope_failed"
        return result
    exported = _export(context, source)
    result["seed_export_received"] = exported is not None
    if exported is None or not isinstance(exported.get("messages"), list):
        result["reason"] = "empty_seed_export_template_unavailable"
        return result
    info = exported.get("info")
    if not isinstance(info, dict) or info.get("id") != source:
        result["reason"] = "seed_export_identity_mismatch"
        return result
    info = dict(info)
    info["title"] = PARENT_TITLE
    # Construct exactly the pinned bundle's Session.Message.Synthetic union
    # member (id/type/text/description/time.created), rather than posting an
    # inbox item that will not appear in a projected export until a model turn.
    # The import is the only write; the immediately exported transcript must
    # round-trip to the identical canonical message hash before fork is tried.
    static_message = {
        "id": "msg_" + secrets.token_hex(16),
        "type": "synthetic",
        "text": SEED_TEXT,
        "description": "static fixture transcript seed",
        "time": {"created": int(time.time() * 1000)},
    }
    transcript = [static_message]
    transcript_hash = _messages_digest(transcript)
    if transcript_hash is None:
        result["reason"] = "static_transcript_schema_or_bounds_invalid"
        return result
    result["transcript_nonempty"] = True
    # Importing an existing ID is a conflict. Delete only this private seed,
    # require absence, then import the exact ID and exported message records.
    delete_status, _delete_body = _request(
        context, "DELETE", f"/api/session/{quote(source, safe='')}")
    result["seed_delete_status"] = delete_status
    if delete_status not in (200, 204):
        result["reason"] = "seed_delete_for_import_failed"
        return result
    missing_status, _missing_body = _request(
        context, "GET", f"/api/session/{quote(source, safe='')}")
    if missing_status != 404:
        result["reason"] = "seed_id_not_absent_before_import"
        return result
    import_status, import_payload = _request(
        context, "POST", "/api/experimental/session/import",
        {"info": info, "messages": transcript,
         "location": {"directory": str(project)}})
    result["import_route_status"] = import_status
    imported_id = _session_id(import_payload, top_level=True) if import_status == 200 else None
    if imported_id != source or not _lookup(context, source, project, top_level=True):
        result["reason"] = "experimental_import_not_supported_or_scope_failed"
        return result
    imported = _export(context, imported_id)
    imported_messages = imported.get("messages") if imported else None
    imported_hash = _messages_digest(imported_messages)
    result["imported_message_count"] = (
        len(imported_messages) if isinstance(imported_messages, list) else 0)
    imported_message = (imported_messages[0] if isinstance(imported_messages, list)
                        and len(imported_messages) == 1
                        and isinstance(imported_messages[0], dict) else None)
    if imported_message is not None:
        result["imported_message_fields"] = sorted(imported_message.keys())
        result["imported_field_matches"] = {
            key: imported_message.get(key) == static_message.get(key)
            for key in ("id", "type", "text", "description", "time")}
        result["imported_message_schema_fields_match"] = all(
            result["imported_field_matches"].values())
    result["transcript_preserved_by_import"] = imported_hash == transcript_hash
    if not result["transcript_preserved_by_import"]:
        result["reason"] = "imported_transcript_mismatch"
        return result

    fork_status, fork_payload = _request(
        context, "POST", f"/api/session/{quote(imported_id, safe='')}/fork", {})
    result["fork_route_status"] = fork_status
    child_id = _session_id(fork_payload, top_level=True) if fork_status == 200 else None
    child_data = _data(fork_payload) if fork_status == 200 else None
    result["fork_response_has_identity"] = child_id is not None
    if child_id is None or child_id == imported_id:
        result["reason"] = "full_history_fork_or_parent_link_failed"
        return result
    lookup_status, lookup_payload = _request(
        context, "GET", f"/api/session/{quote(child_id, safe='')}")
    result["fork_parent_lookup_status"] = lookup_status
    child_info = _data(lookup_payload) if lookup_status == 200 else None
    fork_info = child_info.get("fork") if isinstance(child_info, dict) else None
    fork_parent = fork_info.get("sessionID") if isinstance(fork_info, dict) else None
    boundary = fork_info.get("boundary") if isinstance(fork_info, dict) else None
    last_message_id = transcript[-1].get("id")
    if (fork_parent != imported_id or not isinstance(boundary, dict)
            or boundary.get("type") != "through"
            or boundary.get("messageID") != last_message_id):
        result["reason"] = "fork_parent_link_not_verified"
        return result
    if not _lookup(context, child_id, project, top_level=True):
        result["reason"] = "fork_child_scope_not_verified"
        return result
    expected_roots = set(context["session_ids"]) | {context["new_root_id"], imported_id}
    if not _root_listing_contains_fork(context, project, child_id, expected_roots):
        result["reason"] = "fork_child_root_listing_scope_failed"
        return result
    result["parent_link_verified"] = True
    result["fork_child_listed_top_level"] = True
    child_export = _export(context, child_id)
    child_messages = child_export.get("messages") if child_export else None
    result["fork_export_received"] = child_export is not None
    result["fork_message_count"] = len(child_messages) if isinstance(child_messages, list) else 0
    fork_message = (child_messages[0] if isinstance(child_messages, list)
                    and len(child_messages) == 1
                    and isinstance(child_messages[0], dict) else None)
    if fork_message is not None:
        result["fork_message_fields"] = sorted(fork_message.keys())
        result["fork_field_matches"] = {
            key: fork_message.get(key) == static_message.get(key)
            for key in ("type", "text", "description", "time")}
        result["fork_message_schema_fields_match"] = all(
            result["fork_field_matches"].values())
    source_ids = [message.get("id") for message in transcript]
    fork_ids = ([message.get("id") for message in child_messages]
                if isinstance(child_messages, list) else [])
    result["fork_message_ids_remapped"] = (
        len(fork_ids) == len(source_ids) and len(set(fork_ids)) == len(fork_ids)
        and set(fork_ids).isdisjoint(source_ids))
    child_hash = _fork_history_digest(child_messages)
    expected_fork_hash = _fork_history_digest(transcript)
    result["full_history_fork_equal"] = (
        child_hash is not None and child_hash == expected_fork_hash
        and result["fork_message_ids_remapped"])
    if not result["full_history_fork_equal"]:
        result["reason"] = "fork_history_not_exact"
        return result
    title_status, _title_payload = _request(
        context, "PATCH", f"/api/session/{quote(child_id, safe='')}",
        {"title": FORK_TITLE})
    result["fork_title_update_status"] = title_status
    if title_status not in (200, 204):
        result["reason"] = "fork_child_unique_title_failed"
        return result

    new_root_id = _identifier(context.get("new_root_id"))
    if new_root_id is None:
        result["reason"] = "new_root_identity_unavailable_before_fork_selection"
        return result
    active_records = _wait_current_route(
        context, context["project_hash"], hashlib.sha256(new_root_id.encode()).hexdigest(),
        top_level=True)
    if active_records is None:
        result["reason"] = "active_tui_not_fresh_before_fork_selection"
        return result
    watermark = _passive_watermark(context)
    if watermark is None:
        result["reason"] = "passive_observer_not_fresh_before_picker"
        return result
    context["passive_watermark"] = watermark

    # Select the imported parent first, then the fork child, then return to the
    # exact parent in the same TUI instance. Each Enter follows an exact row.
    for expected, other, top_level, key in (
            (imported_id, FORK_TITLE, True, "parent_selected"),
            (child_id, PARENT_TITLE, True, "child_selected"),
            (imported_id, FORK_TITLE, True, "parent_returned")):
        expected_hash = hashlib.sha256(expected.encode()).hexdigest()
        records, rejected = _read_records(
            Path(context["record_paths"][0]), context["nonces"][0], context["project_hash"])
        if rejected or not records:
            result["reason"] = "invalid_active_route_records"
            return result
        current_hash = records[-1]["session_id_hash"]
        current_top_level = records[-1]["session_top_level"]
        if (current_top_level not in (True, False)
                or not _last(records, project_hash=context["project_hash"],
                             session_hash=current_hash, top_level=current_top_level)):
            result["reason"] = "active_route_not_top_level_before_picker"
            return result
        if not tui_probe._send_control(context["tuis"][0], b"\x18l", context["deadline"]):
            result["reason"] = "session_picker_open_failed"
            return result
        if not tui_probe._wait_until(
                lambda: session_list_visible(tui_probe._picker_screen_text(
                    context["tuis"][0].screen_text()), (PARENT_TITLE, FORK_TITLE))
                and _passive_fresh(context, context["passive_watermark"]),
                context["tuis"], min(context["deadline"], time.monotonic() + 4.0)):
            result["reason"] = "exact_parent_and_fork_rows_not_visible"
            return result
        if not tui_probe._send_control(
                context["tuis"][0],
                (PARENT_TITLE if expected == imported_id else FORK_TITLE).encode("ascii"),
                context["deadline"]):
            result["reason"] = "session_picker_filter_failed"
            return result
        target_label = PARENT_TITLE if expected == imported_id else FORK_TITLE
        other_label = FORK_TITLE if expected == imported_id else PARENT_TITLE
        if not tui_probe._wait_until(
                lambda: filtered_session_row_visible(
                    tui_probe._picker_screen_text(context["tuis"][0].screen_text()),
                    target_label, other_label)
                and _passive_fresh(context, context["passive_watermark"]),
                context["tuis"], min(context["deadline"], time.monotonic() + 4.0)):
            result["reason"] = "filtered_exact_session_row_not_visible"
            return result
        marker_seq = int(records[-1]["seq"])
        if not tui_probe._send_control(context["tuis"][0], b"\r", context["deadline"]):
            result["reason"] = "session_picker_select_failed"
            return result
        ok, _seq = _wait_route(
            context, context["project_hash"], expected_hash,
            top_level=top_level, after_seq=marker_seq,
            allowed_project_hashes={context["project_hash"]})
        if not ok or not _passive_fresh(context, context["passive_watermark"]):
            result["reason"] = "fork_parent_child_route_not_observed"
            return result
        result[key] = True
    result["classification"] = "proven"
    result["reason"] = "imported_nonempty_transcript_forked_and_selected"
    result["transcript_sha256"] = expected_fork_hash
    result["transcript_message_count"] = len(transcript)
    result["provider_actions"] = 0
    return result


def _new_root_case(context: dict[str, Any], project: Path) -> dict[str, Any]:
    result: dict[str, Any] = {
        "classification": "not_proven", "reason": "not_started",
        "api_created": False, "root_scope_verified": False,
        "same_tui_selected": False, "prompt_submissions": 0,
    }
    identifier = _create_root(context, ROOT_TITLE, project)
    if identifier is None:
        result["reason"] = "new_root_create_or_scope_failed"
        return result
    result["api_created"] = True
    result["root_scope_verified"] = True
    records = _wait_current_route(
        context, context["project_hash"], context["hashes"][0], top_level=True)
    if records is None:
        result["reason"] = "active_tui_not_fresh_before_new_root_selection"
        result["route_wait_diagnostic"] = context.get("_route_wait_diagnostic", {})
        return result
    watermark = _passive_watermark(context)
    if watermark is None:
        result["reason"] = "passive_observer_not_fresh_before_new_root_picker"
        return result
    context["passive_watermark"] = watermark
    ok, _seq = _menu_select(
        context, ROOT_TITLE, None,
        before_project_hash=context["project_hash"],
        before_session_hash=context["hashes"][0],
        target_project_hash=context["project_hash"],
        target_session_hash=hashlib.sha256(identifier.encode()).hexdigest(),
        target_top_level=True)
    if not ok:
        result["reason"] = "new_root_exact_row_or_selected_identity_failed"
        result["failure_stage"] = context.get("_menu_failure_stage", "unknown")
        result["failure_flags"] = context.get("_menu_failure_flags", {})
        return result
    result["same_tui_selected"] = True
    empty, user_message_status = _empty_user_messages(context, identifier)
    result["empty_root_user_messages"] = empty
    result["empty_root_read_status"] = user_message_status
    if not empty:
        result["reason"] = "new_root_user_message_absence_not_verified"
        return result
    result.update(classification="proven", reason="new_api_root_selected_in_same_tui",
                  identity_hash=hashlib.sha256(identifier.encode()).hexdigest())
    return result


def _project_switch_case(context: dict[str, Any], project: Path) -> dict[str, Any]:
    result: dict[str, Any] = {
        "classification": "not_proven", "reason": "not_started",
        "second_project_created": False, "foreign_session_verified": False,
        "switch_to_other_project": False, "switch_back_to_original": False,
        "same_tui_instance": False, "passive_observer_fresh": False,
        "prompt_submissions": 0,
    }
    root: Path = context["root"]
    second = root / "same-client-project"
    try:
        second.mkdir(mode=0o700)
        if second.stat().st_mode & 0o077:
            result["reason"] = "private_second_project_permissions_failed"
            return result
    except OSError:
        result["reason"] = "private_second_project_create_failed"
        return result
    result["second_project_created"] = True
    foreign_id = _create_root(context, OTHER_PROJECT_TITLE, second)
    if foreign_id is None:
        result["reason"] = "foreign_project_root_scope_failed"
        return result
    result["foreign_session_verified"] = True
    original_hash = context["project_hash"]
    second_hash = tui_probe.hash_value(str(second.resolve(strict=True)))
    records, rejected = _read_records(
        Path(context["record_paths"][0]), context["nonces"][0], original_hash)
    if rejected or not records:
        result["reason"] = "active_observer_invalid_before_project_switch"
        return result
    current_hash = records[-1]["session_id_hash"]
    if (records[-1]["route_name"] != "session"
            or records[-1]["session_top_level"] is not True
            or records[-1]["project_hash"] != original_hash):
        result["reason"] = "active_route_not_ready_for_project_switch"
        return result
    initial_instances = _instance_ids(
        Path(context["record_paths"][0]), context["nonces"][0], {original_hash})
    if not initial_instances or len(set(initial_instances)) != 1:
        result["reason"] = "active_tui_instance_missing"
        return result
    initial_instance = initial_instances[-1]
    watermark = _passive_watermark(context)
    if watermark is None:
        result["reason"] = "passive_observer_not_fresh_before_project_switch"
        return result
    context["passive_watermark"] = watermark
    ok, seq = _menu_select(
        context, OTHER_PROJECT_TITLE, None,
        before_project_hash=original_hash, before_session_hash=current_hash,
        target_project_hash=second_hash,
        target_session_hash=hashlib.sha256(foreign_id.encode()).hexdigest(),
        target_top_level=True)
    if not ok:
        result["reason"] = "same_client_project_switch_not_observed"
        result["failure_stage"] = context.get("_menu_failure_stage", "unknown")
        result["failure_flags"] = context.get("_menu_failure_flags", {})
        result["route_diagnostic"] = context.get("_target_route_diagnostic", {})
        return result
    result["switch_to_other_project"] = True
    result["same_tui_instance"] = True
    records_b, _instances_b, rejected_b = _read_transition_records(
        Path(context["record_paths"][0]), context["nonces"][0],
        {original_hash, second_hash})
    switched_instances = _instance_ids(
        Path(context["record_paths"][0]), context["nonces"][0],
        {original_hash, second_hash})
    if (rejected_b or not records_b or not switched_instances
            or set(switched_instances) != {initial_instance}
            or not _last(records_b, project_hash=second_hash,
                         session_hash=hashlib.sha256(foreign_id.encode()).hexdigest(),
                         top_level=True)):
        result["reason"] = "other_project_identity_or_same_client_not_verified"
        return result
    # Use the public open-session menu to return to the exact original root.
    # The initial route may have been the new root selected by the prior case;
    # its API target remains registered in the original fixture project.
    original_id = _identifier(context.get("new_root_id"))
    if original_id is None:
        original_id = context["session_ids"][0]
    target_original_hash = hashlib.sha256(original_id.encode()).hexdigest()
    ok, _seq = _menu_select(
        context, ROOT_TITLE if original_id == context.get("new_root_id") else "Panelink V2 Identity A",
        OTHER_PROJECT_TITLE,
        before_project_hash=second_hash,
        before_session_hash=hashlib.sha256(foreign_id.encode()).hexdigest(),
        target_project_hash=original_hash,
        target_session_hash=target_original_hash,
        target_top_level=True)
    if not ok:
        result["reason"] = "same_client_project_return_not_observed"
        return result
    result["switch_back_to_original"] = True
    result["passive_observer_fresh"] = _passive_fresh(
        {**context, "project_hash": original_hash}, context["passive_watermark"])
    if not result["passive_observer_fresh"]:
        result["reason"] = "passive_original_project_observer_stale"
        return result
    result.update(classification="proven", reason="same_tui_switched_project_and_returned",
                  second_project_hash=second_hash)
    return result


def _remaining_cases(context: dict[str, Any]) -> dict[str, Any]:
    result: dict[str, Any] = {
        "classification": "not_proven", "reason": "not_started",
        "new_root": {}, "nonempty_fork": {}, "same_client_project_switch": {},
        "no_prompt_endpoint_calls": True, "model_calls": 0,
        "network_scope": "fixture_loopback_only",
    }
    project: Path = context["project"]
    root_result = _new_root_case(context, project)
    result["new_root"] = root_result
    if root_result.get("classification") != "proven":
        result["reason"] = "new_root_gate_not_proven"
        return result
    context["new_root_id"] = None
    root_hash = root_result.get("identity_hash")
    # Keep the identifier in worker context only; never include it in output.
    rows = _list_sessions(context, project, top_level=True)
    root_id = None
    if rows is not None:
        root_matches = [row.get("id") for row in rows if isinstance(row, dict)
                        and hashlib.sha256(str(row.get("id", "")).encode()).hexdigest() == root_hash]
        if len(root_matches) == 1:
            root_id = _identifier(root_matches[0])
    if root_id is None:
        result["reason"] = "new_root_exact_id_lookup_failed"
        return result
    context["new_root_id"] = root_id
    fork_result = _imported_fork(context, project)
    result["nonempty_fork"] = fork_result
    if fork_result.get("classification") != "proven":
        result["reason"] = "nonempty_imported_fork_gate_not_proven"
        return result
    # The fork case returned to its imported parent. Restore the newly-created
    # root as the intended project-A identity before the cross-project case.
    parent_session_hash = root_result["identity_hash"]
    # The imported-parent route is private evidence only; open exact new root.
    records, rejected = _read_records(
        Path(context["record_paths"][0]), context["nonces"][0], context["project_hash"])
    if rejected or not records:
        result["reason"] = "active_tui_invalid_before_project_gate"
        return result
    if not _last(records, project_hash=context["project_hash"], top_level=True):
        result["reason"] = "active_tui_not_on_root_project_before_switch"
        return result
    if records[-1]["session_id_hash"] != parent_session_hash:
        ok, _seq = _menu_select(
            context, ROOT_TITLE, PARENT_TITLE,
            before_project_hash=context["project_hash"],
            before_session_hash=records[-1]["session_id_hash"],
            target_project_hash=context["project_hash"],
            target_session_hash=parent_session_hash, target_top_level=True)
        if not ok:
            result["reason"] = "new_root_not_restored_before_project_gate"
            return result
    project_result = _project_switch_case(context, project)
    result["same_client_project_switch"] = project_result
    if project_result.get("classification") != "proven":
        result["reason"] = "same_client_project_switch_gate_not_proven"
        return result
    result.update(classification="proven", reason="all_remaining_identity_gates_observed")
    return result


def _after_routes(context: dict[str, Any]) -> dict[str, Any]:
    return _remaining_cases(context)


def _callback(context: dict[str, Any]) -> dict[str, Any]:
    context = {**context, "scenario": "baseline", "after_routes": _after_routes,
               "hook_result_key": "child_routes"}
    result = tui_probe.after_bootstrap(context)
    # `after_bootstrap` only permits a small set of historical hook keys. Give
    # this hook its own descriptive result key in the final artifact.
    if "child_routes" in result:
        result["remaining_identity"] = result.pop("child_routes")
    return result


def _worker(executable: str, launcher: str) -> tuple[int, dict[str, Any]]:
    result = bootstrap._run_worker(
        executable, launcher, after_bootstrap=_callback,
        total_deadline_seconds=60.0, scenario="remaining-identity")
    nested = result.get("tui") if isinstance(result, dict) else None
    remaining = nested.get("remaining_identity") if isinstance(nested, dict) else None
    ok = (isinstance(remaining, dict) and remaining.get("classification") == "proven"
          and nested.get("classification") == "proven"
          and result.get("cleanup_completed") is True
          and nested.get("tui_cleanup_completed") is True)
    return (0 if ok else 2), result


def run(executable: str | None = None, *, which=shutil.which,
        home: Path | None = None) -> tuple[int, dict[str, Any]]:
    result: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "version_status": "blocked", "sandbox": "not_started",
        "network_scope": "loopback_only_sandbox", "prompt_submissions": 0,
    }
    chosen, launcher = bootstrap.resolve_executable(executable, which=which, home=home)
    if chosen is None or launcher is None:
        result["reason"] = "launcher_not_found"
        return 2, result
    if sys.platform != "darwin":
        result.update(reason="macos_sandbox_required", launcher=launcher)
        return 2, result
    sandbox = shutil.which("sandbox-exec")
    if sandbox is None:
        result.update(reason="sandbox_exec_unavailable", launcher=launcher)
        return 2, result
    cwd = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="panelink-v2-remaining-sandbox-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        profile = root / "loopback.sb"
        profile.write_text(bootstrap.SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        worker_env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C",
                      bootstrap.WORKER_ENV: "1"}
        argv = [sandbox, "-f", str(profile), sys.executable, str(Path(__file__).resolve()),
                "--sandbox-worker", chosen, launcher]
        code, raw, sandbox_error = bootstrap.bounded_worker(
            argv, cwd, worker_env, timeout=75.0)
    try:
        report = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        report = {"classification": "blocked",
                  "reason": sandbox_error or "sandbox_worker_invalid_output"}
    if not isinstance(report, dict):
        report = {"classification": "blocked", "reason": "sandbox_worker_invalid_output"}
    report["sandbox"] = "not_enforced" if sandbox_error else "enforced_loopback_only"
    report["sandbox_profile"] = "deny_network_except_loopback"
    success = (code == 0 and report.get("cleanup_completed") is True
               and isinstance(report.get("tui"), dict)
               and report["tui"].get("tui_cleanup_completed") is True
               and isinstance(report["tui"].get("remaining_identity"), dict)
               and report["tui"]["remaining_identity"].get("classification") == "proven")
    return (0 if success else 2), report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sandbox-worker", nargs=2, metavar=("BINARY", "LAUNCHER"))
    args = parser.parse_args()
    if args.sandbox_worker:
        code, report = _worker(args.sandbox_worker[0], args.sandbox_worker[1])
    else:
        code, report = run()
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
