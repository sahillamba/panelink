#!/usr/bin/env python3
"""Private native-OpenCode acceptance for Panelink's explicit recovery CLI.

Uses the pinned OpenCode API on loopback and Panelink's managed bridge. It
creates only empty fixture sessions, never submits prompts, and prints only
fixed statuses, booleans, and SHA-256 identifiers. All fixture state is owned
by a temporary HOME and removed during cleanup.
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

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts import probe_opencode_v2_remaining_identity as identity
    from scripts import probe_panelink_provider_bridge as bridge_probe
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    import probe_opencode_v2_remaining_identity as identity
    import probe_panelink_provider_bridge as bridge_probe


EXPECTED_VERSION = bootstrap.EXPECTED_VERSION
EXPECTED_SHA256 = bootstrap.EXPECTED_SHA256
MAX_CLI_BYTES = 4096
MAX_PROBE_SECONDS = 90.0
_FIXTURE_STAGE = "callback_start"


def _identifier_hash(value: str) -> str:
    return hashlib.sha256(value.encode("ascii")).hexdigest()


def _manual_error(cli: dict[str, Any], expected: str) -> bool:
    return (cli.get("valid") is True and cli.get("returncode") == 1
            and cli.get("state") == "manual_selection_required"
            and cli.get("error") == expected
            and cli.get("automatic_resume") is False
            and cli.get("current_at_crash") is False)


def _public_evidence(nested: dict[str, Any]) -> dict[str, Any]:
    """Project worker output onto the fixed, non-secret evidence schema."""
    fields = (
        "reason", "invalid_choice_errors", "primary_source_preserved",
        "prepared_status_verified", "explicit_resume_confirmed",
        "duplicate_resume_rejected", "duplicate_did_not_advance_revision",
        "historical_managed_linked", "native_root_choice_verified",
        "no_user_messages", "no_prompt_submissions", "explicit_resume_attempted",
        "serve_lines_valid", "cleanup_completed",
        "all_user_ids_sha256", "deleted_fixture_verified",
        "deleted_fixture_had_no_user_messages", "post_resume_status_confirmed",
        "selected_choice_snapshot", "source_record_preserved_before_resume",
        "primary_initial_revision_is_one",
        "primary_source_unchanged_multiple_checks", "all_invalid_choices_manual",
        "missing_lookup_classified", "deleted_lookup_classified",
        "foreign_lookup_classified", "child_lookup_classified",
        "safe_manual_fallback_for_uncertain_lookup",
        "fixture_setup_complete", "fixture_setup_checks", "serve_startup_status",
        "managed_directory_private", "unix_socket_path_bounded",
        "pinned_provider_hash_matches_launch",
        "fixture_stage",
        "fork_seed_export_verified", "fork_seed_removed_for_import",
        "fork_seed_absent_before_import", "fork_import_route_succeeded",
        "fork_import_roundtrip_exact", "fork_parent_link_verified",
        "fork_child_is_top_level", "fork_history_hash_verified",
    )
    evidence = {key: nested.get(key) for key in fields if key in nested}
    evidence["classification"] = nested.get("classification", "blocked")
    return evidence


def _session(context: dict[str, Any], project: Path, title: str,
             label: str, checks: dict[str, bool]) -> str | None:
    status, body = identity._request(context, "POST", "/api/session", {
        "title": title, "location": {"directory": str(project)},
    })
    identifier = identity._session_id(body, top_level=True) if status == 200 else None
    checks[label + "_created"] = identifier is not None
    if identifier is None:
        return None
    status, detail = identity._request(
        context, "GET", f"/api/session/{identifier}")
    data = identity._data(detail) if status == 200 else None
    if (data is None or data.get("id") != identifier
            or data.get("parentID", data.get("parentId")) is not None):
        checks[label + "_info_verified"] = False
        return None
    location = data.get("location")
    directory = location.get("directory") if isinstance(location, dict) else None
    try:
        if Path(directory).resolve(strict=True) != project.resolve(strict=True):
            checks[label + "_info_verified"] = False
            return None
    except (OSError, TypeError, ValueError):
        checks[label + "_info_verified"] = False
        return None
    checks[label + "_info_verified"] = True
    return identifier


def _child(context: dict[str, Any], parent_id: str, project: Path,
           checks: dict[str, bool]) -> str | None:
    # OpenCode's documented POST /api/session parentID field creates a linked
    # child without starting a model turn or relying on forked transcript data.
    status, body = identity._request(context, "POST", "/api/session", {
        "parentID": parent_id, "title": "Panelink manual recovery linked child",
    })
    child_data = identity._data(body) if status == 200 else None
    identifier = identity._identifier(child_data.get("id")) if child_data else None
    checks["child_created"] = identifier is not None
    if identifier is None:
        return None
    status, detail = identity._request(context, "GET", f"/api/session/{identifier}")
    data = identity._data(detail) if status == 200 else None
    parent = data.get("parentID", data.get("parentId")) if data else None
    location = data.get("location") if data else None
    directory = location.get("directory") if isinstance(location, dict) else None
    try:
        project_matches = Path(directory).resolve(strict=True) == project.resolve(strict=True)
    except (OSError, TypeError, ValueError):
        project_matches = False
    valid = isinstance(parent, str) and parent == parent_id and project_matches
    checks["child_parent_verified"] = valid
    return identifier if valid else None


def _delete(context: dict[str, Any], identifier: str) -> bool:
    status, _body = identity._request(
        context, "DELETE", f"/api/session/{identifier}")
    if status not in (200, 204):
        return False
    status, _body = identity._request(context, "GET", f"/api/session/{identifier}")
    return status == 404


def _forked_root_choice(context: dict[str, Any], project: Path,
                        checks: dict[str, bool]) -> tuple[str | None, str | None]:
    """Build a real top-level fork with one schema-verified static message."""
    seed = _session(context, project, "Panelink recovery fork seed", "fork_seed", checks)
    if seed is None:
        return None, None
    exported = identity._export(context, seed)
    info = exported.get("info") if exported else None
    if not isinstance(info, dict) or info.get("id") != seed:
        checks["fork_seed_export_verified"] = False
        return None, None
    info = dict(info)
    info["title"] = "Panelink recovery imported fork parent"
    message = {
        "id": "msg_" + secrets.token_hex(16),
        "type": "synthetic",
        "text": "Panelink private recovery fork fixture; not a prompt.",
        "description": "static fixture transcript seed",
        "time": {"created": int(time.time() * 1000)},
    }
    transcript_hash = identity._messages_digest([message])
    checks["fork_seed_export_verified"] = transcript_hash is not None
    if transcript_hash is None:
        return None, None
    status, _body = identity._request(context, "DELETE", f"/api/session/{seed}")
    checks["fork_seed_removed_for_import"] = status in (200, 204)
    if not checks["fork_seed_removed_for_import"]:
        return None, None
    status, _body = identity._request(context, "GET", f"/api/session/{seed}")
    checks["fork_seed_absent_before_import"] = status == 404
    if not checks["fork_seed_absent_before_import"]:
        return None, None
    status, payload = identity._request(
        context, "POST", "/api/experimental/session/import",
        {"info": info, "messages": [message],
         "location": {"directory": str(project)}})
    checks["fork_import_route_succeeded"] = status == 200
    imported_id = identity._session_id(payload, top_level=True) if status == 200 else None
    imported = identity._export(context, imported_id) if imported_id else None
    imported_messages = imported.get("messages") if imported else None
    checks["fork_import_roundtrip_exact"] = (
        imported_id == seed and identity._messages_digest(imported_messages) == transcript_hash)
    if not checks["fork_import_roundtrip_exact"] or imported_id is None:
        return None, None
    status, payload = identity._request(
        context, "POST", f"/api/session/{imported_id}/fork", {})
    fork_id = identity._session_id(payload, top_level=True) if status == 200 else None
    checks["root_choice_created"] = fork_id is not None and fork_id != imported_id
    if not checks["root_choice_created"] or fork_id is None:
        return None, None
    status, payload = identity._request(context, "GET", f"/api/session/{fork_id}")
    fork_info = identity._data(payload) if status == 200 else None
    origin = fork_info.get("fork") if isinstance(fork_info, dict) else None
    boundary = origin.get("boundary") if isinstance(origin, dict) else None
    checks["fork_parent_link_verified"] = bool(
        isinstance(origin, dict) and origin.get("sessionID") == imported_id
        and isinstance(boundary, dict) and boundary.get("type") == "through"
        and boundary.get("messageID") == message["id"])
    checks["fork_child_is_top_level"] = (
        identity._lookup(context, fork_id, project, top_level=True)
        and identity._session_row_matches(fork_info, fork_id, project, top_level=True))
    fork_export = identity._export(context, fork_id)
    fork_messages = fork_export.get("messages") if fork_export else None
    checks["fork_history_hash_verified"] = bool(
        identity._fork_history_digest(fork_messages)
        == identity._fork_history_digest([message])
        and isinstance(fork_messages, list) and len(fork_messages) == 1
        and fork_messages[0].get("id") != message["id"])
    checks["root_choice_info_verified"] = bool(
        checks["fork_parent_link_verified"] and checks["fork_child_is_top_level"]
        and checks["fork_history_hash_verified"])
    return (fork_id, imported_id) if checks["root_choice_info_verified"] else (None, imported_id)


def _cli(binary: Path, verb: str, directory: Path, project: Path,
         target: str, *, session_id: str | None = None,
         ticket: str | None = None, revision: int | None = None,
         timeout: float = 12.0) -> dict[str, Any]:
    argv = [str(binary), "provider-binding", verb,
            "--directory", str(directory), "--target", target,
            "--project", str(project)]
    if verb == "prepare" and session_id is not None:
        argv.extend(["--session", session_id])
    elif verb == "resume" and ticket is not None and revision is not None:
        argv.extend(["--ticket", ticket, "--revision", str(revision)])
    env = {"PATH": os.defpath, "HOME": str(directory.parent), "LANG": "C", "LC_ALL": "C"}
    try:
        completed = subprocess.run(
            argv, cwd=project, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            timeout=timeout, check=False, close_fds=True, shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        return {"valid": False, "reason": "command_unavailable"}
    if len(completed.stdout) > MAX_CLI_BYTES:
        return {"valid": False, "reason": "response_oversized"}
    try:
        value = json.loads(completed.stdout)
    except (ValueError, UnicodeDecodeError):
        return {"valid": False, "reason": "response_invalid"}
    if not isinstance(value, dict):
        return {"valid": False, "reason": "response_invalid"}
    # Keep only allowlisted response fields. Session IDs and tickets stay in
    # this process's local variables and never enter the emitted report.
    clean = {
        "valid": True,
        "returncode": completed.returncode,
        "state": value.get("state"),
        "error": value.get("error"),
        "revision": value.get("revision"),
        "selected_confirmed": value.get("selected_confirmed"),
        "automatic_resume": value.get("automatic_resume"),
        "current_at_crash": value.get("current_at_crash"),
        "ticket": value.get("ticket"),
        "managed_launch_linked": value.get("managed_launch_linked"),
        "provider_version": value.get("provider_version"),
        "binding_revision": value.get("binding_revision"),
        "provider_session_id": value.get("provider_session_id"),
    }
    return clean


def _target(handle: bridge_probe.ServeProcess) -> str | None:
    return bridge_probe.safe_target(handle.expected_target)


def _serve(context: dict[str, Any], binary: Path, root: Path,
           session_id: str, checks: dict[str, bool]
           ) -> tuple[bridge_probe.ServeProcess | None, Path | None, str]:
    global _FIXTURE_STAGE
    _FIXTURE_STAGE = "managed_terminfo_setup"
    existing_terminfo = root / "terminfo"
    if existing_terminfo.exists():
        try:
            info = existing_terminfo.lstat()
            terminfo = (existing_terminfo if existing_terminfo.is_dir()
                        and not existing_terminfo.is_symlink()
                        and info.st_uid == os.geteuid()
                        and info.st_mode & 0o7777 == 0o700 else None)
        except OSError:
            terminfo = None
    else:
        terminfo = bridge_probe.compile_terminfo(root)
    if terminfo is None:
        return None, None, "private_terminfo_compilation_failed"
    _FIXTURE_STAGE = "managed_directory_setup"
    directory = root / ("managed-" + hashlib.sha256(session_id.encode()).hexdigest()[:8])
    directory.mkdir(mode=0o700)
    metadata = directory.stat()
    checks["managed_directory_private"] = (
        metadata.st_uid == os.geteuid() and metadata.st_mode & 0o7777 == 0o700)
    max_socket = directory / "provider-bridge-18446744073709551615" / "bridge.sock"
    checks["unix_socket_path_bounded"] = len(os.fsencode(max_socket)) <= 103
    checks["pinned_provider_hash_matches_launch"] = (
        bootstrap.sha256_file(Path(context["executable"])) == EXPECTED_SHA256)
    env = dict(context["env"])
    env.update({"TERM": "xterm-256color",
                "OPENCODE_SERVER_PASSWORD": context["password"]})
    env.pop("TERMINFO", None)
    argv = [str(binary), "serve", "--directory", str(directory),
            "--terminfo", str(terminfo), "--initialize", "--allow-input",
            "--opencode-bridge", "--", context["executable"], "--server",
            context["base"], "--session", session_id, str(context["project"])]
    _FIXTURE_STAGE = "managed_process_start"
    handle = bridge_probe.ServeProcess(argv, context["project"], env, expected_target="")
    _FIXTURE_STAGE = "managed_startup_wait"
    deadline = min(context["deadline"], time.monotonic() + bridge_probe.CHILD_DEADLINE)
    while time.monotonic() < deadline and handle.startup is None and handle.process.poll() is None:
        handle.poll(0.05)
    if handle.startup is None or handle.invalid_line:
        reason = handle.failure if handle.failure in {
            "provider_bridge_launch_unavailable", "listener_permission_denied",
            "server_directory_invalid", "server_storage_unavailable",
            "server_start_failed", "launch_context_invalid", "terminfo_invalid",
            "listener_unavailable", "signal_registration_failed",
        } else "startup_output_invalid" if handle.invalid_line else "startup_unverified"
        handle.close(min(context["deadline"], time.monotonic() + 3.0))
        return None, directory, reason
    return handle, directory, "running"


def _wait_initial(handle: bridge_probe.ServeProcess,
                  deadline: float) -> dict[str, Any] | None:
    while time.monotonic() < deadline and handle.process.poll() is None:
        handle.poll(0.05)
        current = handle.latest
        if current is not None and current["status"] == "last_confirmed":
            return current
    return None


def _wait_revision(handle: bridge_probe.ServeProcess, revision: int,
                   deadline: float) -> dict[str, Any] | None:
    """Require a newly received observer frame for the requested revision."""
    while time.monotonic() < deadline and handle.process.poll() is None:
        handle.poll(0.05)
        current = handle.latest
        if (current is not None and current["status"] == "last_confirmed"
                and current["last_confirmed_revision"] == revision):
            return current
    return None


def _stable_source(handle: bridge_probe.ServeProcess, binary: Path,
                   directory: Path, project: Path, target: str,
                   expected_revision: int, source_id: str,
                   checks: int = 3) -> bool:
    """Check the fixture-owned atomic record while the live writer is held.

    The public historical CLI correctly refuses to open the store while the
    live owner holds its exclusive lock. This bounded test-only parser reads
    the atomic current file directly and validates only enough of the public
    on-disk codec to compare the expected source hash/revision/scope.
    """
    source_hash = _identifier_hash(source_id)
    for _ in range(checks):
        handle.poll(0.05)
        if handle.process.poll() is not None:
            return False
        if (handle.latest is None or handle.latest["status"] != "last_confirmed"
                or handle.latest["last_confirmed_revision"] != expected_revision):
            return False
        record = bridge_probe.read_live_managed_binding(
            directory, target, project, source_id)
        if (record is None or record["revision"] != expected_revision
                or record["provider_session_hash"] != source_hash
                or record["provider_version"] != EXPECTED_VERSION):
            return False
    return True


def _no_user_messages(context: dict[str, Any], ids: list[str]) -> bool:
    if not ids or len(set(ids)) != len(ids):
        return False
    for identifier in ids:
        if identity._identifier(identifier) is None:
            return False
        ok, status = identity._empty_user_messages(context, identifier)
        if not ok or status != 200:
            return False
    return True


def managed_recovery(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    global _FIXTURE_STAGE
    _FIXTURE_STAGE = "fixture_setup"
    result: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "invalid_choice_errors": {}, "primary_source_preserved": False,
        "prepared_status_verified": False, "explicit_resume_confirmed": False,
        "duplicate_resume_rejected": False, "historical_managed_linked": False,
        "native_root_choice_verified": False, "no_user_messages": False,
        "prompt_submissions": 0, "no_prompt_submissions": True,
        "attempted_control_count": 0, "explicit_resume_attempted": False,
        "serve_lines_valid": True, "cleanup_completed": False,
        "fixture_setup_checks": {},
    }
    root: Path = context["root"]
    project: Path = context["project"]
    other_project = root / "other-project"
    other_project.mkdir(mode=0o700)
    checks: dict[str, bool] = {}
    source = _session(context, project, "Panelink manual recovery source A", "source", checks)
    choice = _session(context, project, "Panelink manual recovery choice B", "choice", checks)
    deleted = _session(context, project, "Panelink manual recovery deleted", "deleted", checks)
    foreign = _session(context, other_project, "Panelink manual recovery foreign", "foreign", checks)
    parent = _session(context, project, "Panelink manual recovery parent", "parent", checks)
    child = _child(context, parent, project, checks) if parent else None
    root_source = _session(context, project, "Panelink manual recovery root source", "root_source", checks)
    root_choice, fork_parent = _forked_root_choice(context, project, checks)
    result["fixture_setup_checks"] = checks
    all_ids = [value for value in (source, choice, deleted, foreign, parent, child,
                                   root_source, fork_parent, root_choice) if value is not None]
    result["fixture_setup_complete"] = len(all_ids) == 9
    deleted_empty = bool(deleted and identity._empty_user_messages(context, deleted)[0])
    checks["deleted_user_messages_empty"] = deleted_empty
    checks["deleted_removed_and_absent"] = bool(deleted and _delete(context, deleted))
    if (not source or not choice or not deleted or not deleted_empty or not foreign
            or not parent or not child or not root_source or not root_choice
            or not checks["deleted_removed_and_absent"]):
        result["reason"] = "native_fixture_session_setup_failed"
        return result
    result["deleted_fixture_verified"] = True
    result["deleted_fixture_had_no_user_messages"] = deleted_empty
    handles: list[bridge_probe.ServeProcess] = []
    directories: list[Path] = []
    primary_target: str | None = None
    primary_directory: Path | None = None
    primary_revision: int | None = None
    ticket: str | None = None
    prepared_revision: int | None = None
    stable_source_checks = 0
    try:
        handle, directory, serve_status = _serve(context, binary, root, source, checks)
        if handle is None or directory is None:
            result["serve_startup_status"] = serve_status
            result["reason"] = "primary_managed_server_start_failed"
            return result
        handles.append(handle)
        _FIXTURE_STAGE = "primary_observation"
        directories.append(directory)
        primary_directory = directory
        primary_target = _target(handle)
        initial = _wait_initial(handle, min(context["deadline"], time.monotonic() + 16.0))
        if not primary_target or initial is None:
            result["reason"] = "primary_source_not_last_confirmed"
            return result
        primary_revision = initial["last_confirmed_revision"]
        if primary_revision != 1:
            result["reason"] = "primary_initial_revision_unexpected"
            return result
        result["primary_initial_revision_is_one"] = True

        missing_id = "ses_missing_" + secrets.token_hex(8)
        missing_status, _missing_body = identity._request(
            context, "GET", f"/api/session/{missing_id}")
        checks["synthetic_missing_id_absent"] = missing_status == 404
        if missing_status != 404:
            result["reason"] = "native_missing_fixture_not_absent"
            return result
        bad_cases = (
            ("missing", missing_id, "provider_session_not_found"),
            ("deleted", deleted, "provider_session_not_found"),
            ("foreign", foreign, "provider_project_mismatch"),
            ("child", child, "provider_session_is_child"),
        )
        expected_matches: dict[str, bool] = {}
        safe_manual: dict[str, bool] = {}
        for label, identifier, expected_error in bad_cases:
            _FIXTURE_STAGE = "invalid_" + label + "_cli"
            cli = _cli(binary, "prepare", directory, project, primary_target,
                       session_id=identifier, timeout=6.0)
            _FIXTURE_STAGE = "invalid_" + label + "_classification"
            expected_matches[label] = _manual_error(cli, expected_error)
            safe_manual[label] = (
                cli.get("valid") is True and cli.get("returncode") == 1
                and cli.get("state") == "manual_selection_required"
                and cli.get("error") in {
                    "provider_session_not_found", "provider_project_mismatch",
                    "provider_session_is_child", "provider_version_unsupported",
                    "provider_choice_unverified", "recovery_interrupted_or_already_spent",
                }
                and cli.get("automatic_resume") is False)
            result["invalid_choice_errors"][label] = (
                cli.get("error") if isinstance(cli.get("error"), str)
                and cli.get("error") in {
                    "provider_session_not_found", "provider_project_mismatch",
                    "provider_session_is_child", "provider_version_unsupported",
                    "provider_choice_unverified", "recovery_interrupted_or_already_spent",
                } else "fixed_manual_error_unavailable")
            _FIXTURE_STAGE = "invalid_" + label + "_source_check"
            stable = _stable_source(
                handle, binary, directory, project, primary_target, primary_revision, source)
            stable_source_checks += 3
            if not stable:
                result["reason"] = "source_changed_during_invalid_choice_checks"
                return result
        result["all_invalid_choices_manual"] = all(safe_manual.values())
        result["missing_lookup_classified"] = expected_matches["missing"]
        result["deleted_lookup_classified"] = expected_matches["deleted"]
        result["foreign_lookup_classified"] = expected_matches["foreign"]
        result["child_lookup_classified"] = expected_matches["child"]
        result["safe_manual_fallback_for_uncertain_lookup"] = (
            safe_manual["missing"] and result["invalid_choice_errors"]["missing"]
            == "provider_choice_unverified")

        _FIXTURE_STAGE = "prepare_choice"
        prepared = _cli(binary, "prepare", directory, project, primary_target,
                        session_id=choice, timeout=8.0)
        ticket_value = prepared.get("ticket")
        revision_value = prepared.get("revision")
        if (prepared.get("valid") is not True
                or prepared.get("state") != "prepared_choice_saved"
                or prepared.get("selected_confirmed") is not False
                or prepared.get("automatic_resume") is not False
                or not isinstance(ticket_value, str) or len(ticket_value) != 32
                or any(char not in "0123456789abcdef" for char in ticket_value)
                or not isinstance(revision_value, int) or revision_value <= primary_revision):
            result["reason"] = "valid_choice_prepare_not_saved"
            return result
        ticket, prepared_revision = ticket_value, revision_value
        result["prepared_revision"] = prepared_revision
        status = _cli(binary, "status", directory, project, primary_target, timeout=5.0)
        result["prepared_status_verified"] = bool(
            status.get("valid") is True and status.get("state") == "prepared_choice_saved"
            and status.get("ticket") == ticket and status.get("revision") == prepared_revision
            and status.get("selected_confirmed") is False)
        result["source_record_preserved_before_resume"] = _stable_source(
            handle, binary, directory, project, primary_target, primary_revision,
            source, checks=1)
        stable = _stable_source(
            handle, binary, directory, project, primary_target, primary_revision,
            source, checks=4)
        stable_source_checks += 4
        result["primary_source_preserved"] = bool(
            result["prepared_status_verified"] and result["source_record_preserved_before_resume"]
            and stable)
        result["primary_source_unchanged_multiple_checks"] = stable_source_checks >= 12
        if not result["primary_source_preserved"]:
            result["reason"] = "prepare_changed_source_or_status"
            return result

        # The resume CLI is the separate, explicit user action. It must wait
        # for the source to revalidate and a fresh selected snapshot to save.
        result["attempted_control_count"] += 1
        result["explicit_resume_attempted"] = True
        _FIXTURE_STAGE = "resume_choice"
        resumed = _cli(binary, "resume", directory, project, primary_target,
                       ticket=ticket, revision=prepared_revision, timeout=12.0)
        result["explicit_resume_confirmed"] = bool(
            resumed.get("valid") is True and resumed.get("state") == "selected_and_saved"
            and resumed.get("selected_confirmed") is True
            and resumed.get("revision") == prepared_revision)
        if not result["explicit_resume_confirmed"]:
            result["reason"] = "explicit_resume_not_confirmed"
            return result
        status = _cli(binary, "status", directory, project, primary_target, timeout=5.0)
        result["post_resume_status_confirmed"] = bool(
            status.get("valid") is True and status.get("state") == "selected_and_saved"
            and status.get("selected_confirmed") is True
            and status.get("revision") == prepared_revision)

        after_resume = _wait_revision(
            handle, prepared_revision, min(context["deadline"], time.monotonic() + 6.0))
        result["selected_choice_snapshot"] = bool(
            after_resume is not None and after_resume["status"] == "last_confirmed"
            and after_resume["last_confirmed_revision"] == prepared_revision)
        before_duplicate_revision = (after_resume or {}).get("last_confirmed_revision")
        duplicate = _cli(binary, "resume", directory, project, primary_target,
                         ticket=ticket, revision=prepared_revision, timeout=6.0)
        result["duplicate_resume_rejected"] = _manual_error(
            duplicate, "recovery_interrupted_or_already_spent")
        result["duplicate_did_not_advance_revision"] = bool(
            handle.latest is not None
            and handle.latest["last_confirmed_revision"] == before_duplicate_revision)
        if not (result["post_resume_status_confirmed"] and result["selected_choice_snapshot"]
                and result["duplicate_resume_rejected"]
                and result["duplicate_did_not_advance_revision"]):
            result["reason"] = "resume_or_one_shot_gate_failed"
            return result

        # Run the second managed target with an API-created source and a
        # nonempty static-transcript fork. This verifies native top-level
        # classification for fork-origin roots, not just new roots.
        # Close the first fixture owner before starting its sibling so the
        # native client/store locks are never concurrently held for one HOME.
        if not handle.close(min(context["deadline"], time.monotonic() + 6.0)):
            result["reason"] = "primary_managed_server_cleanup_failed"
            return result
        _FIXTURE_STAGE = "second_root_choice"
        second, second_directory, serve_status = _serve(context, binary, root, root_source, checks)
        if second is None or second_directory is None:
            result["serve_startup_status"] = serve_status
            result["reason"] = "second_managed_server_start_failed"
            return result
        handles.append(second)
        directories.append(second_directory)
        second_target = _target(second)
        second_initial = _wait_initial(second, min(context["deadline"], time.monotonic() + 12.0))
        if not second_target or second_initial is None:
            result["reason"] = "second_source_not_last_confirmed"
            return result
        second_revision = second_initial["last_confirmed_revision"]
        second_prepared = _cli(binary, "prepare", second_directory, project, second_target,
                               session_id=root_choice, timeout=8.0)
        root_ticket = second_prepared.get("ticket")
        root_revision = second_prepared.get("revision")
        if (second_prepared.get("valid") is not True
                or second_prepared.get("state") != "prepared_choice_saved"
                or second_prepared.get("selected_confirmed") is not False
                or not isinstance(root_ticket, str) or len(root_ticket) != 32
                or not isinstance(root_revision, int) or root_revision <= second_revision):
            result["reason"] = "native_root_choice_not_verified"
            return result
        root_resumed = _cli(binary, "resume", second_directory, project, second_target,
                            ticket=root_ticket, revision=root_revision, timeout=12.0)
        result["native_root_choice_verified"] = bool(
            root_resumed.get("valid") is True and root_resumed.get("state") == "selected_and_saved"
            and root_resumed.get("selected_confirmed") is True
            and root_resumed.get("revision") == root_revision)
        result["root_revision"] = root_revision
        if not result["native_root_choice_verified"]:
            result["reason"] = "native_root_resume_not_confirmed"
            return result

        result["all_user_ids_sha256"] = sorted(_identifier_hash(value) for value in all_ids)
        # IDs made in the second fixture are included in `all_ids`; check each
        # only after all native operations, without retaining message bodies.
        result["no_user_messages"] = _no_user_messages(
            context, [identifier for identifier in all_ids if identifier != deleted])
        result["no_prompt_submissions"] = result["prompt_submissions"] == 0
        result["explicit_resume_attempted"] = result["attempted_control_count"] == 1
        result["classification"] = "proven"
        result["reason"] = "explicit_recovery_gates_observed"
        if not all(expected_matches.values()):
            result["classification"] = "blocked"
            result["reason"] = "provider_lookup_status_classification_incomplete"
        return result
    finally:
        cleanup_ok = True
        for handle in reversed(handles):
            result["serve_lines_valid"] = result["serve_lines_valid"] and not handle.invalid_line
            cleanup_ok = handle.close(
                min(context["deadline"], time.monotonic() + 6.0)) and cleanup_ok
        result["cleanup_completed"] = cleanup_ok and all(
            handle.process.poll() is not None for handle in handles)
        if primary_directory is not None and primary_target is not None and primary_revision is not None:
            historical = bridge_probe.historical_binding(
                binary, primary_directory, project, primary_target)
            result["historical_managed_linked"] = bool(
                historical is not None and historical["managed_launch_linked"]
                and historical["provider_session_hash"] == _identifier_hash(choice or "")
                and historical["provider_version"] == EXPECTED_VERSION
                and historical["current_at_crash"] is False
                and historical["automatic_resume"] is False
                and historical["binding_revision"] == prepared_revision)
        if (result.get("classification") == "proven"
                and (not result["cleanup_completed"] or not result["historical_managed_linked"]
                     or not result["no_user_messages"] or not result["serve_lines_valid"])):
            result.update(classification="blocked", reason="cleanup_or_historical_gate_failed")


def _callback(binary: Path):
    def run(context: dict[str, Any]) -> dict[str, Any]:
        try:
            return managed_recovery(context, binary)
        except TimeoutError:
            return {"classification": "blocked", "reason": "fixture_adapter_timeout",
                    "cleanup_completed": False, "no_prompt_submissions": True,
                    "fixture_stage": _FIXTURE_STAGE}
        except subprocess.SubprocessError:
            return {"classification": "blocked", "reason": "fixture_adapter_process_error",
                    "cleanup_completed": False, "no_prompt_submissions": True,
                    "fixture_stage": _FIXTURE_STAGE}
        except OSError:
            return {"classification": "blocked", "reason": "fixture_adapter_os_error",
                    "cleanup_completed": False, "no_prompt_submissions": True,
                    "fixture_stage": _FIXTURE_STAGE}
        except Exception as error:
            error_reason = {
                "AttributeError": "fixture_adapter_attribute_error",
                "AssertionError": "fixture_adapter_assertion_error",
                "KeyError": "fixture_adapter_key_error",
                "TypeError": "fixture_adapter_type_error",
                "ValueError": "fixture_adapter_value_error",
            }.get(type(error).__name__, "fixture_adapter_unexpected_error")
            return {"classification": "blocked", "reason": error_reason,
                    "fixture_stage": _FIXTURE_STAGE,
                    "cleanup_completed": False, "no_prompt_submissions": True}
    return run


def _worker(binary: Path, executable: str) -> tuple[int, dict[str, Any]]:
    # The existing bootstrap helper owns version pinning, isolated HOME, API
    # startup, and teardown. Raise its in-process deadline for this larger
    # multi-target acceptance while preserving the bounded outer worker.
    bootstrap.TOTAL_DEADLINE_SECONDS = MAX_PROBE_SECONDS
    # BridgeLaunch binds an AF_UNIX socket under its server directory and
    # correctly enforces the platform's 103-byte pathname limit. Keep the
    # owned bootstrap fixture root short enough for that private socket path.
    tempfile.tempdir = "/private/tmp"
    original_temporary_directory = tempfile.TemporaryDirectory

    def short_private_tempdir(*_args, **_kwargs):
        return original_temporary_directory(prefix="p4m-", dir="/private/tmp")

    tempfile.TemporaryDirectory = short_private_tempdir
    result = bootstrap._run_worker(
        executable, "opencode", after_bootstrap=_callback(binary),
        total_deadline_seconds=MAX_PROBE_SECONDS, scenario="manual-recovery")
    nested = result.get("tui") if isinstance(result, dict) else None
    ok = (isinstance(nested, dict) and nested.get("classification") == "proven"
          and nested.get("cleanup_completed") is True
          and nested.get("historical_managed_linked") is True
          and nested.get("no_user_messages") is True
          and nested.get("prompt_submissions") == 0)
    return (0 if ok and result.get("cleanup_completed") is True else 2), result


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
        return 2, report
    if not binary.is_file() or not os.access(binary, os.X_OK):
        report["reason"] = "panelink_binary_unavailable"
        return 2, report
    report["panelink_binary_hash"] = bridge_probe.sha256_file(binary)
    report["pinned_opencode_hash_match"] = bootstrap.sha256_file(Path(chosen)) == EXPECTED_SHA256
    if not report["pinned_opencode_hash_match"]:
        report["reason"] = "pinned_opencode_hash_mismatch"
        return 2, report
    if sys.platform != "darwin" or shutil.which("sandbox-exec") is None:
        report["reason"] = "macos_loopback_sandbox_unavailable"
        return 2, report
    cwd = Path(__file__).resolve().parents[1]
    with tempfile.TemporaryDirectory(prefix="panelink-manual-recovery-sandbox-") as temporary:
        sandbox_root = Path(temporary)
        sandbox_root.chmod(0o700)
        profile = sandbox_root / "loopback.sb"
        profile.write_text(bridge_probe.MANAGED_SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C",
               "TMPDIR": "/private/tmp", bootstrap.WORKER_ENV: "1"}
        argv = [shutil.which("sandbox-exec") or "sandbox-exec", "-f", str(profile),
                sys.executable, str(Path(__file__).resolve()), "--sandbox-worker",
                str(binary), chosen]
        code, raw, sandbox_error = bootstrap.bounded_worker(
            argv, cwd, env, timeout=MAX_PROBE_SECONDS + 5.0)
    try:
        parsed = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        parsed = {"classification": "blocked",
                  "reason": sandbox_error or "worker_output_invalid"}
    if isinstance(parsed, dict):
        nested = parsed.get("tui")
        if isinstance(nested, dict):
            # Copy only the allowlisted worker evidence, never raw CLI output,
            # fixture paths, identifiers, or the one-shot ticket.
            report.update(_public_evidence(nested))
        report["no_prompt_submissions"] = parsed.get("prompt_submissions") == 0
        report["cleanup_completed"] = (parsed.get("cleanup_completed") is True
                                        and (not isinstance(nested, dict)
                                             or nested.get("cleanup_completed") is True))
    report["sandbox"] = "not_enforced" if sandbox_error else "enforced_loopback_only"
    if sandbox_error:
        report["classification"] = "blocked"
        report["reason"] = "sandbox_worker_failed"
    return (0 if code == 0 and report.get("classification") == "proven"
            and report.get("cleanup_completed") is True else 2), report


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
        code, report = _worker(binary, executable)
    else:
        code, report = run(args.panelink_binary, args.opencode)
    print(json.dumps(report, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
