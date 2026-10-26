import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from scripts import probe_opencode_v2_project_isolation as probe


class ProjectIsolationTests(unittest.TestCase):
    def row(self, *, seq=1, route="session", session_hash="a" * 64,
            project_hash="b" * 64, top_level=True):
        return {"seq": seq, "route_name": route, "session_id_hash": session_hash,
                "project_hash": project_hash, "session_top_level": top_level}

    def test_exact_project_scope_and_unknown_startup_then_resolved_identity(self):
        rows = [self.row(route="session", session_hash="a" * 64,
                         project_hash=None, top_level=None), self.row(seq=2)]
        self.assertTrue(probe.scope_proof(rows, session_hash="a" * 64,
                                          project_hash="b" * 64))
        self.assertFalse(probe.scope_proof(rows, session_hash="a" * 64,
                                           project_hash="c" * 64))

    def test_missing_child_and_non_session_identity_are_rejected(self):
        for rows in ([], [self.row(top_level=False)], [self.row(route="home")],
                     [self.row(session_hash=None)]):
            self.assertFalse(probe.scope_proof(rows, session_hash="a" * 64,
                                               project_hash="b" * 64))
        self.assertFalse(probe.scope_proof(
            [self.row(), self.row(seq=2, project_hash="c" * 64)],
            session_hash="a" * 64, project_hash="b" * 64))
        self.assertFalse(probe.scope_proof(
            [self.row(), self.row(seq=2, session_hash="d" * 64, top_level=False)],
            session_hash="a" * 64, project_hash="b" * 64))

    def _run_probe(self, *, original_rows="fresh", late_empty_failure=False,
                   extra_rejected=0, lose_listener_at=None):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        project = root / "project"
        project.mkdir(mode=0o700)
        (root / "opencode-config").mkdir()
        paths = [root / "original-a.jsonl", root / "original-b.jsonl"]
        for path in paths:
            path.touch()
        context = {
            "root": root, "project": project, "deadline": 10**12,
            "port": 1234, "server_pid": 123, "base": "http://127.0.0.1:1234",
            "password": "fixture-only", "env": {}, "executable": "/bin/true",
            "launcher": "native", "record_paths": paths,
            "nonces": ["1" * 32, "2" * 32], "hashes": ["3" * 64, "4" * 64],
            "project_hash": "5" * 64,
        }

        class FakeTui:
            def __init__(self, *args):
                self.child, self.closed = object(), False

            def close(self):
                self.closed = True

            def alive(self):
                return True

        class OriginalTui:
            def alive(self):
                return True

        context["tuis"] = [OriginalTui(), OriginalTui()]

        fake = FakeTui()
        request_calls = []
        message_reads = 0

        def request(_base, method, path, **_kwargs):
            nonlocal message_reads
            request_calls.append((method, path))
            if method == "POST":
                return 200, b'{"data":{"id":"private-session","parentID":null}}'
            if path.startswith("/api/session?"):
                directory = str(root / "project-isolation")
                return 200, json.dumps({"data": [{
                    "id": "private-session", "parentID": None,
                    "location": {"directory": directory},
                }]}).encode()
            if "/message?" in path:
                message_reads += 1
                if late_empty_failure and message_reads > 1:
                    return 500, b""
                return 200, b'{"data":[]}'
            raise AssertionError("unexpected authenticated request")

        ownership_calls = 0

        def owns(_port, _pid):
            nonlocal ownership_calls
            ownership_calls += 1
            return lose_listener_at is None or ownership_calls < lose_listener_at

        ready = {"value": False}
        original_calls = [0, 0]
        drained = []

        def read_records(path, _nonce, project_hash, *, enforce_gap_bound):
            name = Path(path).name
            if name.startswith("original-"):
                index = 0 if name.endswith("a.jsonl") else 1
                original_calls[index] += 1
                target = "3" * 64 if index == 0 else "4" * 64
                first = self.row(seq=1, session_hash=target, project_hash=project_hash)
                if original_rows == "noadvance":
                    return [first], 0
                if original_calls[index] == 1:
                    return [first], 0
                if original_rows == "drift":
                    second = self.row(seq=2, session_hash="9" * 64, project_hash=project_hash)
                else:
                    second = self.row(seq=2, session_hash=target, project_hash=project_hash)
                return [first, second], 0
            if not ready["value"]:
                return ([self.row(route="other", session_hash=None, project_hash=None,
                                  top_level=None)], extra_rejected)
            rows = [self.row(route="other", session_hash=None, project_hash=None,
                             top_level=None),
                    self.row(seq=2, session_hash=probe.hash_value("private-session"),
                             project_hash=project_hash)]
            return rows, extra_rejected

        def tick(_tuis, _deadline):
            drained.append(len(_tuis))
            ready["value"] = True

        with patch.object(probe.bootstrap, "server_owns_port", side_effect=owns), \
             patch.object(probe.bootstrap, "request", side_effect=request), \
             patch.object(probe.bootstrap, "launcher_argv", return_value=["fixture"]), \
             patch.object(probe.bootstrap, "_kill_group", return_value=True), \
             patch.object(probe, "PrivatePty", return_value=fake), \
             patch.object(probe, "_records_with_policy", side_effect=read_records), \
             patch.object(probe, "_tick", side_effect=tick):
            result = probe.after_routes(context)
        return result, fake, request_calls, drained

    def test_mocked_full_success_drains_all_clients_checks_scope_and_cleans_up(self):
        result, fake, calls, drained = self._run_probe()
        self.assertEqual(result["classification"], "proven")
        self.assertTrue(result["second_tui_route_observed"])
        self.assertTrue(result["cross_project_rejected"])
        self.assertTrue(result["first_project_observers_fresh"])
        self.assertTrue(result["extra_session_empty"])
        self.assertTrue(result["extra_tui_cleanup_completed"])
        self.assertTrue(fake.closed)
        self.assertTrue(drained and all(count == 3 for count in drained))
        self.assertTrue(any(method == "GET" and "/message?" in path for method, path in calls))

    def test_original_observer_must_advance_and_preserve_identity(self):
        for case in ("noadvance", "drift"):
            result, _fake, _calls, _drained = self._run_probe(original_rows=case)
            self.assertNotEqual(result["classification"], "proven")
            self.assertFalse(result["first_project_observers_fresh"])

    def test_rejected_extra_records_fail_even_when_valid_identity_is_present(self):
        result, _fake, _calls, _drained = self._run_probe(extra_rejected=1)
        self.assertNotEqual(result["classification"], "proven")
        self.assertFalse(result["second_tui_route_observed"])
        self.assertEqual(result["rejected_record_count"], 1)

    def test_listener_loss_blocks_later_authenticated_calls(self):
        result, _fake, calls, _drained = self._run_probe(lose_listener_at=3)
        self.assertNotEqual(result["classification"], "proven")
        self.assertEqual([method for method, _path in calls], ["POST"])

    def test_post_cleanup_empty_read_failure_downgrades_success(self):
        result, _fake, _calls, _drained = self._run_probe(late_empty_failure=True)
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["reason"], "extra_session_empty_verification_failed")
        self.assertFalse(result["extra_session_empty"])


if __name__ == "__main__":
    unittest.main()
