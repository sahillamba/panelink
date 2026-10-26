import json
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import probe_opencode_v2_bridge_health as health


class OpenCodeV2BridgeHealthTests(unittest.TestCase):
    nonce = "c" * 32
    project = "f" * 64
    instance = "d" * 32

    def _row(self, seq, *, nonce=None, project=None, instance=None, route="session",
             session_hash="a" * 64, top_level=True, timestamp=None):
        return {
            "schema": 1, "kind": "poll", "client_nonce": nonce or self.nonce,
            "tui_instance": instance or self.instance, "seq": seq,
            "monotonic_ms": timestamp if timestamp is not None else seq * 100.0,
            "route_name": route, "session_id_hash": session_hash,
            "session_top_level": top_level, "project_hash": project or self.project,
        }

    def _stream(self, last, **changes):
        return b"\n".join(json.dumps(self._row(seq, **changes)).encode()
                          for seq in range(1, last + 1))

    def test_duplicate_reread_does_not_renew_host_receipt_and_expiry_is_ineligible(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        first = self._stream(1)
        self.assertTrue(monitor.accept_stream(first, now=10.0))
        self.assertFalse(monitor.accept_stream(first, now=10.9))
        self.assertEqual(monitor.received_at, 10.0)
        snapshot = monitor.snapshot(11.01)
        self.assertEqual(snapshot["state"], "stale")
        self.assertFalse(snapshot["current_identity_eligible"])
        self.assertFalse(snapshot["durable"])

    def test_unknown_identity_inside_batch_requires_explicit_reconciliation(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        monitor.accept_stream(self._stream(1), now=10.0)
        rows = [self._row(1), self._row(2, route="home", session_hash=None, top_level=None),
                self._row(3)]
        stream = b"\n".join(json.dumps(row).encode() for row in rows)
        self.assertTrue(monitor.accept_stream(stream, now=10.2))
        self.assertFalse(monitor.snapshot(10.2)["current_identity_eligible"])
        monitor.request_reconciliation()
        stream += b"\n" + json.dumps(self._row(4)).encode()
        self.assertTrue(monitor.accept_stream(stream, now=10.3))
        self.assertTrue(monitor.snapshot(10.3)["current_identity_eligible"])

    def test_valid_contiguous_batch_renews_from_first_unseen_sequence(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        self.assertTrue(monitor.accept_stream(self._stream(2), now=10.0))
        self.assertEqual(monitor.sequence, 2)
        self.assertEqual(monitor.advances, 2)
        # A later file read can contain several new contiguous rows. The whole
        # file is validated, then every row after the old watermark is consumed.
        self.assertTrue(monitor.accept_stream(self._stream(7), now=10.5))
        self.assertEqual(monitor.sequence, 7)
        self.assertEqual(monitor.advances, 7)
        self.assertEqual(monitor.received_at, 10.5)

    def test_wrong_scope_sequence_gap_and_new_instance_fail_closed(self):
        cases = [
            self._stream(1, nonce="e" * 32),
            b"\n".join(json.dumps(row).encode() for row in (
                self._row(1), self._row(3, timestamp=300.0))),
            b"\n".join(json.dumps(row).encode() for row in (
                self._row(1), self._row(2, instance="e" * 32))),
            self._stream(1, project="e" * 64),
        ]
        for payload in cases:
            with self.subTest(payload=payload[-80:]):
                monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
                self.assertFalse(monitor.accept_stream(payload, now=1.0))
                self.assertEqual(monitor.state, "stale")
                self.assertTrue(monitor.ambiguous)
                self.assertFalse(monitor.snapshot(1.0)["current_identity_eligible"])

    def test_source_version_is_pinned(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        self.assertFalse(monitor.accept_stream(
            self._stream(1), now=1.0, version="2.0.31"))
        self.assertEqual(monitor.state, "stale")
        self.assertTrue(monitor.ambiguous)

    def test_regressing_sequence_marks_existing_observation_stale_and_ambiguous(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        self.assertTrue(monitor.accept_stream(self._stream(2), now=1.0))
        self.assertFalse(monitor.accept_stream(self._stream(1), now=1.1))
        snapshot = monitor.snapshot(1.1)
        self.assertEqual(snapshot["state"], "stale")
        self.assertTrue(snapshot["ambiguous"])
        self.assertFalse(snapshot["current_identity_eligible"])

    def test_late_new_row_expires_before_acceptance_even_without_prior_snapshot(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        self.assertTrue(monitor.accept_stream(self._stream(1), now=1.0))
        self.assertTrue(monitor.accept_stream(self._stream(2), now=3.0))
        snapshot = monitor.snapshot(3.0)
        self.assertEqual(snapshot["state"], "stale")
        self.assertTrue(snapshot["ambiguous"])
        self.assertFalse(snapshot["current_identity_eligible"])

    def test_fresh_rows_after_loss_stay_stale_until_new_post_request_row(self):
        monitor = health.ObserverHealth(nonce=self.nonce, project_hash=self.project)
        self.assertTrue(monitor.accept_stream(self._stream(1), now=1.0))
        monitor.lose()
        self.assertTrue(monitor.accept_stream(self._stream(2), now=1.2))
        self.assertEqual(monitor.state, "stale")
        self.assertFalse(monitor.snapshot(1.2)["current_identity_eligible"])
        watermark = monitor.request_reconciliation()
        self.assertEqual(watermark, 2)
        self.assertFalse(monitor.accept_stream(self._stream(2), now=1.3))
        self.assertTrue(monitor.accept_stream(self._stream(3), now=1.4))
        snapshot = monitor.snapshot(1.4)
        self.assertEqual(snapshot["state"], "observed")
        self.assertTrue(snapshot["current_identity_eligible"])
        self.assertFalse(snapshot["durable"])

    def test_bounded_reader_caps_input_before_parsing(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "records"
            path.write_bytes(b"x" * (health.MAX_RECORD_BYTES + 1))
            self.assertIsNone(health.bounded_read(path))

    def _run_mocked_hook(self, *, inject_invalid_active=False):
        class FakeTui:
            def alive(self):
                return True

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            paths = [root / "a.jsonl", root / "b.jsonl"]
            controls = [root / "a.control", root / "b.control"]
            hashes = ["a" * 64, "b" * 64]
            rows = [[self._row(1, session_hash=hashes[index])]
                    for index in range(2)]
            for path, stream in zip(paths, rows):
                path.write_text(json.dumps(stream[0]) + "\n", encoding="utf-8")
                path.chmod(0o600)
            counts = [1, 1]
            injected = False

            def pump():
                nonlocal injected
                for index in (0, 1):
                    if index == 0 and controls[0].exists():
                        if inject_invalid_active and not injected:
                            bad_row = self._row(2, nonce="e" * 32,
                                                session_hash=hashes[0], timestamp=200.0)
                            paths[0].write_text(
                                json.dumps(rows[0][0]) + "\n" + json.dumps(bad_row) + "\n",
                                encoding="utf-8")
                            injected = True
                        continue
                    counts[index] += 1
                    rows[index].append(self._row(
                        counts[index], session_hash=hashes[index],
                        timestamp=counts[index] * 100.0))
                    paths[index].write_text(
                        "\n".join(json.dumps(row) for row in rows[index]) + "\n",
                        encoding="utf-8")

            context = {
                "record_paths": paths, "nonces": [self.nonce, self.nonce],
                "project_hash": self.project, "tuis": [FakeTui(), FakeTui()],
                "hashes": hashes, "control_paths": controls,
                "server_pid": 42, "port": 12345,
                "deadline": time.monotonic() + 5.0, "pump": pump,
            }
            with patch.object(health.tui.bootstrap, "server_owns_port", return_value=True):
                result = health.after_routes(context)
            return result

    def test_bounded_hook_pauses_only_active_observer_and_requires_new_reconciliation_row(self):
        result = self._run_mocked_hook()

        self.assertEqual(result["classification"], "proven", result)
        self.assertTrue(result["host_receipt_expiry_observed"])
        self.assertTrue(result["passive_observer_remained_fresh"])
        self.assertTrue(result["both_tuis_remained_alive"])
        self.assertTrue(result["owned_server_remained_alive"])
        self.assertFalse(result["automatic_promotion_after_resume"])
        self.assertTrue(result["explicit_fresh_reconciliation_observed"])
        self.assertTrue(result["current_identity_eligible_after_reconciliation"])
        self.assertFalse(result["durable"])

    def test_invalid_active_scope_cannot_masquerade_as_transport_expiry(self):
        result = self._run_mocked_hook(inject_invalid_active=True)
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["reason"], "active_observer_data_invalid_during_loss_case")
        self.assertFalse(result["host_receipt_expiry_observed"])
        self.assertFalse(result["explicit_fresh_reconciliation_observed"])


if __name__ == "__main__":
    unittest.main()
