#!/usr/bin/env python3
"""Bounded OpenCode 2.0.32 selected-route and concurrent-TUI diagnostic.

This enters through the authenticated bootstrap worker and uses only its
fixture-owned loopback server and private project. It submits no prompts.
"""

from __future__ import annotations

import hashlib
import argparse
import json
import math
import os
from pathlib import Path
import re
import select
import secrets
import shutil
import subprocess
import time
from typing import Any

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts.probe_opencode_tui_identity import (
        PrivatePty,
        correlate_switches,
        filtered_session_row_visible,
        session_list_visible,
    )
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    from probe_opencode_tui_identity import (
        PrivatePty,
        correlate_switches,
        filtered_session_row_visible,
        session_list_visible,
    )


MAX_RECORD_BYTES = 2 * 1024 * 1024
MAX_RECORDS = 1000
MAX_PTY_BYTES = 512 * 1024
MAX_PTY_BYTES = 512 * 1024
POLL_SECONDS = 0.1
MAX_CONTROL_BYTES = 128
MAX_CONTROL_WRITE_SECONDS = 1.0
MAX_LATENCY_MS = 5000.0
MAX_RECORD_GAP_MS = 1000.0
LABELS = ("Panelink V2 Identity A", "Panelink V2 Identity B")
STARTUP_MARKERS = {
    "trust_or_folder_prompt": ("trust this folder", "trust folder", "trust this project",
                               "untrusted folder", "folder is not trusted"),
    "workspace_approval_prompt": ("workspace approval", "approve workspace",
                                  "trust workspace", "workspace is not trusted"),
    "loading_indicator": ("loading",),
    "plugin_error_visible": ("plugin error", "plugin failed", "failed to load plugin"),
    "error_visible": ("error", "failed", "failure"),
    "connect_prompt": ("connect", "connecting"),
    "provider_prompt": ("provider",),
    "no_models_prompt": ("no models", "no available models"),
    "config_error_visible": ("config error", "invalid config", "failed to load config",
                             "configuration error"),
    "plugin_fixture_id_visible": ("panelink.v2-identity-fixture",),
    "press_enter_or_continue_prompt": ("press enter", "press any key", "continue"),
}


