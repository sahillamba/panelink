from __future__ import annotations

import threading
import unittest

from scripts.probe_panelink_api_mutation_liveness import (
    PumpMetrics,
    _fixed_result,
    _join_mutation_worker,
    pump_once,
    pump_until,
    promoted_binding_matches,
)


class FakeServe:
    def __init__(self) -> None:
        self.polls = 0

    def poll(self, _timeout: float = 0.0) -> None:
        self.polls += 1


class FakeAttach:
    def __init__(self, alive: bool = True) -> None:
        self.is_alive = alive
        self.drains = 0

    def alive(self) -> bool:
        return self.is_alive

    def drain(self, _timeout: float = 0.0) -> bytes:
        self.drains += 1
        return b""


class PanelinkApiMutationLivenessTests(unittest.TestCase):
    def test_cleanup_accounting_requires_mutation_worker_termination(self) -> None:
        entered = threading.Event()
        release = threading.Event()

        def blocked_mutation() -> None:
            entered.set()
            release.wait()

        worker = threading.Thread(target=blocked_mutation, daemon=True)
        worker.start()
        self.assertTrue(entered.wait(timeout=1))
        self.assertFalse(_join_mutation_worker(worker, timeout=0.01))

        release.set()
        self.assertTrue(_join_mutation_worker(worker, timeout=1))

    def test_each_owner_pump_polls_serve_and_drains_attach(self) -> None:
        attach = FakeAttach()
        serve = FakeServe()
        metrics = PumpMetrics()

        pump_once(attach, [serve], metrics, timeout=0)
        pump_once(attach, [serve], metrics, timeout=0)

        self.assertEqual(serve.polls, 2)
        self.assertEqual(attach.drains, 2)
        self.assertEqual(metrics.turns, 2)
        self.assertGreaterEqual(metrics.max_gap_ms, 0)

    def test_mutation_pump_keeps_polling_if_attach_exits(self) -> None:
        attach = FakeAttach(alive=False)
        serve = FakeServe()
        metrics = PumpMetrics()

        pump_once(attach, [serve], metrics, timeout=0)

        self.assertEqual(serve.polls, 1)
        self.assertEqual(attach.drains, 0)
        self.assertEqual(metrics.turns, 1)

    def test_report_schema_contains_only_fixed_evidence_fields(self) -> None:
        result = _fixed_result()
        self.assertEqual(result["prompt_submissions"], 0)
        self.assertEqual(result["initial_revision"], None)
        self.assertNotIn("session_id", result)
        self.assertNotIn("screen_text", result)
        self.assertNotIn("api_response", result)
        self.assertIn("attach_alive_after_picker", result)
        self.assertIn("provider_status_after_picker", result)

    def test_disk_row_waits_for_matching_runtime_ack(self) -> None:
        row = {"revision": 2}
        pending = {"status": "pending", "last_confirmed_revision": 1}
        self.assertFalse(promoted_binding_matches(row, pending, 1))
        self.assertFalse(promoted_binding_matches(
            row, {"status": "last_confirmed", "last_confirmed_revision": 3}, 1,
        ))
        self.assertTrue(promoted_binding_matches(
            row, {"status": "last_confirmed", "last_confirmed_revision": 2}, 1,
        ))


if __name__ == "__main__":
    unittest.main()
