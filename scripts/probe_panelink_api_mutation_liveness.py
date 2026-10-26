#!/usr/bin/env python3
"""Check managed attach/source liveness during concurrent OpenCode API changes.

Uses the existing pinned native fixture and loopback-only sandbox. API-created
roots/fork/project are built on a worker thread while the owner callback keeps
draining the attach PTY and polling the managed serve process. It then requires
the original A binding to remain exact and performs one guarded native picker
selection. No prompt or model call is made; output is fixed and allowlisted.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any

try:
    from scripts import probe_opencode_v2_identity as bootstrap
    from scripts import probe_panelink_provider_bridge as bridge
except ModuleNotFoundError:
    import probe_opencode_v2_identity as bootstrap
    import probe_panelink_provider_bridge as bridge


MAX_WORKER_SECONDS = 60.0
MUTATION_SECONDS = 24.0
PICKER_SECONDS = 9.0
MUTATION_CLEANUP_JOIN_SECONDS = 0.2
MANAGED_SANDBOX_PROFILE = bridge.MANAGED_SANDBOX_PROFILE


class PumpMetrics:
    def __init__(self) -> None:
        self.last_turn = time.monotonic()
        self.max_gap_ms = 0.0
        self.turns = 0

    def reset(self) -> None:
        self.last_turn = time.monotonic()
        self.max_gap_ms = 0.0
        self.turns = 0

    def record_turn(self) -> None:
        now = time.monotonic()
        self.max_gap_ms = max(self.max_gap_ms, (now - self.last_turn) * 1000.0)
        self.last_turn = now
        self.turns += 1


def pump_once(attach: Any, serves: list[Any], metrics: PumpMetrics,
              timeout: float = 0.02) -> None:
    """Poll bounded serve pipes and yield to the attach's lifetime PTY drainer."""
    metrics.record_turn()
    for serve in serves:
        serve.poll(0.0)
    drained = attach.alive()
    if drained:
        attach.drain(max(0.0, timeout))
    else:
        time.sleep(min(0.01, max(0.0, timeout)))


def pump_until(predicate, deadline: float, attach: Any, serves: list[Any],
               metrics: PumpMetrics, *, stop_if_attach_exits: bool = True) -> bool:
    while time.monotonic() < deadline:
        if predicate():
            return True
        if not attach.alive() and stop_if_attach_exits:
            return False
        pump_once(attach, serves, metrics)
    return bool(predicate())


def promoted_binding_matches(final_binding: dict[str, Any] | None,
                            latest: dict[str, Any] | None,
                            initial_revision: int | None) -> bool:
    """Require the exact durable row and its matching owner completion."""
    return bool(
        final_binding is not None
        and isinstance(final_binding.get("revision"), int)
        and initial_revision is not None
        and final_binding["revision"] > initial_revision
        and latest is not None
        and latest.get("status") == "last_confirmed"
        and latest.get("last_confirmed_revision") == final_binding["revision"]
    )


def _join_mutation_worker(worker: threading.Thread | None,
                          timeout: float = MUTATION_CLEANUP_JOIN_SECONDS) -> bool:
    """Wait briefly for fixture work and report whether it actually stopped."""
    if worker is None:
        return True
    worker.join(timeout=max(0.0, timeout))
    return not worker.is_alive()


def _fixed_result() -> dict[str, Any]:
    return {
        "classification": "blocked",
        "reason": "not_started",
        "managed_serve_started": False,
        "serve_failure": None,
        "serve_exit_code": None,
        "initial_binding_verified": False,
        "api_mutation_finished": False,
        "mutation_worker_terminated": False,
        "api_mutation_verified": False,
        "attach_alive_during_mutation": False,
        "attach_alive_after_mutation": False,
        "serve_alive_after_mutation": False,
        "pane_live_after_mutation": False,
        "owner_status_after_mutation": None,
        "input_owner_after_mutation": None,
        "binding_a_unchanged": False,
        "picker_selection_accepted": False,
        "new_root_binding_confirmed": False,
        "attach_alive_after_picker": False,
        "serve_alive_after_picker": False,
        "pane_live_after_picker": False,
        "input_owner_after_picker": None,
        "provider_status_after_picker": None,
        "provider_revision_after_picker": None,
        "binding_a_still_durable_after_picker": False,
        "picker_heading_after_enter": None,
        "cleanup_completed": False,
        "prompt_submissions": 0,
        "attempted_control_count": 0,
        "user_message_absence_verified": False,
        "mutation_owner_pump_turns": 0,
        "max_owner_pump_gap_ms": None,
        "drain_pump_active_during_mutation": False,
        "attach_output_bytes_during_mutation": 0,
        "initial_revision": None,
        "final_revision": None,
        "binding_hashes_match": False,
        "transition_fixtures": None,
        "picker_trace": None,
    }