def hash_value(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def parse_client_records(payload: bytes, *, client_nonce: str,
                         project_hash: str,
                         enforce_gap_bound: bool = True) -> tuple[list[dict[str, Any]], int]:
    """Validate the complete bounded, hash-only record stream for one TUI."""
    if len(payload) >= MAX_RECORD_BYTES:
        return [], 1
    records: list[dict[str, Any]] = []
    rejected = 0
    previous_seq = 0
    previous_ms = -1.0
    identity_ready = False
    instance_nonce: str | None = None
    lines = payload.splitlines()
    if len(lines) >= MAX_RECORDS:
        return [], 1
    for raw in lines:
        if len(raw) > 4096 or len(records) >= MAX_RECORDS:
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
        observed_ms = item.get("monotonic_ms")
        route_name = item.get("route_name")
        session_hash = item.get("session_id_hash")
        record_nonce = item.get("client_nonce")
        record_instance = item.get("tui_instance")
        record_project = item.get("project_hash")
        kind = item.get("kind", "poll")
        schema = item.get("schema")
        timestamp_valid = (isinstance(observed_ms, (int, float))
                           and not isinstance(observed_ms, bool)
                           and (not isinstance(observed_ms, int) or observed_ms <= 10**15))
        if timestamp_valid:
            try:
                timestamp_valid = math.isfinite(float(observed_ms))
            except (OverflowError, ValueError):
                timestamp_valid = False
        if (not isinstance(seq, int) or isinstance(seq, bool) or seq != previous_seq + 1
                or seq > MAX_RECORDS
                or not isinstance(schema, int) or isinstance(schema, bool) or schema != 1
                or not timestamp_valid or observed_ms < 0 or observed_ms <= previous_ms
                or (enforce_gap_bound and identity_ready
                    and observed_ms - previous_ms > MAX_RECORD_GAP_MS)
                or route_name not in ("home", "session", "plugin", "other")
                or record_nonce != client_nonce
                or (identity_ready and record_project != project_hash)
                or (record_project is not None and (
                    not isinstance(record_project, str) or len(record_project) != 64
                    or any(c not in "0123456789abcdef" for c in record_project)))
                or not isinstance(record_instance, str) or len(record_instance) != 32
                or any(c not in "0123456789abcdef" for c in record_instance)
                or (instance_nonce is not None and record_instance != instance_nonce)
                or not isinstance(item.get("session_top_level"), (bool, type(None)))
                or (session_hash is not None and
                    (not isinstance(session_hash, str) or
                     len(session_hash) != 64 or any(c not in "0123456789abcdef" for c in session_hash)))
                or kind not in ("poll", "plugin_loaded", "route_change_callback")
                or set(item) - {"schema", "kind", "client_nonce", "tui_instance", "seq",
                                "monotonic_ms", "route_name", "session_id_hash",
                                "session_top_level", "project_hash"}
                or any(key in item for key in
                       ("session_id", "sessionID", "id", "body", "text", "prompt", "project"))):
            rejected += 1
            continue
        previous_seq = seq
        previous_ms = float(observed_ms)
        instance_nonce = record_instance
        identity_ready = identity_ready or (
            route_name == "session" and session_hash is not None
            and item["session_top_level"] is True and record_project == project_hash)
        records.append({
            "seq": seq,
            "monotonic_ms": previous_ms,
            "route_name": route_name,
            "session_id_hash": session_hash,
            "session_top_level": item["session_top_level"],
            "project_hash": record_project,
            "kind": kind,
        })
    return records, rejected


def current_route(records: list[dict[str, Any]], expected_hash: str,
                  project_hash: str) -> bool:
    return bool(records and records[-1]["route_name"] == "session"
                and records[-1]["session_id_hash"] == expected_hash
                and records[-1]["session_top_level"] is True
                and records[-1]["project_hash"] == project_hash)


def selected_history_matches(records: list[dict[str, Any]], initial_seq: int,
                             initial_hash: str, expected_hashes: list[str],
                             expected_changes: list[str],
                             project_hash: str) -> bool:
    """Reject child, unknown, and non-session routes after initial identity capture."""
    history = [record for record in records if int(record["seq"]) > initial_seq]
    if not history or any(
            record["route_name"] != "session" or record["session_top_level"] is not True
            or record["session_id_hash"] not in expected_hashes
            or record["project_hash"] != project_hash for record in history):
        return False
    changes: list[str] = []
    for record in history:
        value = record["session_id_hash"]
        if not changes or changes[-1] != value:
            changes.append(value)
    if changes and changes[0] == initial_hash:
        changes = changes[1:]
    return changes == expected_changes


def passive_history_matches(records: list[dict[str, Any]], initial_seq: int,
                            expected_hash: str, project_hash: str) -> bool:
    """Validate passive-client history after its initial displayed route is known."""
    history = [record for record in records if int(record["seq"]) >= initial_seq]
    return bool(history) and all(
        record["route_name"] == "session" and record["session_top_level"] is True
        and record["session_id_hash"] == expected_hash
        and record["project_hash"] == project_hash for record in history)


def _send_control(tui: PrivatePty, payload: bytes, deadline: float) -> bool:
    """Write only tiny key/search controls with a fixed readiness deadline."""
    if not payload or len(payload) > MAX_CONTROL_BYTES or tui.master < 0:
        return False
    stop = min(deadline, time.monotonic() + MAX_CONTROL_WRITE_SECONDS)
    view = memoryview(payload)
    while view and time.monotonic() < stop:
        try:
            _readable, writable, _errors = select.select([], [tui.master], [],
                                                          min(0.05, stop - time.monotonic()))
            if not writable:
                continue
            written = os.write(tui.master, view)
            if written <= 0:
                return False
            view = view[written:]
        except (OSError, ValueError):
            return False
    return not view


def _tick(tuis: list[PrivatePty], deadline: float) -> None:
    if time.monotonic() >= deadline:
        raise TimeoutError("tui_deadline")
    for tui in tuis:
        if not tui.alive():
            raise RuntimeError("tui_exited")
        tui.drain(min(POLL_SECONDS, max(0.0, deadline - time.monotonic())))
        if sum(len(active.output) for active in tuis) >= MAX_PTY_BYTES:
            raise RuntimeError("pty_output_cap")


def _wait_until(predicate, tuis: list[PrivatePty], deadline: float) -> bool:
    while time.monotonic() < deadline:
        _tick(tuis, deadline)
        if predicate():
            return True
    return bool(predicate())


def _records(path: Path, nonce: str, project_hash: str) -> tuple[list[dict[str, Any]], int]:
    try:
        with path.open("rb") as stream:
            payload = stream.read(MAX_RECORD_BYTES + 1)
    except OSError:
        return [], 1
    return parse_client_records(payload, client_nonce=nonce, project_hash=project_hash)


def _records_with_policy(path: Path, nonce: str, project_hash: str,
                         *, enforce_gap_bound: bool) -> tuple[list[dict[str, Any]], int]:
    try:
        with path.open("rb") as stream:
            payload = stream.read(MAX_RECORD_BYTES + 1)
    except OSError:
        return [], 1
    return parse_client_records(payload, client_nonce=nonce, project_hash=project_hash,
                                enforce_gap_bound=enforce_gap_bound)


def _client_diagnostic(path: Path, nonce: str, tui: PrivatePty) -> dict[str, Any]:
    """Return allowlisted counts for diagnosing an initial route capture miss."""
    try:
        with path.open("rb") as stream:
            payload = stream.read(MAX_RECORD_BYTES + 1)
        file_size = path.stat().st_size
    except OSError:
        payload, file_size = b"", 0
    valid_rows: list[dict[str, Any]] = []
    for raw in payload[:MAX_RECORD_BYTES].splitlines()[:MAX_RECORDS]:
        if len(raw) > 4096:
            continue
        try:
            row = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
        if isinstance(row, dict):
            valid_rows.append(row)
    diagnostics = {
        "pty_output_bytes": len(tui.output),
        "pty_alive": tui.alive(),
        "record_file_bytes": min(file_size, MAX_RECORD_BYTES + 1),
        "raw_json_valid_row_count": len(valid_rows),
        "plugin_loaded_record_count": sum(row.get("kind") == "plugin_loaded" for row in valid_rows),
        "project_hash_nonnull_count": sum(bool(row.get("project_hash")) for row in valid_rows),
        "session_route_hash_count": sum(
            row.get("route_name") == "session" and bool(row.get("session_id_hash"))
            for row in valid_rows),
        "top_level_session_count": sum(row.get("session_top_level") is True for row in valid_rows),
        "client_nonce_match_count": sum(row.get("client_nonce") == nonce for row in valid_rows),
    }
    diagnostics.update(_startup_markers(bytes(tui.output[:MAX_PTY_BYTES])))
    return diagnostics


def _startup_markers(output: bytes) -> dict[str, bool]:
    """Scan bounded terminal output for fixed startup indicators; never return text."""
    bounded = output[:MAX_PTY_BYTES].decode("utf-8", errors="replace").lower()
    # Strip ANSI control sequences so styling does not split ordinary words.
    plain = re.sub(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))", " ", bounded)
    return {name: any(marker in plain for marker in markers)
            for name, markers in STARTUP_MARKERS.items()}


