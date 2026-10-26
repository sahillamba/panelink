import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from types import SimpleNamespace
from unittest.mock import patch

from scripts import probe_opencode_v2_tui_identity as probe


class OpenCodeV2TuiIdentityProbeTests(unittest.TestCase):
    def _row(self, *, seq=1, timestamp=10.0, route="session", session_hash=None,
             top_level=True, project_hash="f" * 64, client_nonce="c" * 32,
             tui_instance="d" * 32, kind="poll"):
        return {
            "schema": 1,
            "kind": kind,
            "client_nonce": client_nonce,
            "tui_instance": tui_instance,
            "seq": seq,
            "monotonic_ms": timestamp,
            "route_name": route,
            "session_id_hash": session_hash or "a" * 64,
            "session_top_level": top_level,
            "project_hash": project_hash,
        }

    def test_parser_accepts_hash_only_exact_project_and_client_instance(self):
        nonce = "c" * 32
        project = "f" * 64
        plugin_row = self._row(seq=1, timestamp=10.0, route="plugin", session_hash=None,
                               top_level=None, kind="plugin_loaded", client_nonce=nonce,
                               project_hash=project)
        plugin_row["session_id_hash"] = None
        payload = b"\n".join(json.dumps(row).encode() for row in (
            plugin_row,
            self._row(seq=2, timestamp=110.0, session_hash="b" * 64,
                      client_nonce=nonce, project_hash=project),
        ))

        records, rejected = probe.parse_client_records(
            payload, client_nonce=nonce, project_hash=project)

        self.assertEqual(rejected, 0)
        self.assertEqual([record["seq"] for record in records], [1, 2])
        self.assertEqual(records[-1]["session_id_hash"], "b" * 64)
        self.assertNotIn("client_nonce", records[-1])
        self.assertNotIn("tui_instance", records[-1])
        self.assertNotIn("session_id", records[-1])

    def test_parser_rejects_wrong_nonce_project_instance_regressing_time_and_raw_ids(self):
        nonce = "c" * 32
        project = "f" * 64
        rows = [
            self._row(seq=1, timestamp=1.0),
            self._row(seq=2, timestamp=2.0, client_nonce="d" * 32),
            self._row(seq=3, timestamp=3.0, project_hash="e" * 64),
            self._row(seq=4, timestamp=0.5),
            {**self._row(seq=5, timestamp=5.0), "sessionID": "raw"},
            self._row(seq=6, timestamp=float("nan")),
        ]

        records, rejected = probe.parse_client_records(
            b"\n".join(json.dumps(row).encode() for row in rows),
            client_nonce=nonce, project_hash=project)

        self.assertEqual([record["seq"] for record in records], [1])
        self.assertEqual(rejected, 5)

    def test_parser_rejects_sequence_gaps_instead_of_treating_them_as_fresh_polling(self):
        first = self._row(seq=1, timestamp=1.0)
        gap = self._row(seq=3, timestamp=201.0)
        records, rejected = probe.parse_client_records(
            b"\n".join(json.dumps(row).encode() for row in (first, gap)),
            client_nonce="c" * 32, project_hash="f" * 64)
        self.assertEqual([record["seq"] for record in records], [1])
        self.assertEqual(rejected, 1)

    def test_parser_can_disable_only_plugin_clock_gap_bound(self):
        rows = [self._row(seq=1, timestamp=1.0), self._row(seq=2, timestamp=1500.0)]
        payload = b"\n".join(json.dumps(row).encode() for row in rows)
        records, rejected = probe.parse_client_records(
            payload, client_nonce="c" * 32, project_hash="f" * 64,
            enforce_gap_bound=False)
        self.assertEqual([row["seq"] for row in records], [1, 2])
        self.assertEqual(rejected, 0)
        records, rejected = probe.parse_client_records(
            payload, client_nonce="c" * 32, project_hash="f" * 64)
        self.assertEqual([row["seq"] for row in records], [1])
        self.assertEqual(rejected, 1)

    def test_parser_rejects_bool_schema_and_huge_timestamp_without_overflow(self):
        nonce, project = "c" * 32, "f" * 64
        bool_schema = {**self._row(seq=1, timestamp=1.0), "schema": True}
        huge_time = self._row(seq=1, timestamp=10**400)
        for row in (bool_schema, huge_time):
            records, rejected = probe.parse_client_records(
                json.dumps(row).encode(), client_nonce=nonce, project_hash=project)
            self.assertEqual(records, [])
            self.assertEqual(rejected, 1)

    def test_parser_rejects_record_and_byte_cap_overflows(self):
        nonce = "c" * 32
        project = "f" * 64
        row = json.dumps(self._row()).encode()
        records, rejected = probe.parse_client_records(
            b"\n".join(row for _ in range(probe.MAX_RECORDS + 1)),
            client_nonce=nonce, project_hash=project)
        self.assertEqual(records, [])
        self.assertEqual(rejected, 1)

        records, rejected = probe.parse_client_records(
            row * 1000, client_nonce=nonce, project_hash=project)
        self.assertEqual(records, [])
        self.assertEqual(rejected, 1)
        records, rejected = probe.parse_client_records(
            b"x" * (probe.MAX_RECORD_BYTES + 1), client_nonce=nonce, project_hash=project)
        self.assertEqual(records, [])
        self.assertEqual(rejected, 1)

    def test_startup_unknown_project_does_not_poison_later_verified_observations(self):
        rows = [self._row(seq=1, timestamp=1.0, project_hash=None,
                          top_level=None, kind="plugin_loaded"),
                self._row(seq=2, timestamp=2100.0)]
        records, rejected = probe.parse_client_records(
            b"\n".join(json.dumps(row).encode() for row in rows),
            client_nonce="c" * 32, project_hash="f" * 64)
        self.assertEqual(rejected, 0)
        self.assertFalse(probe.current_route(records[:1], "a" * 64, "f" * 64))
        self.assertTrue(probe.current_route(records, "a" * 64, "f" * 64))
        rows.append(self._row(seq=3, timestamp=2200.0, project_hash=None))
        _records, rejected = probe.parse_client_records(
            b"\n".join(json.dumps(row).encode() for row in rows),
            client_nonce="c" * 32, project_hash="f" * 64)
        self.assertEqual(rejected, 1)
    def test_current_route_requires_exact_hash_project_and_top_level_session(self):
        record = self._row(session_hash="b" * 64)
        self.assertTrue(probe.current_route([record], "b" * 64, "f" * 64))
        self.assertFalse(probe.current_route([record], "a" * 64, "f" * 64))
        self.assertFalse(probe.current_route([record], "b" * 64, "e" * 64))
        self.assertFalse(probe.current_route(
            [{**record, "session_top_level": False}], "b" * 64, "f" * 64))

    def test_startup_markers_are_allowlisted_booleans_and_never_return_terminal_text(self):
        secret_text = (
            "TRUST THIS FOLDER? Workspace approval required. Loading... "
            "Plugin error: panelink.v2-identity-fixture. Connect provider; no models. "
            "Invalid config. Press Enter to continue. DO_NOT_LEAK_123"
        ).encode()

        markers = probe._startup_markers(secret_text)

        expected = {
            "trust_or_folder_prompt", "workspace_approval_prompt", "loading_indicator",
            "plugin_error_visible", "error_visible", "connect_prompt", "provider_prompt",
            "no_models_prompt", "config_error_visible", "plugin_fixture_id_visible",
            "press_enter_or_continue_prompt",
        }
        self.assertEqual(set(markers), expected)
        self.assertTrue(all(markers.values()))
        self.assertTrue(all(type(value) is bool for value in markers.values()))
        self.assertNotIn("DO_NOT_LEAK_123", repr(markers))
        self.assertNotIn(secret_text.decode(), repr(markers))

    def test_client_diagnostic_adds_markers_from_capped_pty_without_raw_text(self):
        with tempfile.TemporaryDirectory(prefix="panelink-client-diagnostic-") as temporary:
            record_path = Path(temporary) / "records.jsonl"
            tui = SimpleNamespace(
                output=bytearray(b"Loading plugin panelink.v2-identity-fixture DO_NOT_LEAK"),
                alive=lambda: True,
            )

            diagnostics = probe._client_diagnostic(record_path, "c" * 32, tui)

        self.assertTrue(diagnostics["loading_indicator"])
        self.assertTrue(diagnostics["plugin_fixture_id_visible"])
        self.assertFalse(diagnostics["error_visible"])
        self.assertEqual(diagnostics["pty_output_bytes"], len(tui.output))
        self.assertNotIn("output", diagnostics)
        self.assertNotIn("DO_NOT_LEAK", repr(diagnostics))

    def test_startup_marker_scan_honors_pty_byte_cap(self):
        output = b"safe" + b"x" * probe.MAX_PTY_BYTES + b"Press Enter to continue"
        markers = probe._startup_markers(output)
        self.assertFalse(markers["press_enter_or_continue_prompt"])

    def test_selected_history_ignores_warmup_but_rejects_child_unknown_and_extra_routes(self):
        initial = {**self._row(seq=2, timestamp=200.0, session_hash="a" * 64),
                   "session_top_level": True}
        warmup = {**self._row(seq=1, timestamp=100.0, route="home", session_hash=None,
                              top_level=None), "session_id_hash": None}
        to_b = self._row(seq=3, timestamp=300.0, session_hash="b" * 64)
        to_a = self._row(seq=4, timestamp=400.0, session_hash="a" * 64)

        self.assertTrue(probe.selected_history_matches(
            [warmup, initial, to_b, {**to_b, "seq": 4, "monotonic_ms": 400.0},
             {**to_a, "seq": 5, "monotonic_ms": 500.0}],
            2, "a" * 64, ["a" * 64, "b" * 64],
            ["b" * 64, "a" * 64], "f" * 64))
        self.assertFalse(probe.selected_history_matches(
            [initial, {**to_b, "session_top_level": False}],
            2, "a" * 64, ["a" * 64, "b" * 64], ["b" * 64], "f" * 64))
        self.assertFalse(probe.selected_history_matches(
            [initial, {**to_b, "session_id_hash": "c" * 64}],
            2, "a" * 64, ["a" * 64, "b" * 64], ["b" * 64], "f" * 64))
        self.assertFalse(probe.selected_history_matches(
            [initial, to_b, {**to_a, "seq": 4, "monotonic_ms": 400.0}],
            2, "a" * 64, ["a" * 64, "b" * 64], ["b" * 64], "f" * 64))

    def test_passive_history_ignores_warmup_and_requires_fresh_same_route(self):
        warmup = {**self._row(seq=1, timestamp=100.0, route="home", session_hash=None,
                              top_level=None), "session_id_hash": None}
        first_b = self._row(seq=2, timestamp=200.0, session_hash="b" * 64)
        second_b = {**first_b, "seq": 3, "monotonic_ms": 300.0}

        self.assertTrue(probe.passive_history_matches(
            [warmup, first_b, second_b], 2, "b" * 64, "f" * 64))
        self.assertFalse(probe.passive_history_matches(
            [warmup, first_b, {**second_b, "session_id_hash": "a" * 64}],
            2, "b" * 64, "f" * 64))

    def test_picker_opens_with_ctrl_x_l_and_waits_for_expected_current_route(self):
        tui = type("Tui", (), {"screen_text": lambda _self: "Sessions\nA\nB"})()
        with patch.object(probe, "_send_control", return_value=True) as send, \
                patch.object(probe, "_wait_until", side_effect=lambda predicate, *_args: predicate()), \
                patch.object(probe, "session_list_visible", return_value=True), \
                patch.object(probe, "_fresh_expected", return_value=True) as fresh:
            self.assertTrue(probe._open_picker(
                tui, [tui], time.monotonic() + 1.0, "f" * 64,
                lambda _index: [], "b" * 64, ("A", "B"), 10))
        send.assert_called_once_with(tui, b"\x18l", unittest.mock.ANY)
        self.assertEqual(fresh.call_args.args[1], "b" * 64)

    def test_picker_guard_excludes_underlying_session_header_but_keeps_other_rows(self):
        screen = "Panelink V2 Identity A\nSessions\nPanelink V2 Identity B\n"
        picker = probe._picker_screen_text(screen)
        self.assertTrue(probe.filtered_session_row_visible(
            picker, "Panelink V2 Identity B", "Panelink V2 Identity A"))
        self.assertFalse(probe.filtered_session_row_visible(
            picker + "Panelink V2 Identity A", "Panelink V2 Identity B", "Panelink V2 Identity A"))
        self.assertEqual(probe._picker_screen_text("Panelink V2 Identity A"), "")

    def test_mocked_two_tui_driver_observes_keyboard_A_B_A_and_passive_B(self):
        self._assert_mocked_driver(passive_drift=False, expected="proven")

    def test_mocked_two_tui_driver_stops_when_passive_client_changes_route(self):
        self._assert_mocked_driver(passive_drift=True, expected="not_proven")

    def test_mocked_two_tui_route_proof_is_downgraded_when_cleanup_fails(self):
        self._assert_mocked_driver(passive_drift=False, cleanup_ok=False, expected="blocked")

    def test_reverse_new_proves_b_a_b_and_records_home_without_selected_id(self):
        result = self._run_mocked_driver(scenario="reverse-new")
        self.assertEqual(result["classification"], "proven", result)
        self.assertEqual(result["active_client_index"], 1)
        self.assertTrue(result["active_round_trip_observed"])
        self.assertFalse(result["first_tui_A_B_A_observed"])
        self.assertTrue(result["passive_identity_unchanged"])
        self.assertEqual(result["new_session_outcome"], "home_without_selected_session_id")
        self.assertEqual(result["prompt_submissions"], 0)

    def test_reverse_new_rejects_passive_identity_drift(self):
        result = self._run_mocked_driver(scenario="reverse-new", passive_drift=True)
        self.assertEqual(result["classification"], "not_proven")
        self.assertFalse(result["passive_identity_unchanged"])

    def test_reverse_new_route_requires_exact_read_only_lookup_proof(self):
        result = self._run_mocked_driver(scenario="reverse-new", new_route="ses_new")
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["new_session_outcome"], "new_route_lookup_not_proven")
        self.assertNotIn("ses_new", repr(result))

    def test_reverse_new_rejects_known_fixture_route_and_unissued_or_unobserved_key(self):
        known_route = self._run_mocked_driver(scenario="reverse-new", new_route="ses_aa")
        self.assertEqual(known_route["classification"], "not_proven")
        self.assertEqual(known_route["new_session_outcome"], "existing_fixture_session_selected")

        unverified = self._run_mocked_driver(scenario="reverse-new", key_verified=False)
        self.assertEqual(unverified["classification"], "not_proven")
        self.assertEqual(unverified["new_session_control_count"], 0)

        failed_write = self._run_mocked_driver(scenario="reverse-new", key_write_fails=True)
        self.assertEqual(failed_write["classification"], "not_proven")
        self.assertEqual(failed_write["new_session_control_count"], 0)

        unchanged = self._run_mocked_driver(scenario="reverse-new", new_route="unchanged")
        self.assertEqual(unchanged["classification"], "not_proven")
        self.assertEqual(unchanged["new_session_outcome"], "route_change_not_observed")

    def test_reverse_new_requires_passive_freshness_after_new_control(self):
        result = self._run_mocked_driver(scenario="reverse-new", new_route="ses_new",
                                        pause_passive_after_new=True)
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["new_session_outcome"],
                         "passive_identity_not_fresh_after_new_control")
        self.assertFalse(result["passive_identity_unchanged"])

        def lookup(route_hash):
            self.assertEqual(route_hash, probe.hash_value("ses_new"))
            return {"exact_session": True, "project_matches": True,
                    "top_level": True, "status": "found"}

        result = self._run_mocked_driver(scenario="reverse-new", new_route="ses_new",
                                        lookup_route_hash=lookup)
        self.assertEqual(result["classification"], "proven", result)
        self.assertEqual(result["new_session_outcome"], "new_top_level_session_verified")
        self.assertEqual(result["new_route_lookup"], {
            "exact_session": True, "project_matches": True,
            "top_level": True, "status": "found"})
        self.assertNotIn("ses_new", repr(result))

    def test_after_routes_hook_runs_before_cleanup_and_unproven_health_downgrades(self):
        observed = {}

        def after_routes(ctx):
            observed.update({"alive": all(tui.alive() for tui in ctx["tuis"]),
                             "pump": ctx["pump"](0.05),
                             "control_count": len(ctx["control_paths"]),
                             "record_count": len(ctx["record_paths"]),
                             "hash_count": len(ctx["hashes"])})
            return {"classification": "blocked", "reason": "health_expired"}

        result = self._run_mocked_driver(after_routes=after_routes)
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["reason"], "bridge_health_not_proven")
        self.assertEqual(result["bridge_health"]["reason"], "health_expired")
        self.assertTrue(observed["alive"])
        self.assertTrue(observed["pump"])
        self.assertEqual(observed["control_count"], 2)
        self.assertEqual(observed["record_count"], 2)
        self.assertEqual(observed["hash_count"], 2)

    def test_identity_hooks_have_distinct_results_and_fail_closed(self):
        for scenario, key in (("child-routes", "child_routes"),
                              ("project-isolation", "project_isolation")):
            for classification in ("proven", "not_proven"):
                result = self._run_mocked_driver(
                    scenario=scenario, hook_result_key=key,
                    after_routes=lambda context: {"classification": classification})
                self.assertEqual(result["classification"],
                                 "proven" if classification == "proven" else "not_proven")
                self.assertEqual(result[key]["classification"], classification)
                self.assertNotIn("bridge_health", result)
                self.assertTrue(result["tui_cleanup_completed"])

    def _assert_mocked_driver(self, *, passive_drift, expected, cleanup_ok=True):
        result = self._run_mocked_driver(passive_drift=passive_drift, cleanup_ok=cleanup_ok)
        self.assertEqual(result["classification"], expected, result)
        self.assertEqual(result["attempted_control_count"], 2 if not passive_drift else 1)
        self.assertEqual(result["tui_cleanup_completed"], cleanup_ok)
        if not passive_drift and cleanup_ok:
            self.assertTrue(result["first_tui_A_B_A_observed"])
            self.assertTrue(result["second_tui_stayed_B"])
            self.assertEqual(result["switch_latency_upper_bounds_ms"], [100.0, 100.0])
        elif passive_drift:
            self.assertEqual(result["reason"], "passive_tui_fresh_route_not_observed_during_switch")
        else:
            self.assertEqual(result["reason"], "tui_process_cleanup_unverified")

    def _run_mocked_driver(self, *, scenario="baseline", passive_drift=False,
                           cleanup_ok=True, after_routes=None, new_route=None,
                           lookup_route_hash=None, key_verified=None,
                           key_write_fails=False, pause_passive_after_new=False,
                           hook_result_key="bridge_health"):
        ids = ("ses_aa", "ses_bb")
        hashes = [probe.hash_value(value) for value in ids]
        live_tuis = []
        new_session_sent = [False]

        def append_record(tui, route_hash=None, *, kind="poll", route="session", top_level=True):
            tui.seq += 1
            tui.timestamp += 100.0
            route = "home" if tui.mode == "home" else route
            row = {
                "schema": 1, "kind": kind,
                "client_nonce": tui.nonce, "tui_instance": tui.instance,
                "seq": tui.seq, "monotonic_ms": tui.timestamp,
                "route_name": route,
                "session_id_hash": None if tui.mode == "home" else route_hash,
                "session_top_level": None if tui.mode == "home" else top_level,
                "project_hash": tui.project_hash,
            }
            with tui.record_path.open("ab") as stream:
                stream.write(json.dumps(row).encode() + b"\n")

        class FakePty:
            def __init__(self, argv, cwd, env):
                config = json.loads((Path(env["OPENCODE_CONFIG_DIR"]) / "cli.json").read_text())
                if set(config) != {"plugins"} or len(config["plugins"]) != 1:
                    raise AssertionError("V2 CLI configuration requires plugins field")
                if not (Path(config["plugins"][0]) / "tui.ts").is_file():
                    raise AssertionError("V2 CLI plugin must resolve to a package directory with a TUI entry")
                self.index = len(live_tuis)
                self.master = -1
                self.output = bytearray()
                self.child = object()
                self.alive_value = True
                self.mode = "main"
                self.typed = ""
                self.project_hash = probe.hash_value(str(cwd.resolve()))
                self.nonce = env["PANELINK_PROBE_CLIENT_NONCE"]
                self.instance = ("a" if self.index == 0 else "b") * 32
                self.record_path = Path(env["PANELINK_PROBE_RECORDS"])
                self.control_path = Path(env["PANELINK_PROBE_CONTROL"])
                if self.control_path.exists():
                    raise AssertionError("Control path must be absent until a health pause is requested")
                self.seq = 0
                self.timestamp = 0.0
                session_id = argv[argv.index("--session") + 1]
                self.current_hash = probe.hash_value(session_id)
                self.screen = ""
                append_record(self, kind="plugin_loaded", route="plugin", top_level=None)
                append_record(self, self.current_hash)
                live_tuis.append(self)

            def alive(self):
                return self.alive_value

            def drain(self, _timeout=0.0):
                return b""

            def screen_text(self):
                if self.mode == "home":
                    return "OpenCode Home"
                if self.mode == "picker":
                    return "Sessions\nPanelink V2 Identity A\nPanelink V2 Identity B"
                if self.mode == "filtered":
                    return "Sessions\n" + self.typed
                return "OpenCode"

            def close(self):
                self.alive_value = False

        def send_control(tui, payload, _deadline):
            if payload == b"\x18l":
                tui.mode = "picker"
                tui.typed = ""
            elif payload == b"\x18n":
                if key_write_fails:
                    return False
                new_session_sent[0] = True
                if new_route != "unchanged":
                    tui.mode = "main" if new_route else "home"
                    tui.current_hash = (probe.hash_value(new_route) if new_route else None)
            elif payload == b"\r":
                target = 1 if tui.typed == probe.LABELS[1] else 0
                tui.current_hash = hashes[target]
                if passive_drift and len(live_tuis) > 1:
                    live_tuis[1].current_hash = hashes[0]
            else:
                tui.typed += payload.decode("ascii")
                tui.mode = "filtered"
            return True

        def wait_until(predicate, tuis, _deadline):
            for active in tuis:
                passive_index = 0 if scenario == "reverse-new" else 1
                if (pause_passive_after_new and new_session_sent[0]
                        and active.index == passive_index):
                    continue
                append_record(active, active.current_hash)
            return bool(predicate())

        with tempfile.TemporaryDirectory(prefix="panelink-mocked-tuis-") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            project = root / "project"
            project.mkdir(mode=0o700)
            context = {
                "root": root, "project": project,
                "env": {"OPENCODE_PASSWORD": "test-only-secret"},
                "base": "http://127.0.0.1:12345", "session_ids": ids,
                "deadline": time.monotonic() + 50.0,
                "executable": "/fake/opencode", "launcher": "opencode",
                "scenario": scenario, "after_routes": after_routes,
                "hook_result_key": hook_result_key,
                "new_session_key_verified": (scenario == "reverse-new" if key_verified is None
                                              else key_verified),
                "lookup_route_hash": lookup_route_hash,
            }
            with patch.object(probe, "PrivatePty", FakePty), \
                    patch.object(probe, "_send_control", side_effect=send_control), \
                    patch.object(probe, "_wait_until", side_effect=wait_until), \
                    patch.object(probe.bootstrap, "_kill_group", return_value=cleanup_ok):
                result = probe.after_bootstrap(context)
        return result

    def test_control_writer_is_bounded_and_writes_only_requested_keys(self):
        read_fd, write_fd = os.pipe()
        try:
            self.assertTrue(probe._send_control(
                type("Tui", (), {"master": write_fd})(), b"\x18l", time.monotonic() + 1.0))
            self.assertEqual(os.read(read_fd, 2), b"\x18l")
            self.assertFalse(probe._send_control(
                type("Tui", (), {"master": write_fd})(),
                b"x" * (probe.MAX_CONTROL_BYTES + 1), time.monotonic() + 1.0))
        finally:
            os.close(read_fd)
            os.close(write_fd)

    def test_run_requests_the_tui_callback_variant_of_verified_bootstrap(self):
        with patch.object(probe.bootstrap, "run", return_value=(2, {"reason": "mock"})) as run:
            self.assertEqual(probe.run("/bin/opencode", which=lambda _name: None,
                                       home=Path("/private/home")), (2, {"reason": "mock"}))
        run.assert_called_once_with("/bin/opencode", which=unittest.mock.ANY,
                                    home=Path("/private/home"), tui=True,
                                    scenario="baseline")


if __name__ == "__main__":
    unittest.main()