def managed_mutation_callback(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    try:
        from scripts.p4_managed_transition_fixtures import (
            FORK_ROOT_TITLE,
            FOREIGN_ROOT_TITLE,
            NEW_ROOT_TITLE,
            create_managed_transition_fixtures,
            safe_evidence,
        )
    except ModuleNotFoundError:
        from p4_managed_transition_fixtures import (
            FORK_ROOT_TITLE,
            FOREIGN_ROOT_TITLE,
            NEW_ROOT_TITLE,
            create_managed_transition_fixtures,
            safe_evidence,
        )

    result = _fixed_result()
    root: Path = context["root"]
    project: Path = context["project"]
    session_a, _session_b = context["session_ids"]
    handle: bridge.ServeProcess | None = None
    attach: bridge.PanelinkAttachPty | None = None
    mutation_thread: threading.Thread | None = None
    metrics = PumpMetrics()
    mutation_done = threading.Event()
    mutation_result: list[tuple[Any, dict[str, Any]] | None] = []
    mutation_error: list[str] = []
    selected = False
    initial_revision: int | None = None
    initial_hash: str | None = None
    try:
        terminfo = bridge.compile_terminfo(root)
        if terminfo is None:
            result["reason"] = "private_terminfo_compilation_failed"
            return result
        child_env = dict(context["env"])
        private_config = bridge.pinned_cli_config_content(Path(context["executable"]))
        if private_config is None:
            result["reason"] = "pinned_cli_keybind_config_unverified"
            return result
        child_env["OPENCODE_CLI_CONFIG_CONTENT"] = private_config
        child_env.update({"TERM": "xterm-256color", "OPENCODE_SERVER_PASSWORD": context["password"]})
        child_env.pop("TERMINFO", None)

        # Keep every nested Unix socket pathname under macOS's 103-byte limit.
        serve_dir = root / "serve-a"
        serve_dir.mkdir(mode=0o700)
        argv = [str(binary), "serve", "--directory", str(serve_dir), "--terminfo",
                str(terminfo), "--initialize", "--allow-input", "--opencode-bridge",
                "--", context["executable"], "--server", context["base"],
                "--session", session_a, str(project)]
        handle = bridge.ServeProcess(argv, project, child_env, expected_target="")
        serves = [handle]
        serve_deadline = min(context["deadline"], time.monotonic() + bridge.CHILD_DEADLINE)
        while time.monotonic() < serve_deadline and handle.startup is None:
            handle.poll(0.05)
            if handle.process.poll() is not None:
                break
        if handle.startup is None or handle.invalid_line:
            result["serve_failure"] = handle.failure
            result["serve_exit_code"] = handle.process.poll()
            result["reason"] = ("managed_serve_" + handle.failure
                                if handle.failure else "managed_serve_startup_unverified")
            return result
        target = handle.expected_target
        result["managed_serve_started"] = True

        attach_context = root / "attach-context"
        attach_context.mkdir(mode=0o700)
        observer_context = root / "screen-observer-context"
        if not bridge.create_screen_observer(binary, serve_dir, observer_context, target):
            result["reason"] = "screen_observer_unavailable"
            return result
        attach_env = {
            "PATH": os.defpath,
            "HOME": child_env.get("HOME", str(root)),
            "ZDOTDIR": child_env.get("HOME", str(root)),
            "PS1": "P4AM$ ",
        }
        attach = bridge.PanelinkAttachPty(
            [str(binary), "attach", "--directory", str(serve_dir),
             "--context-dir", str(attach_context)],
            project, attach_env, attach_context,
        )
        if not pump_until(
            lambda: attach.raw_mode() and bool(attach.output),
            min(context["deadline"], time.monotonic() + 5.0),
            attach, serves, metrics,
        ):
            result["reason"] = "panelink_attach_not_ready"
            return result
        if not pump_until(
            lambda: handle.latest is not None and handle.latest["status"] == "last_confirmed",
            min(context["deadline"], time.monotonic() + 18.0),
            attach, serves, metrics,
        ):
            result["reason"] = "initial_binding_not_confirmed"
            return result
        initial = bridge.read_live_managed_binding(
            serve_dir, target, project, session_a,
        )
        if initial is None or initial["revision"] != handle.latest["last_confirmed_revision"]:
            result["reason"] = "initial_binding_scope_invalid"
            return result
        initial_revision = initial["revision"]
        initial_hash = initial["provider_session_hash"]
        result["initial_binding_verified"] = True
        result["initial_revision"] = initial_revision
        initial_owner = bridge.managed_pane_screen_markers(
            binary, serve_dir, observer_context, target,
        )
        if initial_owner.get("status") != "known" or initial_owner.get("input_owner") != "client":
            result["reason"] = "exact_attach_owner_not_verified_before_mutation"
            return result

        def mutate_api_fixtures() -> None:
            try:
                fixtures, checks = create_managed_transition_fixtures(context, project)
                mutation_result.append((fixtures, checks))
            except (OSError, subprocess.SubprocessError, TimeoutError, ValueError):
                mutation_error.append("fixture_creation_failed")
            except BaseException:
                mutation_error.append("fixture_creation_exception")
            finally:
                mutation_done.set()

        mutation_thread = threading.Thread(
            target=mutate_api_fixtures, name="panelink-api-fixture-mutation", daemon=True,
        )
        mutation_started = time.monotonic()
        metrics.reset()
        before_mutation_output = attach.output_snapshot()[1]
        mutation_thread.start()
        mutation_deadline = min(context["deadline"], mutation_started + MUTATION_SECONDS)
        completed = pump_until(
            mutation_done.is_set, mutation_deadline, attach, serves, metrics,
            stop_if_attach_exits=False,
        )
        # API operations have their own finite HTTP deadlines. Continue pumping
        # until the worker exits or the bootstrap's absolute fixture deadline.
        if not completed and mutation_thread.is_alive():
            completed = pump_until(
                mutation_done.is_set, context["deadline"], attach, serves, metrics,
                stop_if_attach_exits=False,
            )
        result["mutation_worker_terminated"] = _join_mutation_worker(mutation_thread)
        result["attach_alive_during_mutation"] = attach.alive()
        result["api_mutation_finished"] = completed and result["mutation_worker_terminated"]
        result["mutation_owner_pump_turns"] = metrics.turns
        result["max_owner_pump_gap_ms"] = round(metrics.max_gap_ms, 3)
        result["drain_pump_active_during_mutation"] = attach._pump_thread.is_alive()
        result["attach_output_bytes_during_mutation"] = max(
            0, attach.output_snapshot()[1] - before_mutation_output,
        )
        if mutation_result:
            fixtures, checks = mutation_result[0]
            result["transition_fixtures"] = safe_evidence(fixtures, checks)
        if not result["api_mutation_finished"] or not mutation_result or mutation_result[0][0] is None:
            result["reason"] = mutation_error[0] if mutation_error else "api_mutation_not_verified"
            return result
        fixtures, checks = mutation_result[0]
        result["api_mutation_verified"] = all(checks.values())
        if not result["api_mutation_verified"]:
            result["reason"] = "api_mutation_fixture_unverified"
            return result
        if (fixtures.new_root_title != NEW_ROOT_TITLE
                or fixtures.fork_root_title != FORK_ROOT_TITLE
                or fixtures.foreign_root_title != FOREIGN_ROOT_TITLE):
            result["reason"] = "fixture_title_set_unverified"
            return result

        result["attach_alive_after_mutation"] = attach.alive()
        result["serve_alive_after_mutation"] = handle.process.poll() is None
        pane = bridge.managed_pane_status(binary, serve_dir, observer_context, target)
        owner = bridge.managed_pane_screen_markers(
            binary, serve_dir, observer_context, target,
        )
        result["pane_live_after_mutation"] = pane.get("status") == "known" and pane.get("live") is True
        result["owner_status_after_mutation"] = owner.get("status")
        result["input_owner_after_mutation"] = owner.get("input_owner")
        current = bridge.read_live_managed_binding(serve_dir, target, project, session_a)
        result["binding_a_unchanged"] = bool(
            current is not None and current["revision"] == initial_revision
            and current["provider_session_hash"] == initial_hash
            and handle.latest is not None
            and handle.latest["status"] == "last_confirmed"
            and handle.latest["last_confirmed_revision"] == initial_revision
        )
        result["binding_hashes_match"] = bool(
            current is not None and current["provider_session_hash"] == initial_hash
        )
        if not (result["attach_alive_after_mutation"] and result["serve_alive_after_mutation"]
                and result["drain_pump_active_during_mutation"]
                and result["mutation_owner_pump_turns"] > 0
                and result["pane_live_after_mutation"]
                and result["owner_status_after_mutation"] == "known"
                and result["input_owner_after_mutation"] == "client"
                and result["binding_a_unchanged"]):
            result["reason"] = "managed_liveness_or_binding_changed_during_api_mutation"
            return result

        labels = (fixtures.new_root_title, "Panelink V2 Identity A")
        fixture_titles = (
            "Panelink V2 Identity A",
            "Panelink V2 Identity B",
            fixtures.new_root_title,
            fixtures.fork_root_title,
            "Panelink recovery imported fork parent",
            "Panelink recovery fork seed",
            fixtures.foreign_root_title,
        )
        pre_read = bridge.managed_pane_screen_markers(
            binary, serve_dir, observer_context, target,
        )
        fresh_child_read = lambda: bridge.managed_pane_screen_markers(
            binary, serve_dir, observer_context, target,
            labels=labels, fixture_titles=fixture_titles,
            target_label=labels[0], other_label=fixtures.fork_root_title,
        )
        trace: dict[str, Any] = {}
        result["attempted_control_count"] = 1
        selected = bridge.select_session(
            attach, labels[0], labels[1], labels,
            min(context["deadline"], time.monotonic() + PICKER_SECONDS),
            trace, pre_read=pre_read, fresh_child_read=fresh_child_read,
            fixture_titles=fixture_titles, open_chord=b"\x0f",
        )
        result["picker_selection_accepted"] = selected
        result["picker_trace"] = {
            key: value for key, value in trace.items()
            if isinstance(value, (bool, int, float, str))
            and key in {
                "fresh_pre_action_read_valid", "fresh_pre_action_screen_nonempty",
                "picker_open_key", "picker_open_sent", "alive_after_picker_open",
                "alive_after_menu_wait", "output_advanced_after_picker_open",
                "fresh_menu_child_read_valid", "menu_child_screen_positions",
                "target_filter_sent", "target_only_filtered_row_seen",
                "filtered_child_read_valid", "filtered_child_target_visible",
                "filtered_child_competitor_visible", "enter_sent", "enter_write_ok",
            }
        }
        if not selected:
            result["reason"] = "safe_native_picker_selection_not_proven"
            return result
        attach.drain(0.02)
        final_binding = None
        confirmation_deadline = min(context["deadline"], time.monotonic() + 7.0)
        while time.monotonic() < confirmation_deadline and attach.alive():
            pump_once(attach, serves, metrics, 0.02)
            final_binding = bridge.read_live_managed_binding(
                serve_dir, target, project, fixtures.new_root_id,
            )
            if promoted_binding_matches(final_binding, handle.latest, initial_revision):
                break
        result["attach_alive_after_picker"] = attach.alive()
        result["serve_alive_after_picker"] = handle.process.poll() is None
        final_pane = bridge.managed_pane_status(binary, serve_dir, observer_context, target)
        final_screen = bridge.managed_pane_screen_markers(
            binary, serve_dir, observer_context, target,
        )
        result["pane_live_after_picker"] = (
            final_pane.get("status") == "known" and final_pane.get("live") is True
        )
        result["input_owner_after_picker"] = final_screen.get("input_owner")
        result["picker_heading_after_enter"] = final_screen.get("contains_picker_heading")
        if handle.latest is not None:
            result["provider_status_after_picker"] = handle.latest["status"]
            result["provider_revision_after_picker"] = handle.latest["last_confirmed_revision"]
        final_a = bridge.read_live_managed_binding(serve_dir, target, project, session_a)
        result["binding_a_still_durable_after_picker"] = bool(
            final_a is not None and final_a["revision"] == initial_revision
            and final_a["provider_session_hash"] == initial_hash
        )
        result["final_revision"] = final_binding["revision"] if final_binding else None
        result["new_root_binding_confirmed"] = promoted_binding_matches(
            final_binding, handle.latest, initial_revision,
        )
        result["prompt_submissions"] = 0
        result["user_message_absence_verified"] = checks.get("new_root_has_no_user_messages") is True
        if result["new_root_binding_confirmed"]:
            result["classification"] = "proven"
            result["reason"] = "api_mutations_preserved_managed_attach_and_picker"
        else:
            result["reason"] = "new_root_selection_not_durably_confirmed"
        return result
    except (OSError, subprocess.SubprocessError, TimeoutError):
        result["reason"] = "bounded_mutation_probe_failed"
        return result
    finally:
        result["cleanup_completed"] = True
        if attach is not None:
            attach.close()
            result["cleanup_completed"] = result["cleanup_completed"] and not attach.alive()
        if handle is not None:
            serve_closed = handle.close(
                min(context["deadline"], time.monotonic() + 5.0),
            )
            result["cleanup_completed"] = result["cleanup_completed"] and serve_closed
        # Fixture creation may still be inside a finite API request. Do not
        # claim cancellation: wait only a bounded interval and report cleanup
        # incomplete while the worker remains live.
        result["mutation_worker_terminated"] = _join_mutation_worker(mutation_thread)
        result["cleanup_completed"] = (
            result["cleanup_completed"] and result["mutation_worker_terminated"]
        )
        if result["classification"] == "proven" and not result["cleanup_completed"]:
            result.update(classification="blocked", reason="cleanup_unverified")


def callback(context: dict[str, Any], binary: Path) -> dict[str, Any]:
    try:
        return managed_mutation_callback(context, binary)
    except Exception:
        return {**_fixed_result(), "reason": "mutation_callback_failed"}


def run(panelink_binary: str, executable: str | None = None) -> tuple[int, dict[str, Any]]:
    opencode, launcher = bridge._resolve_opencode(executable)
    panelink = Path(panelink_binary).resolve()
    summary: dict[str, Any] = {
        "classification": "blocked", "reason": "not_started",
        "opencode_version": None, "pinned_binary_match": False,
        "sandbox": "not_started", "network_scope": "loopback_only_sandbox",
        "cleanup_completed": False,
    }
    if opencode is None or launcher != "opencode":
        return 2, {**summary, "reason": "native_opencode_missing"}
    if not panelink.is_file() or not os.access(panelink, os.X_OK):
        return 2, {**summary, "reason": "panelink_binary_unavailable"}
    if bootstrap.sha256_file(Path(opencode)) != bootstrap.EXPECTED_SHA256:
        return 2, {**summary, "reason": "pinned_opencode_hash_mismatch"}
    if sys.platform != "darwin":
        return 2, {**summary, "reason": "macos_sandbox_required"}
    sandbox = shutil.which("sandbox-exec")
    if sandbox is None:
        return 2, {**summary, "reason": "sandbox_exec_unavailable"}
    with tempfile.TemporaryDirectory(prefix="panelink-p4-api-mutation-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        profile = root / "loopback.sb"
        profile.write_text(MANAGED_SANDBOX_PROFILE, encoding="utf-8")
        profile.chmod(0o600)
        worker_env = {"PATH": os.defpath, "LANG": "C", "LC_ALL": "C",
                      "TMPDIR": "/private/tmp", bootstrap.WORKER_ENV: "1"}
        argv = [sandbox, "-f", str(profile), sys.executable, str(Path(__file__).resolve()),
                "--sandbox-worker", str(panelink), opencode]
        code, raw, failure = bootstrap.bounded_worker(
            argv, Path(__file__).resolve().parents[1], worker_env,
            timeout=MAX_WORKER_SECONDS + 8.0,
        )
    try:
        value = json.loads(raw)
    except (ValueError, UnicodeDecodeError):
        value = {"classification": "blocked", "reason": failure or "worker_output_invalid"}
    if isinstance(value, dict):
        summary.update(value)
    summary["sandbox"] = "not_enforced" if failure else "enforced_loopback_only"
    if failure:
        summary.update(classification="blocked", reason=failure)
    return (0 if code == 0 and summary.get("classification") == "proven"
            and summary.get("cleanup_completed") is True else 2), summary


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
        binary, opencode = map(Path, args.sandbox_worker)
        result = bootstrap._run_worker(
            str(opencode), "opencode",
            after_bootstrap=lambda context: callback(context, binary),
            total_deadline_seconds=MAX_WORKER_SECONDS,
            scenario="panelink_api_mutation_liveness",
        )
        tui = result.get("tui") if isinstance(result.get("tui"), dict) else {}
        output = {
            "classification": tui.get("classification", "blocked"),
            "reason": tui.get("reason", result.get("reason", "worker_failed")),
            "opencode_version": result.get("version"),
            "pinned_binary_match": result.get("binary_sha256_matches") is True,
            "sandbox": "worker", "network_scope": result.get("network_scope"),
            "cleanup_completed": result.get("cleanup_completed") is True and tui.get("cleanup_completed") is True,
            **{key: tui.get(key) for key in _fixed_result() if key in tui},
        }
        print(json.dumps(output, sort_keys=True, separators=(",", ":")))
        return 0 if output["classification"] == "proven" and output["cleanup_completed"] else 2
    code, result = run(args.panelink_binary, args.opencode)
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
