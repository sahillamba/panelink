import unittest

from scripts import probe_panelink_bridge_failure as probe


class ManagedBridgeFailureProbeTests(unittest.TestCase):
    def test_public_projection_drops_process_ids_paths_and_session_data(self):
        raw = {
            "classification": "proven", "reason": "owned_tui_failure_latched_and_source_preserved",
            "tui_pid": 123456, "project": "/private/fixture/project",
            "session_id": "ses_private", "failure_status": "unavailable",
            "failure_revision_unchanged": True, "historical_source_a_exact": True,
        }
        projected = probe._public(raw)
        self.assertEqual(projected["failure_status"], "unavailable")
        self.assertTrue(projected["failure_revision_unchanged"])
        self.assertTrue(projected["historical_source_a_exact"])
        encoded = str(projected)
        for private in ("123456", "/private/fixture", "ses_private", "tui_pid", "session_id"):
            self.assertNotIn(private, encoded)

    def test_process_state_refuses_unverified_or_dead_pid(self):
        self.assertIsNone(probe._process_state(-1, 1))

    def test_missing_native_tools_fails_closed_without_launch(self):
        code, report = probe.run("/nonexistent/panelink", executable="/nonexistent/opencode")
        self.assertEqual(code, 2)
        self.assertEqual(report["classification"], "blocked")
        self.assertEqual(report["reason"], "native_opencode_missing")
        self.assertTrue(report["no_prompt_submissions"])
        self.assertFalse(report["cleanup_completed"])


if __name__ == "__main__":
    unittest.main()