def _filter_guard_diagnostic(tui: PrivatePty, target: str, other: str) -> dict[str, Any]:
    screen = tui.screen_text()
    controls = re.findall(rb"\x1b\[([0-?]*)([ -/]*)([@-~])", bytes(tui.output[:MAX_PTY_BYTES]))
    return {
        "sessions_visible": "Sessions" in screen,
        "target_visible": target in screen,
        "other_visible": other in screen,
        "other_visible_after_picker_title": other in _picker_screen_text(screen),
        "partial_display_erase_count": sum(final == b"J" and params not in (b"2", b"3")
                                            for params, _intermediate, final in controls),
        "erase_characters_count": sum(final == b"X" for _params, _intermediate, final in controls),
        "delete_lines_count": sum(final == b"M" for _params, _intermediate, final in controls),
        "scroll_region_count": sum(final == b"r" for _params, _intermediate, final in controls),
    }


def _picker_screen_text(screen: str) -> str:
    """Exclude the current session header above the modal's Sessions title."""
    _background, title, content = screen.partition("Sessions")
    return title + content if title else ""


def after_bootstrap(context: dict[str, Any]) -> dict[str, Any]:
    """Run one two-TUI diagnostic inside the authenticated sandbox worker."""
    root: Path = context["root"]
    project: Path = context["project"]
    env: dict[str, str] = context["env"]
    base: str = context["base"]
    ids: tuple[str, ...] = context["session_ids"]
    deadline: float = context["deadline"]
    executable: str = context["executable"]
    launcher: str = context["launcher"]
    scenario = context.get("scenario", "baseline")
    started = time.monotonic()
    result: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "plugin_loaded": False, "initial_routes_captured": False,
        "first_tui_A_B_A_observed": False, "second_tui_stayed_B": False,
        "attempted_control_count": 0, "missed_attempt_count": 0,
        "rejected_record_count": 0, "prompt_submissions": 0,
        "sessions_created_count": 2, "tui_count": 2,
        "route_observation_interval_ms": 100,
        "latency_measurement": "conservative_plugin_polling_upper_bound",
        "active_client_index": 0,
        "active_round_trip_observed": False,
        "passive_identity_unchanged": False,
        "new_session_outcome": "not_attempted",
        "new_session_control_count": 0,
    }
    tuis: list[PrivatePty] = []
    try:
        if len(ids) != 2:
            result["reason"] = "two_fixture_sessions_required"
            return result
        if scenario not in bootstrap.TUI_SCENARIOS:
            result["reason"] = "unknown_scenario"
            return result
        if root.stat().st_mode & 0o077 or project.stat().st_mode & 0o077:
            result["reason"] = "private_fixture_permissions_unverified"
            return result
        # The server is already running; install a private CLI config after
        # readiness so its plugin cannot affect the server's startup path.
        record_paths = [root / "tui-a.jsonl", root / "tui-b.jsonl"]
        control_paths = [root / "tui-a.control", root / "tui-b.control"]
        nonces = [secrets.token_hex(16), secrets.token_hex(16)]
        config_dir = root / "opencode-config"
        config_path = config_dir / "cli.json"
        fixture_path = (Path(__file__).resolve().parent.parent / "docs" / "progress"
                        / "p4-v2-tui-plugin-fixture" / "plugin.mjs")
        try:
            config_dir.mkdir(mode=0o700)
            plugin_dir = root / "identity-plugin"
            plugin_dir.mkdir(mode=0o700)
            plugin_entry = plugin_dir / "tui.ts"
            shutil.copyfile(fixture_path, plugin_entry)
            plugin_entry.chmod(0o600)
            config_path.write_text(json.dumps({"plugins": [str(plugin_dir)]}), encoding="utf-8")
            config_path.chmod(0o600)
            for path in record_paths:
                path.touch(mode=0o600)
                path.chmod(0o600)
        except OSError:
            result["reason"] = "private_tui_fixture_unavailable"
            return result

        project_hash = hash_value(str(project.resolve()))
        client_envs = []
        for path, nonce, control_path in zip(record_paths, nonces, control_paths):
            client_env = dict(env)
            client_env.update({
                "TERM": "xterm-256color",
                "OPENCODE_CONFIG_DIR": str(config_dir),
                "PANELINK_PROBE_RECORDS": str(path),
                "PANELINK_PROBE_CLIENT_NONCE": nonce,
                "PANELINK_PROBE_CONTROL": str(control_path),
            })
            client_envs.append(client_env)

        # Both clients attach to the exact owned server and project. The CLI
        # arguments choose only the initial display; plugin observations prove it.
        for session_id, client_env in zip(ids, client_envs):
            argv = bootstrap.launcher_argv(
                executable, launcher,
                ["--server", base, "--session", session_id, str(project)],
            )
            tuis.append(PrivatePty(argv, project, client_env))

        hashes = [hash_value(value) for value in ids]

        def load_records(index: int):
            records, rejected = _records_with_policy(
                record_paths[index], nonces[index], project_hash,
                enforce_gap_bound=True)
            result["rejected_record_count"] = max(result["rejected_record_count"], rejected)
            return records

        records = [[], []]

        def both_initial_routes() -> bool:
            records[0] = load_records(0)
            records[1] = load_records(1)
            return current_route(records[0], hashes[0], project_hash) and current_route(
                records[1], hashes[1], project_hash)

        if not _wait_until(both_initial_routes, tuis, min(deadline, time.monotonic() + 15.0)):
            result["client_diagnostics"] = [
                _client_diagnostic(path, nonce, tui)
                for path, nonce, tui in zip(record_paths, nonces, tuis)
            ]
            result["reason"] = "initial_displayed_identity_not_observed"
            return result
        result["plugin_loaded"] = all(
            any(record["kind"] == "plugin_loaded" for record in stream) for stream in records)
        if not result["plugin_loaded"]:
            result["client_diagnostics"] = [
                _client_diagnostic(path, nonce, tui)
                for path, nonce, tui in zip(record_paths, nonces, tuis)
            ]
            result["reason"] = "tui_plugin_not_loaded"
            return result
        result["initial_routes_captured"] = True
        active_index = 1 if scenario == "reverse-new" else 0
        passive_index = 1 - active_index
        result["active_client_index"] = active_index
        active_initial_hash = hashes[active_index]
        active_initial_seq = int(records[active_index][-1]["seq"])
        passive_initial_hash = hashes[passive_index]
        passive_initial_seq = int(records[passive_index][-1]["seq"])
        previous_selected_hash = active_initial_hash

        markers: list[dict[str, float | int]] = []
        target_order = (0, 1) if scenario == "reverse-new" else (1, 0)
        for target_index in target_order:
            _tick(tuis, deadline)
            # The other physical TUI is the live isolation observer for every
            # switch, regardless of which physical client is active.
            records[passive_index] = load_records(passive_index)
            if not _fresh_expected(records[passive_index], passive_initial_hash, project_hash):
                result["reason"] = "passive_tui_initial_route_stale_or_changed"
                return result
            active_seq_before_picker = int(records[active_index][-1]["seq"])
            passive_seq_at_start = int(records[passive_index][-1]["seq"])
            passive_advanced = False
            passive_invalid = False
            if not _open_picker(tuis[active_index], tuis, deadline, project_hash, load_records,
                                previous_selected_hash, LABELS, active_seq_before_picker,
                                client_index=active_index):
                result["reason"] = "session_picker_not_observed"
                return result
            target_label = LABELS[target_index]
            other_label = LABELS[1 - target_index]
            if not _send_control(tuis[active_index], target_label.encode("ascii"), deadline):
                result["reason"] = "picker_filter_write_failed"
                return result
            if not _wait_until(lambda: filtered_session_row_visible(
                    _picker_screen_text(tuis[active_index].screen_text()), target_label, other_label), tuis,
                    min(deadline, time.monotonic() + 4.0)):
                result["picker_filter_diagnostic"] = _filter_guard_diagnostic(
                    tuis[active_index], target_label, other_label)
                result["reason"] = "filtered_target_row_not_observed"
                return result
            records[active_index] = load_records(active_index)
            if not _fresh_expected(records[active_index], previous_selected_hash, project_hash,
                                   after_seq=active_seq_before_picker):
                result["reason"] = "preselection_observation_stale"
                return result
            # Issued time comes from the plugin's latest preselection record,
            # never Python's clock. The 100 ms poll means latency is an upper bound.
            marker = {"after_seq": int(records[active_index][-1]["seq"]),
                      "issued_ms": float(records[active_index][-1]["monotonic_ms"])}
            if not _send_control(tuis[active_index], b"\r", deadline):
                result["reason"] = "picker_select_write_failed"
                return result
            markers.append(marker)
            result["attempted_control_count"] = len(markers)

            target_hash = hashes[target_index]

            def route_changed() -> bool:
                nonlocal passive_advanced, passive_invalid
                records[active_index] = load_records(active_index)
                records[passive_index] = load_records(passive_index)
                passive_tail = [record for record in records[passive_index]
                                if int(record["seq"]) >= passive_initial_seq]
                if not passive_history_matches(records[passive_index], passive_initial_seq,
                                               passive_initial_hash, project_hash):
                    passive_invalid = True
                elif int(passive_tail[-1]["seq"]) > passive_seq_at_start:
                    passive_advanced = True
                return any(record["seq"] > marker["after_seq"]
                           and record["monotonic_ms"] >= marker["issued_ms"]
                           and record["route_name"] == "session"
                           and record["session_id_hash"] == target_hash
                           and record["session_top_level"] is True
                           and record["project_hash"] == project_hash
                           for record in records[active_index])

            if not _wait_until(route_changed, tuis,
                               min(deadline, time.monotonic() + 8.0)):
                if passive_invalid:
                    result.update(classification="not_proven",
                                  reason="passive_tui_route_changed_during_switch")
                    return result
                result.update(classification="not_proven",
                              reason="keyboard_selected_route_not_observed")
                return result
            if passive_invalid or not passive_advanced:
                result.update(classification="not_proven",
                              reason="passive_tui_fresh_route_not_observed_during_switch")
                return result
            observed = next((record for record in records[active_index]
                             if int(record["seq"]) > int(marker["after_seq"])
                             and record["monotonic_ms"] >= marker["issued_ms"]
                             and record["route_name"] == "session"
                             and record["session_id_hash"] == target_hash
                             and record["session_top_level"] is True
                             and record["project_hash"] == project_hash), None)
            if observed is None:
                result["reason"] = "keyboard_selected_route_not_observed"
                return result
            latency_upper_bound = float(observed["monotonic_ms"]) - float(marker["issued_ms"])
            result.setdefault("switch_latency_upper_bounds_ms", []).append(
                round(latency_upper_bound, 3))
            if latency_upper_bound > MAX_LATENCY_MS:
                result.update(classification="not_proven",
                              reason="keyboard_selected_route_exceeded_latency_bound")
                return result
            expected_changes = [hashes[index] for index in
                                target_order[:target_order.index(target_index) + 1]]
            if not selected_history_matches(records[active_index], active_initial_seq,
                                            active_initial_hash, hashes,
                                            expected_changes, project_hash):
                result.update(classification="not_proven",
                              reason="unexpected_selected_route_observed")
                return result
            records[passive_index] = load_records(passive_index)
            if not _fresh_expected(records[passive_index], passive_initial_hash, project_hash):
                result["reason"] = "passive_tui_route_changed_or_stale_during_switch"
                return result
            passive_seq_at_target = int(records[passive_index][-1]["seq"])

            def passive_advanced_after_target() -> bool:
                passive_records = load_records(passive_index)
                return (passive_history_matches(passive_records, passive_initial_seq,
                                                passive_initial_hash, project_hash)
                        and int(passive_records[-1]["seq"]) > passive_seq_at_target)

            passive_fresh_after_match = _wait_until(
                passive_advanced_after_target,
                tuis, min(deadline, time.monotonic() + 1.0))
            if not passive_fresh_after_match:
                result.update(classification="not_proven",
                reason="passive_tui_freshness_not_confirmed_after_transition")
                return result

            # Require a fresh route record from TUI 1 before entering the next
            # picker. It also ensures the previous change has settled in view.
            settled = _wait_until(
                lambda: _fresh_expected(load_records(active_index), target_hash, project_hash,
                                         after_seq=int(marker["after_seq"])),
                tuis, min(deadline, time.monotonic() + 3.0))
            if not settled:
                result["reason"] = "selected_route_did_not_remain_fresh"
                return result
            previous_selected_hash = target_hash

        records[0], rejected_a = _records_with_policy(
            record_paths[0], nonces[0], project_hash,
            enforce_gap_bound=True)
        records[1], rejected_b = _records_with_policy(
            record_paths[1], nonces[1], project_hash,
            enforce_gap_bound=True)
        result["rejected_record_count"] = max(result["rejected_record_count"], rejected_a, rejected_b)
        expected_target_hashes = [hashes[index] for index in target_order]
        correlated = correlate_switches(records[active_index], active_initial_hash,
                                        expected_target_hashes, markers,
                                        max_latency_ms=MAX_LATENCY_MS)
        first_history_valid = selected_history_matches(
            records[active_index], active_initial_seq, active_initial_hash, hashes,
            expected_target_hashes, project_hash)
        passive_records = [record for record in records[passive_index]
                           if int(record["seq"]) >= passive_initial_seq]
        passive_history_valid = passive_history_matches(
            records[passive_index], passive_initial_seq, passive_initial_hash, project_hash)
        round_trip = (correlated["switch_A_B_A_observed"] and first_history_valid
                      and correlated["latency_bounds_passed"]
                      and correlated["no_extra_transitions"]
                      and correlated["missed_attempt_count"] == 0)
        result.update({
            "first_tui_A_B_A_observed": (active_index == 0 and scenario != "reverse-new"
                                          and round_trip),
            "active_round_trip_observed": round_trip,
            "first_tui_route_history_valid": first_history_valid,
            "latency_bounds_passed": correlated["latency_bounds_passed"],
            "missed_attempt_count": correlated["missed_attempt_count"],
            "extra_transition_count": correlated["extra_transition_count"],
            "first_tui_record_count": len(records[active_index]),
            "second_tui_record_count": len(passive_records),
            "second_tui_stayed_B": passive_history_valid and passive_index == 1,
            "passive_identity_unchanged": passive_history_valid,
            "switch_latency_upper_bounds_ms": [row["latency_ms"] for row in correlated["transitions"]],
        })
        if result["rejected_record_count"]:
            result.update(classification="not_proven", reason="invalid_or_capped_plugin_records")
        elif round_trip \
                and correlated["latency_bounds_passed"] \
                and correlated["no_extra_transitions"] and correlated["missed_attempt_count"] == 0 \
                and result["passive_identity_unchanged"]:
            result.update(classification="proven", reason="two_tui_selected_identity_and_switches_observed")
        else:
            result.update(classification="not_proven", reason="ordered_switch_or_isolation_not_proven")

        hook = context.get("after_routes")
        if result["classification"] == "proven" and callable(hook):
            def pump(seconds: float = POLL_SECONDS) -> bool:
                if time.monotonic() >= deadline:
                    return False
                _tick(tuis, min(deadline, time.monotonic() + max(0.0, min(seconds, 1.0))))
                return all(tui.alive() for tui in tuis)

            hook_context = dict(context)
            hook_context.update({
                "tuis": tuis,
                "record_paths": record_paths,
                "nonces": nonces,
                "control_paths": control_paths,
                "project_hash": project_hash,
                "hashes": hashes,
                "pump": pump,
            })
            try:
                bridge_health = hook(hook_context)
                if not isinstance(bridge_health, dict):
                    bridge_health = {"classification": "blocked", "reason": "invalid_hook_result"}
            except Exception:
                bridge_health = {"classification": "blocked", "reason": "hook_failed"}
            hook_key = context.get("hook_result_key", "bridge_health")
            if hook_key not in ("bridge_health", "child_routes", "project_isolation"):
                hook_key = "bridge_health"
                bridge_health = {"classification": "blocked", "reason": "invalid_hook_result_key"}
            result[hook_key] = bridge_health
            if bridge_health.get("classification") != "proven":
                result.update(classification="not_proven", reason=hook_key + "_not_proven")

        if scenario == "reverse-new" and result["classification"] == "proven":
            if context.get("new_session_key_verified") is not True:
                result["new_session_outcome"] = "key_not_verified"
                result.update(classification="not_proven", reason="new_session_key_not_verified")
            elif not _send_control(tuis[active_index], b"\x18n", deadline):
                result["new_session_outcome"] = "control_write_failed"
                result.update(classification="not_proven", reason="new_session_control_write_failed")
            else:
                result["new_session_control_count"] = 1
                marker_seq = int(load_records(active_index)[-1]["seq"])
                passive_before_new = load_records(passive_index)
                passive_seq_before_new = int(passive_before_new[-1]["seq"])
                passive_new_fresh = False
                candidate: dict[str, Any] | None = None

                def new_route_and_passive_fresh() -> bool:
                    nonlocal candidate, passive_new_fresh
                    active_records = load_records(active_index)
                    passive_records_now = load_records(passive_index)
                    passive_tail = [row for row in passive_records_now
                                    if int(row["seq"]) > passive_seq_before_new]
                    passive_new_fresh = (
                        passive_history_matches(passive_records_now, passive_initial_seq,
                                                passive_initial_hash, project_hash)
                        and bool(passive_tail))
                    candidate = next((row for row in active_records
                                      if int(row["seq"]) > marker_seq
                                      and (row["route_name"] == "home"
                                           or (row["route_name"] == "session"
                                               and row["session_top_level"] is True
                                               and row["session_id_hash"] != previous_selected_hash))),
                                     None)
                    return candidate is not None and passive_new_fresh

                _wait_until(
                    new_route_and_passive_fresh, tuis,
                    min(deadline, time.monotonic() + 5.0))
                new_records = load_records(active_index)
                new_route = next((row for row in new_records
                                  if candidate is not None and int(row["seq"]) == int(candidate["seq"])),
                                 None)
                passive_after_new = load_records(passive_index)
                result["passive_identity_unchanged"] = (
                    passive_history_matches(passive_after_new, passive_initial_seq,
                                            passive_initial_hash, project_hash)
                    and int(passive_after_new[-1]["seq"]) > passive_seq_before_new)
                if new_route is None:
                    result["new_session_outcome"] = "route_change_not_observed"
                    result.update(classification="not_proven",
                                  reason="new_session_route_not_observed")
                elif not result["passive_identity_unchanged"]:
                    result["new_session_outcome"] = "passive_identity_not_fresh_after_new_control"
                    result.update(classification="not_proven",
                                  reason="passive_identity_not_fresh_after_new_control")
                elif new_route["route_name"] == "home":
                    result["new_session_outcome"] = "home_without_selected_session_id"
                else:
                    route_hash = new_route["session_id_hash"]
                    if route_hash in hashes:
                        result["new_session_outcome"] = "existing_fixture_session_selected"
                        result.update(classification="not_proven",
                                      reason="new_key_selected_known_session")
                    else:
                        lookup = context.get("lookup_route_hash")
                        try:
                            proof = lookup(route_hash) if callable(lookup) else None
                        except Exception:
                            proof = None
                        exact = (isinstance(proof, dict)
                                 and proof.get("exact_session") is True
                                 and proof.get("project_matches") is True
                                 and proof.get("top_level") is True
                                 and proof.get("status") == "found")
                        result["new_session_outcome"] = (
                            "new_top_level_session_verified" if exact
                            else "new_route_lookup_not_proven")
                        result["new_route_lookup"] = {
                            "exact_session": exact,
                            "project_matches": isinstance(proof, dict)
                            and proof.get("project_matches") is True,
                            "top_level": isinstance(proof, dict) and proof.get("top_level") is True,
                            "status": proof.get("status") if isinstance(proof, dict)
                            and proof.get("status") in ("found", "missing", "error") else "unknown",
                        }
                        if not exact:
                            result.update(classification="not_proven",
                                          reason="new_route_identity_not_proven")
        return result
    except TimeoutError:
        result.update(classification="blocked", reason="diagnostic_deadline")
        return result
    except (OSError, RuntimeError, subprocess.SubprocessError, ValueError):
        result.update(classification="blocked", reason="tui_driver_failed")
        return result
    finally:
        cleanup_ok = True
        cleanup_receipts = []
        for index in reversed(range(len(tuis))):
            tui = tuis[index]
            process_group_reaped = False
            pty_close_completed = False
            try:
                process_group_reaped = bool(tui.child is not None
                                             and bootstrap._kill_group(tui.child))
                if tui.child is not None and not process_group_reaped:
                    cleanup_ok = False
                tui.close()
                pty_close_completed = True
            except (OSError, subprocess.SubprocessError):
                cleanup_ok = False
            cleanup_receipts.append({
                "client_index": index,
                "process_group_reaped": process_group_reaped,
                "pty_close_completed": pty_close_completed,
            })
        result["tui_cleanup_completed"] = cleanup_ok
        result["tui_cleanup_receipts"] = cleanup_receipts
        result["pty_output_bytes"] = sum(len(tui.output) for tui in tuis)
        if not cleanup_ok:
            result.update(classification="blocked", reason="tui_process_cleanup_unverified")
        result["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)


def _open_picker(tui: PrivatePty, active: list[PrivatePty], deadline: float,
                 project_hash: str, load_records, expected_hash: str,
                 labels: tuple[str, str], after_seq: int, *, client_index: int = 0) -> bool:
    # Ctrl-X is the confirmed leader; l opens the session list. Wait for the
    # picker and both exact fixture titles before filtering.
    if not _send_control(tui, b"\x18l", deadline):
        return False
    return _wait_until(lambda: session_list_visible(_picker_screen_text(tui.screen_text()), labels)
                       and _fresh_expected(load_records(client_index), expected_hash, project_hash,
                                           after_seq=after_seq),
                       active, min(deadline, time.monotonic() + 4.0))


def _fresh_any(records: list[dict[str, Any]], project_hash: str) -> bool:
    return bool(records and records[-1]["project_hash"] == project_hash)


def _fresh_expected(records: list[dict[str, Any]], expected_hash: str,
                    project_hash: str, *, after_seq: int | None = None) -> bool:
    return (current_route(records, expected_hash, project_hash)
            and _fresh_any(records, project_hash)
            and (after_seq is None or int(records[-1]["seq"]) > after_seq))


def run(executable: str | None = None, *, which=shutil.which,
        home: Path | None = None, scenario: str = "baseline") -> tuple[int, dict[str, Any]]:
    """Invoke the pinned bootstrap with this module's TUI callback under sandbox-exec."""
    if scenario not in bootstrap.TUI_SCENARIOS:
        raise ValueError("unsupported scenario")
    return bootstrap.run(executable, which=which, home=home, tui=True, scenario=scenario)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=bootstrap.TUI_SCENARIOS,
                        default="baseline")
    args = parser.parse_args()
    code, result = run(scenario=args.scenario)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
