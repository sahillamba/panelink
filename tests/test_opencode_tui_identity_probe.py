import json
import socket
import unittest

from scripts.probe_opencode_identity import resolve_executable
from scripts.probe_opencode_tui_identity import (
    classify_repetition,
    correlate_switches,
    filtered_session_row_visible,
    launcher_argv,
    open_loopback_listener,
    parse_records,
    session_list_visible,
)


class OpenCodeTuiIdentityProbeTests(unittest.TestCase):
    def test_parse_records_accepts_hash_only_records_and_rejects_raw_ids(self):
        valid_hash = "a" * 64
        payload = b"\n".join(
            json.dumps(record).encode()
            for record in (
                {"seq": 1, "monotonic_ms": 1.25, "route_name": "session",
                 "session_id_hash": valid_hash},
                {"seq": 2, "monotonic_ms": 2.5, "route_name": "home",
                 "session_id_hash": None, "sessionID": "must-not-be-kept"},
                {"seq": 3, "monotonic_ms": 3, "route_name": "session",
                 "session_id_hash": valid_hash},
            )
        )

        records, rejected = parse_records(payload)

        self.assertEqual([record["seq"] for record in records], [1, 3])
        self.assertEqual(records[0]["session_id_hash"], valid_hash)
        self.assertNotIn("session_id", records[0])
        self.assertEqual(rejected, 1)

    def test_parse_records_rejects_invalid_hashes_duplicate_sequences_and_bad_json(self):
        payload = b"\n".join((
            b'{"seq":1,"monotonic_ms":1,"route_name":"home","session_id_hash":null}',
            b'{"seq":1,"monotonic_ms":2,"route_name":"home","session_id_hash":null}',
            b'{"seq":2,"monotonic_ms":3,"route_name":"session","session_id_hash":"raw-id"}',
            b"not-json",
        ))

        records, rejected = parse_records(payload)

        self.assertEqual([record["seq"] for record in records], [1])
        self.assertEqual(rejected, 3)

    def test_correlate_switches_observes_order_and_latency(self):
        a_hash, b_hash = "a" * 64, "b" * 64
        records = [
            {"seq": 1, "monotonic_ms": 100.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 160.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 3, "monotonic_ms": 225.0, "route_name": "session", "session_id_hash": b_hash},
            {"seq": 4, "monotonic_ms": 310.0, "route_name": "session", "session_id_hash": b_hash},
            {"seq": 5, "monotonic_ms": 360.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        result = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 2, "issued_ms": 225.0}, {"after_seq": 4, "issued_ms": 310.0}],
        )

        self.assertTrue(result["initial_route_captured"])
        self.assertTrue(result["switch_A_B_A_observed"])
        self.assertTrue(result["latency_bounds_passed"])
        self.assertEqual([item["latency_ms"] for item in result["transitions"]], [0.0, 50.0])
        self.assertEqual(result["missed_transition_count"], 0)
        self.assertEqual(result["extra_transition_count"], 0)
        self.assertEqual(result["attempted_control_count"], 2)
        self.assertEqual(result["unissued_control_count"], 0)
        self.assertEqual(result["missed_attempt_count"], 0)

    def test_correlate_switches_flags_missed_extra_and_slow_transitions(self):
        a_hash, b_hash, c_hash = "a" * 64, "b" * 64, "c" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 2.0, "route_name": "session", "session_id_hash": c_hash},
            {"seq": 3, "monotonic_ms": 6002.0, "route_name": "session", "session_id_hash": b_hash},
        ]

        result = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": 3, "issued_ms": 6003.0}],
        )

        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertFalse(result["latency_bounds_passed"])
        self.assertEqual(result["missed_transition_count"], 1)
        self.assertGreaterEqual(result["extra_transition_count"], 1)

    def test_unissued_control_cannot_turn_incidental_routes_into_full_switch_proof(self):
        a_hash, b_hash = "a" * 64, "b" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 2.0, "route_name": "session", "session_id_hash": b_hash},
            {"seq": 3, "monotonic_ms": 3.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        result = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}],
        )

        self.assertTrue(result["transitions"][0]["observed"])
        self.assertFalse(result["transitions"][1]["attempted"])
        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertFalse(result["latency_bounds_passed"])
        self.assertEqual(result["attempted_control_count"], 1)
        self.assertEqual(result["unissued_control_count"], 1)
        self.assertEqual(result["missed_attempt_count"], 0)

        no_attempt = correlate_switches(records, a_hash, [b_hash, a_hash], [])
        self.assertFalse(no_attempt["latency_bounds_passed"])
        self.assertEqual(no_attempt["attempted_control_count"], 0)
        self.assertEqual(no_attempt["unissued_control_count"], 2)

    def test_second_observation_cannot_complete_chain_when_first_attempt_was_missed(self):
        a_hash, b_hash = "a" * 64, "b" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 2.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 3, "monotonic_ms": 3.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        result = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": 1, "issued_ms": 2.0}],
        )

        self.assertFalse(result["transitions"][0]["observed"])
        self.assertTrue(result["transitions"][1]["observed"])
        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertFalse(result["latency_bounds_passed"])
        self.assertEqual(result["attempted_control_count"], 2)
        self.assertEqual(result["missed_attempt_count"], 1)

    def test_regressing_marker_cannot_match_data_before_prior_matched_sequence(self):
        a_hash, b_hash = "a" * 64, "b" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 2.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 3, "monotonic_ms": 3.0, "route_name": "session", "session_id_hash": b_hash},
            {"seq": 4, "monotonic_ms": 4.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        result = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": 1, "issued_ms": 2.0}],
        )

        self.assertEqual([item["observed"] for item in result["transitions"]], [True, False])
        self.assertEqual([item["latency_ms"] for item in result["transitions"]], [1.0, None])
        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertEqual(result["invalid_marker_count"], 1)
        self.assertEqual(result["missed_attempt_count"], 1)

    def test_empty_target_chain_never_counts_as_switch_or_latency_proof(self):
        a_hash = "a" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        result = correlate_switches(records, a_hash, [], [])

        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertFalse(result["latency_bounds_passed"])
        self.assertEqual(result["attempted_control_count"], 0)
        self.assertEqual(result["unissued_control_count"], 0)

    def test_extra_or_malformed_markers_fail_closed(self):
        a_hash, b_hash = "a" * 64, "b" * 64
        records = [
            {"seq": 1, "monotonic_ms": 1.0, "route_name": "session", "session_id_hash": a_hash},
            {"seq": 2, "monotonic_ms": 2.0, "route_name": "session", "session_id_hash": b_hash},
            {"seq": 3, "monotonic_ms": 3.0, "route_name": "session", "session_id_hash": a_hash},
        ]

        extra = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": 2, "issued_ms": 3.0},
             {"after_seq": 3, "issued_ms": 4.0}],
        )
        malformed = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": "2", "issued_ms": 3.0}],
        )
        time_regression = correlate_switches(
            records, a_hash, [b_hash, a_hash],
            [{"after_seq": 1, "issued_ms": 2.0}, {"after_seq": 2, "issued_ms": 1.0}],
        )

        self.assertFalse(extra["switch_A_B_A_observed"])
        self.assertEqual(extra["extra_marker_count"], 1)
        self.assertFalse(malformed["switch_A_B_A_observed"])
        self.assertFalse(malformed["latency_bounds_passed"])
        self.assertEqual(malformed["invalid_marker_count"], 1)
        self.assertEqual(malformed["missed_attempt_count"], 1)
        self.assertFalse(time_regression["switch_A_B_A_observed"])
        self.assertEqual(time_regression["invalid_marker_count"], 1)

    def test_classification_distinguishes_proven_not_proven_and_blocked(self):
        common = {"plugin_loaded": True, "initial_route_captured": True,
                  "switch_observed": True, "latency_bounds_passed": True}

        self.assertEqual(classify_repetition(**common, blocked_reason=None), "proven")
        self.assertEqual(classify_repetition(**{**common, "switch_observed": False},
                                             blocked_reason=None), "not_proven")
        self.assertEqual(classify_repetition(**common, blocked_reason="plugin_api_absent"), "blocked")

    def test_session_picker_waits_require_dialog_and_filtered_target_row(self):
        picker = "Sessions\nPanelink Identity Probe A\nPanelink Identity Probe B"
        filtered = "Sessions\nPanelink Identity Probe B\n"

        self.assertTrue(session_list_visible(
            picker, ("Panelink Identity Probe A", "Panelink Identity Probe B"),
        ))
        self.assertFalse(session_list_visible(
            "Panelink Identity Probe A\nPanelink Identity Probe B",
            ("Panelink Identity Probe A", "Panelink Identity Probe B"),
        ))
        self.assertTrue(filtered_session_row_visible(
            filtered, "Panelink Identity Probe B", "Panelink Identity Probe A",
        ))
        self.assertFalse(filtered_session_row_visible(
            picker, "Panelink Identity Probe B", "Panelink Identity Probe A",
        ))

    def test_loopback_listener_reserves_a_loopback_ephemeral_port(self):
        listener, port = open_loopback_listener()

        self.assertIsNotNone(listener)
        self.assertIsNotNone(port)
        assert listener is not None and port is not None
        try:
            self.assertEqual(listener.getsockname(), ("127.0.0.1", port))
            with socket.socket() as contender:
                with self.assertRaises(OSError):
                    contender.bind(("127.0.0.1", port))
        finally:
            listener.close()

    def test_launcher_resolution_obeys_explicit_path_and_fallback_order(self):
        usable = lambda path: path.name in {"explicit-opencode", "path-opencode", "home-opencode", "tescode"}
        which = lambda name: {"opencode": "/bin/path-opencode", "tescode": "/bin/tescode"}.get(name)

        explicit = resolve_executable(
            "/bin/explicit-opencode", which=which, home=None, executable=usable,
        )
        path = resolve_executable(which=which, home=None, executable=usable)
        home_fallback = resolve_executable(
            which=lambda _name: None, home=__import__("pathlib").Path("/home/test"),
            executable=lambda path: path.name == "opencode",
        )
        wrapper = resolve_executable(which=lambda name: "/bin/tescode" if name == "tescode" else None,
                                     home=__import__("pathlib").Path("/missing"), executable=usable)

        self.assertEqual(explicit, ("/bin/explicit-opencode", "opencode"))
        self.assertEqual(path, ("/bin/path-opencode", "opencode"))
        self.assertEqual(home_fallback, ("/home/test/.opencode/bin/opencode", "opencode"))
        self.assertEqual(wrapper, ("/bin/tescode", "tescode"))
        self.assertEqual(launcher_argv("/bin/tescode", "tescode", ["--session", "hash"]),
                         ["/bin/tescode", "workflow", "custom", "--session", "hash"])


if __name__ == "__main__":
    unittest.main()
