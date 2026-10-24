import unittest

from scripts import probe_panelink_manual_recovery as probe


class ManualRecoveryProbeTests(unittest.TestCase):
    def test_fixed_manual_failures_are_distinct_from_uncertain_results(self):
        missing = {
            "valid": True, "returncode": 1,
            "state": "manual_selection_required",
            "error": "provider_session_not_found",
            "automatic_resume": False, "current_at_crash": False,
        }
        self.assertTrue(probe._manual_error(missing, "provider_session_not_found"))
        uncertain = {**missing, "state": "uncertain", "error": None,
                     "returncode": 2}
        self.assertFalse(probe._manual_error(uncertain, "provider_session_not_found"))
        self.assertFalse(probe._manual_error(missing, "provider_project_mismatch"))

    def test_public_evidence_drops_ticket_ids_paths_and_raw_provider_fields(self):
        raw = {
            "classification": "proven", "reason": "explicit_recovery_gates_observed",
            "ticket": "a" * 32, "provider_session_id": "ses_private_fixture",
            "project": "/private/fixture", "id": "ses_private_fixture",
            "fork_parent_link_verified": True, "fork_history_hash_verified": True,
            "all_user_ids_sha256": ["b" * 64], "cleanup_completed": True,
        }
        public = probe._public_evidence(raw)
        self.assertEqual(public["classification"], "proven")
        self.assertEqual(public["all_user_ids_sha256"], ["b" * 64])
        self.assertTrue(public["fork_parent_link_verified"])
        self.assertTrue(public["fork_history_hash_verified"])
        encoded = str(public)
        for secret in ("a" * 32, "ses_private_fixture", "/private/fixture", "provider_session_id"):
            self.assertNotIn(secret, encoded)

    def test_unavailable_native_binary_fails_closed_without_external_work(self):
        code, report = probe.run("/nonexistent/panelink", executable="/nonexistent/opencode")
        self.assertEqual(code, 2)
        self.assertEqual(report["classification"], "blocked")
        self.assertEqual(report["reason"], "native_opencode_missing")
        self.assertTrue(report["no_prompt_submissions"])
        self.assertFalse(report["cleanup_completed"])


if __name__ == "__main__":
    unittest.main()
