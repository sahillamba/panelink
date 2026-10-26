from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

from scripts.probe_panelink_provider_bridge import (
    ServeProcess,
    PanelinkAttachPty,
    append_rolling,
    compact_fixed_evidence,
    exact_line_positions,
    picker_heading_position,
    picker_heading_marker,
    managed_cli_config_content,
    pinned_cli_config_content,
    parse_target,
    read_live_managed_binding,
    safe_cli_target,
    sanitized_snapshot,
    select_session,
    session_picker_screen,
    filtered_session_row,
    fresh_child_modal_rows,
    fixture_route_diagnostic,
    fixture_route_matches,
    foreign_refusal_acceptance,
    global_picker_session_result_rows,
    session_picker_heading,
    session_picker_region,
    stable_fresh_child_modal_rows,
)


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/probe_panelink_provider_bridge.py"
TARGET = "71:2:3:4:5"


class PanelinkProviderBridgeProbeTests(unittest.TestCase):
    def test_compact_report_drops_repeated_geometry_under_worker_limit(self) -> None:
        repeated_positions = {
            "heading": [[2, 4]], "target": [[3, 8], [7, 8]],
            **{f"fixture_label_{index}": [[7 + index, 10]] for index in range(7)},
        }
        report = {
            "classification": "blocked", "prompt_submissions": 0,
            "switch_trace": [{
                "enter_sent": True, "filtered_child_row_layout_safe": True,
                "screen_positions": repeated_positions,
                "filtered_child_screen_positions": repeated_positions,
                "screen_row_widths": list(range(24)),
            } for _ in range(12)],
        }
        encoded = json.dumps(compact_fixed_evidence(report), separators=(",", ":"))
        self.assertLess(len(encoded.encode("utf-8")), 16 * 1024)
        self.assertNotIn("screen_positions", encoded)
        self.assertNotIn("screen_row_widths", encoded)
        self.assertIn('"enter_sent":true', encoded)

    def test_pty_transcript_retains_newest_bounded_suffix(self) -> None:
        retained = bytearray(b"old")
        append_rolling(retained, b"-new-content", 8)
        self.assertEqual(retained, b"-content")
        append_rolling(retained, b"123456789", 4)
        self.assertEqual(retained, b"6789")

    def test_startup_stderr_projection_discards_raw_text(self) -> None:
        process = ServeProcess.__new__(ServeProcess)
        process.stderr_capture = bytearray(b"permission denied for /private/fixture/secret")
        self.assertEqual(process.stderr_category(), "permission_denied")
        self.assertEqual(process.stderr_capture, bytearray())

    def test_serve_capture_drains_buffered_diagnostics_after_exit(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-exit-drain-") as temporary:
            root = Path(temporary)
            script = root / "failed-serve.py"
            script.write_text(
                "import sys\n"
                "print('{\"ok\":false,\"error\":\"listener_permission_denied\"}', flush=True)\n"
                "print('permission denied /private/fixture/secret', file=sys.stderr, flush=True)\n",
                encoding="utf-8",
            )
            handle = ServeProcess([sys.executable, str(script)], root,
                                  {"PATH": os.defpath}, expected_target="")
            try:
                self.assertEqual(handle.process.wait(timeout=2), 0)
                deadline = time.monotonic() + 2
                while time.monotonic() < deadline and handle.selector.get_map():
                    handle.poll(0.02)
                self.assertEqual(handle.failure, "listener_permission_denied")
                self.assertEqual(handle.event_count, 1)
                self.assertEqual(handle.stderr_category(), "permission_denied")
                self.assertNotIn("secret", repr(handle))
            finally:
                handle.selector.close()
                for stream in (handle.process.stdout, handle.process.stderr):
                    if stream is not None and not stream.closed:
                        stream.close()

    def test_managed_attach_pump_drains_while_main_callback_blocks(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-drain-pump-") as temporary:
            root = Path(temporary)
            child_ready = root / "child-ready"
            child_release = root / "child-release"
            attach = PanelinkAttachPty(
                ["/bin/sh", "-c",
                 'printf ready > "$PANELINK_TEST_READY"; '
                 'while [ ! -f "$PANELINK_TEST_RELEASE" ]; do /bin/sleep 0.01; done; '
                 'i=0; while [ "$i" -lt 8 ]; do printf x; i=$((i+1)); done'],
                root, {"PATH": os.defpath, "HOME": str(root), "LANG": "C",
                       "PANELINK_TEST_READY": str(child_ready),
                       "PANELINK_TEST_RELEASE": str(child_release)}, root,
            )
            callback_entered = threading.Event()
            callback_released = threading.Event()
            pump_observed_all = threading.Event()

            def observe_pump_during_callback() -> None:
                if not callback_entered.wait(2.0):
                    return
                deadline = time.monotonic() + 2.0
                while time.monotonic() < deadline:
                    output, _total_bytes = attach.output_snapshot()
                    if output.count(b"x") >= 8:
                        pump_observed_all.set()
                        callback_released.set()
                        return
                    time.sleep(0.005)

            observer = threading.Thread(target=observe_pump_during_callback, daemon=True)
            observer.start()
            try:
                ready_deadline = time.monotonic() + 2.0
                while time.monotonic() < ready_deadline and not child_ready.exists():
                    time.sleep(0.005)
                self.assertTrue(child_ready.exists(), "PTY child did not reach its output gate")
                # Simulate a blocking control-read/API callback on the owner
                # thread. The independent PTY reader must drain all output
                # before the callback is allowed to return.
                callback_entered.set()
                child_release.write_text("release\n", encoding="ascii")
                self.assertTrue(callback_released.wait(2.0), "pump did not drain during callback")
                self.assertTrue(pump_observed_all.is_set())
                output, total_bytes = attach.output_snapshot()
                self.assertGreaterEqual(total_bytes, 8)
                self.assertGreaterEqual(output.count(b"x"), 8)
                self.assertLessEqual(len(output), 512 * 1024)
            finally:
                child_release.touch(exist_ok=True)
                callback_released.set()
                observer.join(timeout=2.0)
                attach.close()
            self.assertFalse(attach._pump_thread.is_alive())

    def test_picker_guard_accepts_generic_heading_only_with_exact_rows(self) -> None:
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")
        self.assertTrue(session_picker_screen("Sessions for project\n" + "\n".join(labels), labels))
        self.assertFalse(session_picker_screen("Sessions for project\n" + labels[0], labels))
        self.assertTrue(filtered_session_row("Sessions for project\n" + labels[1], labels[1], labels[0]))
        self.assertFalse(filtered_session_row("Sessions for project\n" + "\n".join(labels), labels[1], labels[0]))
        # The current session title can remain above the modal while the
        # picker itself has a unique filtered result.
        self.assertTrue(filtered_session_row(
            labels[0] + "\nSessions for project\n" + labels[1], labels[1], labels[0],
        ))

    def test_private_config_uses_only_pinned_v2_command_ids(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-keybind-artifact-") as temporary:
            binary = Path(temporary) / "opencode"
            binary.write_bytes(
                b"OPENCODE_CLI_CONFIG_CONTENT cli.json session.list open.menu session.first session.new session.fork"
            )
            config = pinned_cli_config_content(binary)
            self.assertIsNotNone(config)
            value = json.loads(config)
            self.assertEqual(value["keybinds"], {
                "session.list": "ctrl+p",
                "open.menu": "ctrl+o",
                "session.first": "none",
                "command.palette.show": "none",
                "session.new": "ctrl+n",
                "session.fork": "ctrl+r",
            })
            binary.write_bytes(b"cli.json session_list")
            self.assertIsNone(pinned_cli_config_content(binary))

    def test_managed_config_retains_production_plugin_and_adds_observer(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-plugin-compose-") as temporary:
            root = Path(temporary)
            binary = root / "opencode"
            binary.write_bytes(
                b"OPENCODE_CLI_CONFIG_CONTENT cli.json session.list open.menu "
                b"session.first session.new session.fork"
            )
            bridge = root / "provider-bridge-7"
            config_dir = bridge / "opencode-config"
            production_plugin = bridge / "identity-plugin"
            observer = root / "route-observer"
            config_dir.mkdir(parents=True)
            production_plugin.mkdir()
            observer.mkdir()

            content = managed_cli_config_content(binary, observer)
            self.assertIsNotNone(content)
            config = json.loads(content)
            self.assertEqual(config["plugins"], [
                "../identity-plugin", str(observer.resolve()),
            ])
            # Exercise the documented source-relative path rule against the
            # actual production launch layout; a replacement observer-only
            # array would fail this assertion.
            resolved_production = (config_dir / config["plugins"][0]).resolve()
            self.assertEqual(resolved_production, production_plugin.resolve())
            self.assertTrue(resolved_production.is_dir())
            self.assertTrue(Path(config["plugins"][1]).is_dir())
            self.assertEqual(config["keybinds"]["session.list"], "ctrl+p")
            self.assertEqual(config["keybinds"]["open.menu"], "ctrl+o")

    def test_foreign_route_requires_exact_same_client_new_sequence(self) -> None:
        project_hash = "a" * 64
        session_hash = "b" * 64
        instance = "c" * 32
        record = {
            "route_name": "session", "session_top_level": True,
            "project_hash": project_hash, "session_id_hash": session_hash,
            "tui_instance": instance, "seq": 9,
        }
        self.assertTrue(fixture_route_matches(
            record, project_hash, session_hash, instance=instance, after_seq=8,
        ))
        for altered in (
            {**record, "project_hash": "d" * 64},
            {**record, "session_id_hash": "e" * 64},
            {**record, "session_top_level": False},
            {**record, "tui_instance": "f" * 32},
            {**record, "seq": 8},
        ):
            self.assertFalse(fixture_route_matches(
                altered, project_hash, session_hash, instance=instance, after_seq=8,
            ))

    def test_foreign_refusal_uses_exact_observer_and_public_refusal_not_title(self) -> None:
        checks = {
            "selected_foreign": True,
            "exact_foreign_route": True,
            "same_tui_instance": True,
            "sequence_advanced": True,
            "stale_or_unavailable": True,
            "a_binding_preserved_before_cli": True,
            "picker_closed": True,
            "pane_owner_client": True,
            "pane_live": True,
            "pane_applied": True,
            "manual_refusal_valid": True,
            "manual_refusal_no_ticket": True,
            "foreign_route_persisted_after_refusal": True,
            "a_binding_preserved_after_cli": True,
        }
        # A missing rendered title is diagnostic only after exact route proof.
        foreign_title_visible = False
        self.assertFalse(foreign_title_visible)
        self.assertTrue(foreign_refusal_acceptance(checks))

        # A title cannot stand in for the exact same-TUI observer record.
        for missing_or_wrong_observer in (
            "exact_foreign_route", "same_tui_instance", "sequence_advanced",
            "foreign_route_persisted_after_refusal",
        ):
            altered = {**checks, missing_or_wrong_observer: False}
            self.assertFalse(foreign_refusal_acceptance(altered))

        # Mutation and any successful/non-manual prepare result block acceptance.
        self.assertFalse(foreign_refusal_acceptance({
            **checks, "a_binding_preserved_after_cli": False,
        }))
        self.assertFalse(foreign_refusal_acceptance({
            **checks, "manual_refusal_valid": False,
        }))
        self.assertFalse(foreign_refusal_acceptance({
            **checks, "manual_refusal_no_ticket": False,
        }))

    def test_picker_never_enters_when_attach_and_fresh_child_do_not_filter(self) -> None:
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")

        class FakeAttach:
            def __init__(self) -> None:
                self.output = bytearray(b"ready")
                self.total_output_bytes = len(self.output)
                self.sent = bytearray()

            def alive(self) -> bool:
                return True

            def drain(self, _timeout: float = 0.0) -> bytes:
                return b""

            def screen_text(self) -> str:
                # Simulate a transcript that keeps both rows after the query.
                return "Sessions for project\n" + "\n".join(labels)

            def send(self, payload: bytes) -> None:
                self.sent.extend(payload)
                self.total_output_bytes += 1

        attach = FakeAttach()
        fresh_reads = []

        def fresh_child_read() -> dict[str, object]:
            fresh_reads.append(True)
            return {
                "status": "known", "contains_picker_heading": True,
                "input_owner": "client",
                "contains_both_fixture_labels": True,
                "contains_any_fixture_label": True,
                "contains_target_label": True, "contains_competitor_label": True,
                "screen_positions": {"heading": [[7, 9]], "target": [[12, 9]],
                                     "competitor": [[14, 9]],
                                     "fixture_label_0": [[12, 9]],
                                     "fixture_label_1": [[14, 9]]},
            }

        trace: dict[str, object] = {}
        selected = select_session(
            attach, labels[1], labels[0], labels, time.monotonic() + 0.05,
            trace, pre_read={"status": "known", "contains_opencode_title": True,
                             "screen_bytes": 5},
            fresh_child_read=fresh_child_read,
        )
        self.assertFalse(selected)
        self.assertNotIn(b"\r", attach.sent)
        self.assertEqual(attach.sent, b"\x10" + labels[1].encode("ascii"))
        self.assertGreaterEqual(len(fresh_reads), 1)
        self.assertFalse(trace.get("filtered_child_read_valid") and selected)

    def test_fresh_child_unique_layout_can_authorize_when_attach_screen_lags(self) -> None:
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")

        class LaggingAttach:
            def __init__(self) -> None:
                self.output = bytearray(b"ready")
                self.total_output_bytes = len(self.output)
                self.sent = bytearray()
                self.filtered = False

            def alive(self) -> bool:
                return True

            def drain(self, _timeout: float = 0.0) -> bytes:
                return b""

            def screen_text(self) -> str:
                return ("OpenCode home screen" if self.filtered else
                        "Sessions for project\n" + "\n".join(labels))

            def send(self, payload: bytes) -> None:
                self.sent.extend(payload)
                self.total_output_bytes += 1
                if payload == labels[1].encode("ascii"):
                    self.filtered = True

        positions = {
            "heading": [[7, 9]], "target": [[9, 9], [12, 9]], "competitor": [],
            # The active title also appears in the fixed application header
            # above the local modal; only the two rows after the heading are
            # selectable session-list content.
            "filter_query": [[0, 35], [9, 9], [12, 9]],
            "fixture_label_0": [], "fixture_label_1": [[9, 9], [12, 9]],
        }
        child_markers = {
            "status": "known", "input_owner": "client", "contains_picker_heading": True,
            "contains_open_menu_heading": False,
            "filter_query_is_target_label": True,
            "contains_target_label": True, "contains_competitor_label": False,
            "target_fixture_index": 1, "fixture_title_count": 2,
            "screen_positions": positions,
        }
        self.assertTrue(fresh_child_modal_rows(child_markers, filtered=True))
        duplicate = {**child_markers, "screen_positions": {
            **positions, "target": [[9, 9], [12, 9], [14, 9]],
            "fixture_label_1": [[9, 9], [12, 9], [14, 9]],
        }}
        self.assertFalse(fresh_child_modal_rows(duplicate, filtered=True))
        menu_only_target = {
            "status": "known", "input_owner": "client", "contains_picker_heading": True,
            "contains_any_fixture_label": True, "contains_projects_category": True,
            "screen_positions": {
                "heading": [[7, 9]], "target": [], "competitor": [],
                "fixture_label_0": [[15, 9]], "fixture_label_1": [],
            },
        }
        self.assertTrue(fresh_child_modal_rows(
            menu_only_target, filtered=False, require_projects_category=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            {**menu_only_target, "contains_projects_category": False},
            filtered=False, require_projects_category=True,
        ))

        attach = LaggingAttach()
        menu_markers = {
            "status": "known", "input_owner": "client", "contains_picker_heading": True,
            "contains_target_label": True, "contains_competitor_label": True,
            "fixture_title_count": 2,
            "screen_positions": {"heading": [[7, 9]], "target": [[12, 9]],
                                 "competitor": [[14, 9]],
                                 "fixture_label_0": [[12, 9]],
                                 "fixture_label_1": [[14, 9]]},
        }
        def read_child() -> dict[str, object]:
            return child_markers if attach.filtered else menu_markers
        trace: dict[str, object] = {}
        selected = select_session(
            attach, labels[1], labels[0], labels, time.monotonic() + 0.5,
            trace, pre_read={"status": "known", "contains_opencode_title": True,
                             "screen_bytes": 5},
            fresh_child_read=read_child,
            fixture_titles=labels,
        )
        self.assertTrue(selected, trace)
        self.assertTrue(trace.get("filter_authority") == "fresh_managed_child_screen")
        self.assertEqual(attach.sent, b"\x10" + labels[1].encode("ascii") + b"\r")

    def test_picker_rejects_target_plus_any_other_known_fixture_result(self) -> None:
        titles = (
            "Panelink V2 Identity A", "Panelink V2 Identity B",
            "Panelink Managed Transition New Root",
            "Panelink Managed Transition Imported Fork",
            "Panelink recovery imported fork parent",
            "Panelink recovery fork seed",
            "Panelink Foreign Root",
        )
        markers = {
            "status": "known", "input_owner": "client", "owner_revision": 9,
            "contains_picker_heading": True, "contains_target_label": True,
            "contains_competitor_label": False, "target_fixture_index": 1,
            "fixture_title_count": len(titles),
            "screen_positions": {
                "heading": [[7, 9]],
                "target": [[9, 9], [12, 9]],
                **{f"fixture_label_{index}": [] for index in range(len(titles))},
            },
        }
        # B is present in the query and as a result, but the prior unfiltered
        # fork row is still visible while the 150 ms native query is pending.
        markers["screen_positions"]["fixture_label_1"] = [[9, 9], [12, 9]]
        markers["screen_positions"]["fixture_label_3"] = [[15, 9]]
        self.assertFalse(fresh_child_modal_rows(markers, filtered=True))

        class Attach:
            def __init__(self) -> None:
                self.output = bytearray(b"ready")
                self.total_output_bytes = len(self.output)
                self.sent = bytearray()

            def alive(self) -> bool:
                return True

            def drain(self, _timeout: float = 0.0) -> bytes:
                return b""

            def screen_text(self) -> str:
                return "OpenCode home screen"

            def send(self, payload: bytes) -> None:
                self.sent.extend(payload)

        attach = Attach()
        def stale_read() -> dict[str, object]:
            return markers
        selected = select_session(
            attach, titles[1], titles[0], (titles[0], titles[1]),
            time.monotonic() + 0.45,
            pre_read={"status": "known", "contains_opencode_title": True,
                      "screen_bytes": 5},
            fresh_child_read=stale_read, fixture_titles=titles,
        )
        self.assertFalse(selected)
        self.assertNotIn(b"\r", attach.sent)
        self.assertTrue(attach.sent.startswith(b"\x10"))

    def test_picker_requires_two_matching_reads_after_debounce(self) -> None:
        titles = ("Fixture A", "Fixture B", "Fixture fork")
        marker = {
            "status": "known", "input_owner": "client", "owner_revision": 4,
            "contains_picker_heading": True, "contains_target_label": True,
            "contains_competitor_label": False, "target_fixture_index": 1,
            "fixture_title_count": len(titles),
            "screen_positions": {
                "heading": [[3, 1]], "target": [[4, 2], [7, 2]],
                "competitor": [],
                "fixture_label_0": [], "fixture_label_1": [[4, 2], [7, 2]],
                "fixture_label_2": [],
            },
        }
        reads = 0

        def fresh_read() -> dict[str, object]:
            nonlocal reads
            reads += 1
            return marker

        stable, last = stable_fresh_child_modal_rows(
            fresh_read, titles[1], titles, time.monotonic() + 0.8,
        )
        self.assertTrue(stable)
        self.assertEqual(last, marker)
        self.assertGreaterEqual(reads, 2)

    def test_session_list_modal_allows_benign_query_before_target_appears(self) -> None:
        foreign_title = "Panelink Foreign Root"
        titles = ("Panelink V2 Identity A", "Panelink V2 Identity B", foreign_title)
        markers = {
            "status": "known", "input_owner": "client", "contains_picker_heading": True,
            "contains_projects_category": False, "target_fixture_index": 2,
            "contains_target_label": False,
            "fixture_title_count": len(titles),
            "screen_positions": {
                "heading": [[3, 1]], "target": [[7, 4]], "competitor": [],
                "fixture_label_0": [[4, 4]], "fixture_label_1": [[5, 4]],
                "fixture_label_2": [[7, 4]],
            },
        }
        self.assertTrue(fresh_child_modal_rows(
            markers, filtered=False,
        ))
        # The foreign row can arrive only after the harmless title query. A
        # modal heading alone, with no fixture results or no client owner, is
        # insufficient to send any query.
        self.assertFalse(fresh_child_modal_rows(
            {**markers, "screen_positions": {
                **markers["screen_positions"], "fixture_label_0": [],
                "fixture_label_1": [], "fixture_label_2": [],
            }},
            filtered=False,
        ))
        self.assertFalse(fresh_child_modal_rows(
            {**markers, "contains_picker_heading": False},
            filtered=False,
        ))
        self.assertFalse(fresh_child_modal_rows(
            {**markers, "input_owner": "none"},
            filtered=False,
        ))

    def test_global_open_modal_counts_query_above_sessions_category(self) -> None:
        titles = ("Fixture A", "Fixture B", "Panelink Foreign Root")
        markers = {
            "status": "known", "input_owner": "client", "owner_revision": 12,
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": 0, "contains_target_label": True,
            "session_rows_boundary_verified": True, "session_rows_overflowed": False,
            "contains_competitor_label": False, "target_fixture_index": 2,
            "fixture_title_count": len(titles),
            "screen_positions": {
                "heading": [[2, 3]], "sessions_category": [[5, 2]],
                "target": [[4, 12], [7, 8]], "competitor": [],
                "filter_query": [[4, 12], [7, 8]],
                "fixture_label_0": [], "fixture_label_1": [],
                "fixture_label_2": [[4, 12], [7, 8]],
            },
            "filter_query_is_target_label": True,
        }
        self.assertTrue(fresh_child_modal_rows(markers, filtered=True))

        # Anchoring at the Sessions category discards the query row above it;
        # the exact Open dialog title must be the modal boundary.
        category_anchored = {**markers, "contains_open_menu_heading": False,
                             "screen_positions": {
                                 **markers["screen_positions"],
                                 "heading": [[5, 2]],
                             }}
        self.assertFalse(fresh_child_modal_rows(category_anchored, filtered=True))
        no_category = {**markers, "contains_sessions_category": False}
        self.assertFalse(fresh_child_modal_rows(no_category, filtered=True))

    def test_global_open_unique_query_requires_one_exact_fixture_result(self) -> None:
        titles = ("Panelink V2 Identity A", "Panelink V2 Identity B",
                  "Panelink Foreign Root")
        positions = {
            "heading": [[2, 3]], "sessions_category": [[5, 2]],
            "target": [[7, 8]], "competitor": [],
            "filter_query": [[4, 12], [7, 18]],
            "fixture_label_0": [], "fixture_label_1": [],
            "fixture_label_2": [[7, 8]],
        }
        markers = {
            "status": "known", "input_owner": "client", "owner_revision": 12,
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": 0, "contains_target_label": True,
            "session_rows_boundary_verified": True, "session_rows_overflowed": False,
            "filter_query_is_target_label": False, "target_fixture_index": 2,
            "fixture_title_count": len(titles), "screen_positions": positions,
        }
        self.assertTrue(fresh_child_modal_rows(markers, filtered=True))

        # A fuzzy/shared-token competitor remains a real result and blocks
        # activation even when the target token itself is unique.
        competitor_result = {**markers, "screen_positions": {
            **positions, "fixture_label_0": [[9, 10]],
        }}
        self.assertFalse(fresh_child_modal_rows(competitor_result, filtered=True))
        # A second occurrence outside the exact target row may be a matching
        # description or another global result, so it also blocks Enter.
        unrelated_match = {**markers, "screen_positions": {
            **positions, "filter_query": [[4, 12], [7, 18], [10, 6]],
        }}
        self.assertFalse(fresh_child_modal_rows(unrelated_match, filtered=True))

        self.assertFalse(fresh_child_modal_rows(
            {**markers, "contains_projects_category": True}, filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            {**markers, "unknown_selectable_row_count": 1}, filtered=True,
        ))
        # A verified modal may accept search text while its unfiltered list
        # contains other choices. Those choices still prohibit Enter.
        unfiltered = {**markers, "contains_projects_category": True,
                      "unknown_selectable_row_count": 1}
        self.assertTrue(fresh_child_modal_rows(unfiltered, filtered=False))
        self.assertFalse(fresh_child_modal_rows(unfiltered, filtered=True))

    def test_global_foreign_picker_uses_full_title_as_exact_filter(self) -> None:
        title = "Panelink Foreign Root"
        titles = ("Panelink V2 Identity A", "Panelink V2 Identity B", title)
        marker = {
            "status": "known", "input_owner": "client", "owner_revision": 3,
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": 0, "contains_target_label": True,
            "session_rows_boundary_verified": True, "session_rows_overflowed": False,
            "contains_competitor_label": False, "target_fixture_index": 2,
            "fixture_title_count": len(titles), "filter_query_is_target_label": True,
            "screen_positions": {
                "heading": [[2, 1]], "sessions_category": [[4, 1]],
                "target": [[3, 1], [5, 1]], "filter_query": [[3, 1], [5, 1]],
                "competitor": [], "fixture_label_0": [], "fixture_label_1": [],
                "fixture_label_2": [[3, 1], [5, 1]],
            },
        }
        stable, current = stable_fresh_child_modal_rows(
            lambda: marker, title, titles, time.monotonic() + 0.8,
            filter_text=title,
        )
        self.assertTrue(stable)
        self.assertIs(current, marker)
        self.assertFalse(fresh_child_modal_rows(
            {**marker, "contains_projects_category": True}, filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            {**marker, "unknown_selectable_row_count": 1}, filtered=True,
        ))

    def test_exact_fixture_session_id_query_is_narrow_and_fail_closed(self) -> None:
        title_a, title_b = "Panelink V2 Identity A", "Panelink V2 Identity B"
        titles = (title_a, title_b)
        session_a = "ses_" + "A" * 26
        session_b = "ses_" + "B" * 26

        def exact_marker(**changes: object) -> dict[str, object]:
            marker: dict[str, object] = {
                "status": "known", "input_owner": "client", "owner_revision": 11,
                "contains_picker_heading": True, "contains_open_menu_heading": True,
                "contains_sessions_category": True, "contains_projects_category": False,
                "unknown_selectable_row_count": 0, "contains_target_label": True,
                "session_rows_boundary_verified": True, "session_rows_overflowed": False,
                "session_result_row_count": 1, "target_fixture_index": 0,
                "fixture_title_count": 2, "filter_query_mode": "fixture_session_id",
                "filter_query_is_fixture_session_id": True,
                "screen_positions": {
                    "heading": [[2, 3]], "filter_query": [[4, 12]],
                    "sessions_category": [[6, 2]], "target": [[8, 9]],
                    "competitor": [], "fixture_label_0": [[8, 9]],
                    "fixture_label_1": [],
                },
            }
            marker.update(changes)
            return marker

        valid = exact_marker()
        self.assertTrue(fresh_child_modal_rows(valid, filtered=True))
        stable, current = stable_fresh_child_modal_rows(
            lambda: valid, title_a, titles, time.monotonic() + 0.8,
            filter_text=session_a, filter_mode="fixture_session_id",
            fixture_session_id=session_a,
        )
        self.assertTrue(stable)
        self.assertIs(current, valid)
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(filter_query_mode="title"), filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(filter_query_is_fixture_session_id=False), filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(screen_positions={
                **valid["screen_positions"], "filter_query": [[4, 12], [5, 12]],
            }), filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(screen_positions={
                **valid["screen_positions"], "target": [[8, 9], [9, 9]],
                "fixture_label_0": [[8, 9], [9, 9]],
            }), filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(screen_positions={
                **valid["screen_positions"], "fixture_label_1": [[10, 9]],
            }), filtered=True,
        ))
        self.assertFalse(fresh_child_modal_rows(
            exact_marker(contains_projects_category=True), filtered=True,
        ))

        class PickerAttach:
            def __init__(self) -> None:
                self.output = bytearray(b"ready")
                self.total_output_bytes = len(self.output)
                self.sent = bytearray()

            def alive(self) -> bool:
                return True

            def drain(self, _timeout: float = 0.0) -> bytes:
                return b""

            def screen_text(self) -> str:
                return f"Open\n{title_a}\n{title_b}\nSessions"

            def send(self, payload: bytes) -> None:
                self.sent.extend(payload)
                self.total_output_bytes += len(payload)

        unfiltered = {
            "status": "known", "input_owner": "client",
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True,
            "contains_projects_category": True,
            "screen_positions": {
                "heading": [[1, 0]], "sessions_category": [[4, 0]],
                "target": [[2, 0]], "competitor": [[3, 0]],
                "fixture_label_0": [[2, 0]], "fixture_label_1": [[3, 0]],
            },
        }

        def run_selection(read_marker: object, query: str, target: str = title_a,
                          expect_selected: bool = False, duration: float = 0.35) -> PickerAttach:
            attach = PickerAttach()
            reads = iter([unfiltered])
            trace: dict[str, object] = {}
            def read() -> dict[str, object]:
                try:
                    return next(reads)
                except StopIteration:
                    if callable(read_marker):
                        return read_marker()
                    return read_marker  # type: ignore[return-value]
            selected = select_session(
                attach, target, title_b, titles, time.monotonic() + duration,
                trace,
                pre_read={"status": "known", "contains_opencode_title": True,
                          "screen_bytes": 10},
                fresh_child_read=read, fixture_titles=titles,
                open_chord=b"\x0f", filter_text=query,
                filter_mode="fixture_session_id", fixture_session_id=session_a,
            )
            self.assertEqual(selected, expect_selected, trace)
            if expect_selected:
                self.assertTrue(attach.sent.endswith(b"\r"))
            else:
                self.assertNotIn(b"\r", attach.sent)
            return attach

        # Malformed IDs and valid IDs for another fixture are rejected before
        # their values can be sent through the picker.
        malformed = PickerAttach()
        self.assertFalse(select_session(
            malformed, title_a, title_b, titles, time.monotonic() + 0.1,
            pre_read={"status": "known", "contains_opencode_title": True,
                      "screen_bytes": 10},
            filter_text="ses_short", filter_mode="fixture_session_id",
            fixture_session_id="ses_short",
        ))
        self.assertEqual(malformed.sent, b"")
        self.assertEqual(run_selection(valid, session_a, expect_selected=True,
                                       duration=1.2).sent,
                         b"\x0f" + session_a.encode("ascii") + b"\r")
        self.assertEqual(run_selection(valid, session_b).sent, b"")
        self.assertEqual(run_selection(exact_marker(filter_query_mode="title"), session_a).sent,
                         b"\x0f" + session_a.encode("ascii"))
        self.assertEqual(run_selection(exact_marker(
            screen_positions={**valid["screen_positions"], "filter_query": [[4, 12], [5, 12]]},
        ), session_a).sent, b"\x0f" + session_a.encode("ascii"))
        self.assertEqual(run_selection(valid, session_a, target=title_b).sent,
                         b"\x0f" + session_a.encode("ascii"))

        read_count = 0
        def unstable_reads() -> dict[str, object]:
            nonlocal read_count
            read_count += 1
            return exact_marker(owner_revision=11 + read_count % 2)
        self.assertEqual(run_selection(unstable_reads, session_a).sent,
                         b"\x0f" + session_a.encode("ascii"))

    def test_global_picker_result_rows_stop_at_modal_blank_before_lower_chrome(self) -> None:
        def row(*parts: tuple[int, str]) -> str:
            cells = [" "] * 100
            for column, value in parts:
                cells[column:column + len(value)] = value
            return "".join(cells)

        lines = [" "] * 24
        lines[7] = row((9, "Open esc"))
        lines[9] = row((9, "Panelink Foreign Root"))
        lines[11] = row((13, "Sessions"))
        lines[12] = row((9, "Panelink Foreign Root"))
        # The app has chrome on this row, but it lies outside the centered
        # 88-column modal (columns 6 through 93) and must not become a result.
        lines[13] = row((95, "lower chrome"))
        lines[14] = row((13, "Projects"))
        rows, boundary, overflow, bounds, guard_reason = global_picker_session_result_rows(
            "\n".join(lines),
        )
        self.assertEqual(rows, ["Panelink Foreign Root"])
        self.assertTrue(boundary)
        self.assertFalse(overflow)
        self.assertEqual(bounds, [6, 94])
        self.assertEqual(guard_reason, "ok")

        titles = ("Panelink V2 Identity A", "Panelink V2 Identity B",
                  "Panelink Foreign Root")
        markers = {
            "status": "known", "input_owner": "client", "owner_revision": 3,
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": 0, "session_result_row_count": len(rows),
            "session_rows_boundary_verified": boundary,
            "session_rows_overflowed": overflow, "contains_target_label": True,
            "filter_query_is_target_label": True, "target_fixture_index": 2,
            "fixture_title_count": len(titles),
            "screen_positions": {
                "heading": [[7, 9]], "sessions_category": [[11, 13]],
                "target": [[9, 9], [12, 9]], "filter_query": [[9, 9], [12, 9]],
                "competitor": [], "fixture_label_0": [], "fixture_label_1": [],
                "fixture_label_2": [[9, 9], [12, 9]],
            },
        }
        self.assertTrue(fresh_child_modal_rows(markers, filtered=True))
        self.assertFalse(fresh_child_modal_rows(
            {**markers, "contains_projects_category": True}, filtered=True,
        ))

    def test_global_picker_unknown_contiguous_row_and_overflow_fail_closed(self) -> None:
        def row(*parts: tuple[int, str]) -> str:
            cells = [" "] * 100
            for column, value in parts:
                cells[column:column + len(value)] = value
            return "".join(cells)

        base = [" "] * 24
        base[7] = row((9, "Open esc"))
        base[9] = row((9, "Panelink Foreign Root"))
        base[11] = row((13, "Sessions"))
        base[12] = row((9, "Panelink Foreign Root"))
        base[13] = row((9, "Unrecognized session"))
        base[14] = " " * 100
        extra, boundary, overflow, _bounds, guard_reason = global_picker_session_result_rows(
            "\n".join(base),
        )
        self.assertEqual(extra, ["Panelink Foreign Root", "Unrecognized session"])
        self.assertTrue(boundary)
        self.assertFalse(overflow)
        self.assertEqual(guard_reason, "ok")
        self.assertEqual(sum("Panelink Foreign Root" not in item for item in extra), 1)
        self.assertFalse(fresh_child_modal_rows({
            "status": "known", "input_owner": "client",
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": 1,
            "session_rows_boundary_verified": boundary,
            "session_rows_overflowed": overflow,
            "screen_positions": {
                "heading": [[7, 9]], "sessions_category": [[11, 13]],
                "target": [[9, 9], [12, 9]], "competitor": [],
                "filter_query": [[9, 9], [12, 9]],
                "fixture_label_0": [], "fixture_label_1": [],
                "fixture_label_2": [[9, 9], [12, 9]],
            },
            "contains_target_label": True, "target_fixture_index": 2,
            "fixture_title_count": 3, "filter_query_is_target_label": True,
        }, filtered=True))

        footer_ellipsis = list(base)
        footer_ellipsis[12] = row((9, "Panelink Foreign Root · older…"))
        footer_rows, footer_boundary, footer_overflow, _bounds, footer_reason = (
            global_picker_session_result_rows("\n".join(footer_ellipsis))
        )
        self.assertEqual(footer_rows, ["Panelink Foreign Root · older…", "Unrecognized session"])
        self.assertTrue(footer_boundary)
        self.assertFalse(footer_overflow)
        self.assertEqual(footer_reason, "ok")

        clipped = list(base)
        clipped[13] = row((9, "…"))
        clipped_rows, clipped_boundary, clipped_overflow, _bounds, guard_reason = (
            global_picker_session_result_rows("\n".join(clipped))
        )
        self.assertEqual(clipped_rows, ["Panelink Foreign Root"])
        self.assertTrue(clipped_boundary)
        self.assertTrue(clipped_overflow)
        self.assertEqual(guard_reason, "ellipsis")
        self.assertFalse(fresh_child_modal_rows({
            "status": "known", "input_owner": "client",
            "contains_picker_heading": True, "contains_open_menu_heading": True,
            "contains_sessions_category": True, "contains_projects_category": False,
            "unknown_selectable_row_count": None,
            "session_rows_boundary_verified": clipped_boundary,
            "session_rows_overflowed": clipped_overflow,
            "screen_positions": {
                "heading": [[7, 9]], "sessions_category": [[11, 13]],
                "target": [[9, 9], [12, 9]], "competitor": [],
                "filter_query": [[9, 9], [12, 9]],
                "fixture_label_0": [], "fixture_label_1": [],
                "fixture_label_2": [[9, 9], [12, 9]],
            },
            "contains_target_label": True, "target_fixture_index": 2,
            "fixture_title_count": 3, "filter_query_is_target_label": True,
        }, filtered=True))

        missing_gap = list(base)
        for index in range(14, len(missing_gap)):
            missing_gap[index] = row((9, "still within modal"))
        _rows, missing_boundary, _overflow, _bounds, guard_reason = global_picker_session_result_rows(
            "\n".join(missing_gap),
        )
        self.assertFalse(missing_boundary)
        self.assertEqual(guard_reason, "blank_boundary_missing")

    def test_route_observer_diagnostics_distinguish_empty_and_rejected(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-route-diagnostic-") as temporary:
            root = Path(temporary)
            route_file = root / "route.jsonl"
            route_file.touch()
            _record, empty = fixture_route_diagnostic(route_file, "secret-nonce", {"a" * 64})
            self.assertEqual(empty["status"], "empty")
            self.assertEqual(empty["byte_count"], 0)
            route_file.write_text("not-json\n", encoding="utf-8")
            _record, rejected = fixture_route_diagnostic(
                route_file, "secret-nonce", {"a" * 64},
            )
            self.assertEqual(rejected["status"], "rejected_stream")
            self.assertEqual(rejected["rejected_count"], 1)
            self.assertNotIn("secret-nonce", repr(rejected))
            instance = "f" * 32
            route_file.write_text(json.dumps({
                "schema": 1, "kind": "plugin_loaded", "client_nonce": "secret-nonce",
                "tui_instance": instance, "seq": 1, "monotonic_ms": 1,
                "route_name": "session", "session_id_hash": "b" * 64,
                "session_top_level": True, "project_hash": "a" * 64,
            }) + "\n", encoding="utf-8")
            record, valid = fixture_route_diagnostic(
                route_file, "secret-nonce", {"a" * 64},
            )
            self.assertEqual(valid["status"], "valid")
            self.assertEqual(record["tui_instance"], instance)
            self.assertNotIn(instance, repr(valid))

    def test_route_observer_diagnostics_hash_identity_and_reject_instance_switch(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-route-identity-") as temporary:
            route_file = Path(temporary) / "route.jsonl"
            nonce = "0123456789abcdef0123456789abcdef"
            projects = {"a" * 64, "b" * 64}
            instance_a = "c" * 32
            instance_b = "d" * 32
            records = []
            for seq, (project, instance) in enumerate(
                    (("a" * 64, instance_a), ("b" * 64, instance_b)), start=1):
                records.append(json.dumps({
                    "schema": 1, "kind": "poll", "client_nonce": nonce,
                    "tui_instance": instance, "seq": seq,
                    "monotonic_ms": seq * 100, "route_name": "session",
                    "session_id_hash": "e" * 64, "session_top_level": True,
                    "project_hash": project,
                }))
            route_file.write_text("\n".join(records) + "\n", encoding="utf-8")
            record, diagnostic = fixture_route_diagnostic(route_file, nonce, projects)
            self.assertIsNone(record)
            self.assertEqual(diagnostic["status"], "instance_mismatch")
            self.assertEqual(diagnostic["accepted_count"], 2)
            self.assertEqual(diagnostic["instance_count"], 2)
            self.assertEqual(diagnostic["latest_project_sha256"], "b" * 64)
            self.assertEqual(diagnostic["latest_session_sha256"], "e" * 64)
            self.assertEqual(diagnostic["latest_instance_sha256"],
                             hashlib.sha256(instance_b.encode()).hexdigest())
            self.assertNotIn(nonce, repr(diagnostic))
            self.assertNotIn(instance_b, repr(diagnostic))

    def test_exact_open_heading_preserves_search_field_before_sessions_category(self) -> None:
        screen = "OpenCode home\nOpen\nSearch sessions and projects\nPanelink Foreign Root\nSessions\nPanelink Foreign Root"
        region = session_picker_region(screen)
        self.assertTrue(region.startswith("Open\n"))
        self.assertIn("Search sessions and projects", region)
        self.assertTrue(session_picker_heading(screen))
        self.assertFalse(session_picker_heading("OpenCode home\nOpenCode settings"))
        # Structural heading extraction must not count the OpenCode header as
        # a second occurrence of the exact dialog title.
        self.assertEqual(exact_line_positions(screen, "Open"), [[1, 0]])
        self.assertEqual(exact_line_positions(screen, "Sessions"), [[4, 0]])

    def test_local_picker_open_action_does_not_replace_earlier_title(self) -> None:
        local = "OpenCode home\n  Sessions\nFixture B\nFixture B\n  Open"
        self.assertEqual(picker_heading_position(local), [[1, 2]])
        self.assertEqual(picker_heading_marker(local), "Sessions")
        self.assertTrue(session_picker_region(local).startswith("  Sessions\n"))
        self.assertIn("Fixture B\nFixture B", session_picker_region(local))

        global_picker = "OpenCode home\n  Open\nFixture B\nSessions\nFixture B"
        self.assertEqual(picker_heading_position(global_picker), [[1, 2]])
        self.assertEqual(picker_heading_marker(global_picker), "Open")
        self.assertFalse(session_picker_heading("OpenCode home\n  Open"))
        with_hint = global_picker.replace("  Open\n", "  Open                 esc\n")
        self.assertEqual(picker_heading_position(with_hint), [[1, 2]])
        self.assertEqual(picker_heading_marker(with_hint), "Open")
        self.assertTrue(session_picker_region(with_hint).startswith("  Open "))

    def test_exact_target_parser_is_bounded_and_canonical(self) -> None:
        self.assertEqual(parse_target(TARGET), (71, 2, 3, 4, 5))
        for value in (
            "latest", "1:2:3:4", "1:2:3:4:5:6", "0:2:3:4:5", "01:2:3:4:5",
            "1:2:3:4:-5", "1:2:3:4:18446744073709551616", None, 4,
        ):
            with self.subTest(value=value):
                self.assertIsNone(parse_target(value))

    def test_historical_cli_target_requires_exact_structured_fields(self) -> None:
        self.assertEqual(safe_cli_target({
            "generation": 71, "workspace": 2, "pane": 3,
            "session": 4, "occupant_revision": 5,
        }), TARGET)
        for value in (
            TARGET,
            {"generation": True, "workspace": 2, "pane": 3,
             "session": 4, "occupant_revision": 5},
            {"generation": 71, "workspace": 2, "pane": 3, "session": 4},
        ):
            with self.subTest(value=value):
                self.assertIsNone(safe_cli_target(value))

    def test_snapshot_accepts_only_fixed_metadata_for_exact_target(self) -> None:
        value = {
            "ok": True,
            "action": "provider-binding",
            "target": TARGET,
            "status": "last_confirmed",
            "last_confirmed_revision": 8,
            "current_at_crash": False,
            "automatic_resume": False,
            "provider_session_id": "do-not-retain",
        }
        clean = sanitized_snapshot(value, TARGET)
        self.assertEqual(clean, {
            "target": TARGET,
            "status": "last_confirmed",
            "last_confirmed_revision": 8,
            "current_at_crash": False,
            "automatic_resume": False,
        })
        self.assertNotIn("provider_session_id", clean)
        self.assertIsNone(sanitized_snapshot({**value, "target": "71:2:3:4:6"}, TARGET))
        self.assertIsNone(sanitized_snapshot({**value, "current_at_crash": True}, TARGET))
        self.assertIsNone(sanitized_snapshot({**value, "last_confirmed_revision": True}, TARGET))

    def test_live_combined_record_requires_exact_scope_and_fixture_id(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-live-slot-") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            project = root / "project"
            project.mkdir(mode=0o700)
            server = root / "server"
            server.mkdir(mode=0o700)
            bindings = server / "provider-bindings"
            bindings.mkdir(mode=0o700)
            target_dir = bindings / TARGET.replace(":", "-")
            target_dir.mkdir(mode=0o700)
            project_bytes = str(project.resolve()).encode("ascii")
            session_bytes = b"ses_fixtureB"
            version = b"2.0.32"
            target_bytes = b"".join(number.to_bytes(8, "big") for number in parse_target(TARGET))
            binding = bytearray(b"PLBIND01" + target_bytes)
            binding.extend((9).to_bytes(8, "big"))
            binding.extend((2).to_bytes(8, "big"))
            binding.extend((10).to_bytes(8, "big"))
            binding.extend((1).to_bytes(2, "big"))
            binding.extend(len(version).to_bytes(2, "big"))
            binding.extend(len(project_bytes).to_bytes(2, "big"))
            binding.extend(len(session_bytes).to_bytes(2, "big"))
            binding.extend(bytes(16))
            binding.extend(version + project_bytes + session_bytes)
            managed = bytearray(b"PLMPRV01")
            managed.extend((parse_target(TARGET)[0]).to_bytes(8, "big"))
            managed.extend((1).to_bytes(8, "big"))
            managed.extend((1).to_bytes(8, "big"))
            managed.extend(bytes([1]) * 32)
            managed.extend(bytes((1, 1)))
            managed.extend(bytes(2))  # no optional executable path
            managed.extend(len(binding).to_bytes(4, "big"))
            managed.extend(binding)
            slot = target_dir / "managed-provider.record"
            slot.write_bytes(managed)
            slot.chmod(0o600)

            valid = read_live_managed_binding(server, TARGET, project, session_bytes.decode())
            self.assertEqual(valid["revision"], 2)
            self.assertEqual(valid["provider_version"], "2.0.32")
            self.assertNotIn(session_bytes.decode(), json.dumps(valid))
            self.assertIsNone(read_live_managed_binding(server, TARGET, project, "ses_other"))
            self.assertIsNone(read_live_managed_binding(
                server, "71:2:3:4:6", project, session_bytes.decode(),
            ))
            self.assertIsNone(read_live_managed_binding(
                server, TARGET, root, session_bytes.decode(),
            ))

    def test_serve_capture_keeps_only_allowlisted_status_and_target(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-probe-unit-") as temporary:
            root = Path(temporary)
            script = root / "fake-serve.py"
            script.write_text(
                "import json\n"
                f"print(json.dumps({{'action':'serve','state':'running','generation':71,'target':'{TARGET}'}}), flush=True)\n"
                f"print(json.dumps({{'action':'provider-binding','target':'{TARGET}','status':'last_confirmed','last_confirmed_revision':8,'current_at_crash':False,'automatic_resume':False,'provider_session_id':'secret'}}), flush=True)\n",
                encoding="utf-8",
            )
            handle = ServeProcess([sys.executable, str(script)], root,
                                  {"PATH": os.defpath}, expected_target="")
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline and handle.latest is None:
                handle.poll(0.02)
            try:
                self.assertEqual(handle.startup, {"target": TARGET, "generation": 71})
                self.assertEqual(handle.latest["status"], "last_confirmed")
                self.assertNotIn("provider_session_id", handle.latest)
                self.assertNotIn("secret", json.dumps(handle.latest))
            finally:
                if handle.process.poll() is None:
                    handle.process.terminate()
                handle.process.wait(timeout=2)
                handle.selector.close()
                for stream in (handle.process.stdout, handle.process.stderr):
                    if stream is not None and not stream.closed:
                        stream.close()

    @unittest.skipUnless(os.environ.get("PANELINK_RUN_P4_MANAGED_BRIDGE") == "1",
                         "real native acceptance is run explicitly after the isolated binary build")
    def test_real_managed_bridge_acceptance(self) -> None:
        binary = os.environ.get(
            "PANELINK_BINARY",
            "/private/tmp/panelink-p4-bridge-store-target/debug/panelink",
        )
        completed = subprocess.run(
            [sys.executable, str(SCRIPT), "--panelink-binary", binary],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=90, check=False, close_fds=True, shell=False,
        )
        self.assertLessEqual(len(completed.stdout), 16 * 1024)
        self.assertLessEqual(len(completed.stderr), 4 * 1024)
        result = json.loads(completed.stdout)
        self.assertEqual(result["classification"], "proven", result.get("reason"))
        self.assertTrue(result["root_a_to_b_to_a"])
        self.assertTrue(result["initial_a_binding_verified"])
        self.assertTrue(result["interim_b_binding_verified"])
        self.assertTrue(result["return_a_binding_verified"])
        self.assertGreaterEqual(result["final_revisions"][0], 2)
        self.assertTrue(result["passive_b_unchanged"])
        self.assertTrue(result["home_stale_and_returned"])
        self.assertTrue(result["historical_identity_matches"])
        self.assertTrue(result["historical_after_stop"])
        self.assertFalse(result["current_at_crash"])
        self.assertFalse(result["automatic_resume"])
        self.assertEqual(result["prompt_submissions"], 0)
        self.assertTrue(result["user_message_absence_verified"])
        self.assertTrue(result["cleanup_completed"])
        self.assertNotIn(b"provider_session_id", completed.stdout)
        self.assertNotIn(b"session-A", completed.stdout)
        self.assertNotIn(b"session-B", completed.stdout)


if __name__ == "__main__":
    unittest.main()
