import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import probe_opencode_v2_remaining_identity as probe


class OpenCodeV2RemainingIdentityTests(unittest.TestCase):
    def test_message_digest_requires_nonempty_bounded_canonical_transcript(self):
        messages = [{"id": "msg_a", "role": "assistant", "text": "fixture"}]
        expected = hashlib.sha256(json.dumps(
            messages, sort_keys=True, separators=(",", ":"), ensure_ascii=True
        ).encode()).hexdigest()
        self.assertEqual(probe._messages_digest(messages), expected)
        self.assertEqual(probe._messages_digest([]), None)
        self.assertEqual(probe._messages_digest("not a message list"), None)

    def test_fork_history_digest_ignores_only_remapped_message_ids(self):
        parent = [{"id": "msg_parent", "type": "synthetic",
                   "text": "fixture", "time": {"created": 10}}]
        child = [{**parent[0], "id": "msg_child"}]
        self.assertEqual(probe._fork_history_digest(parent),
                         probe._fork_history_digest(child))
        self.assertNotEqual(probe._fork_history_digest(parent),
                            probe._fork_history_digest([{**child[0], "text": "changed"}]))
        self.assertIsNone(probe._fork_history_digest([]))

    def test_empty_user_message_check_is_single_id_bounded_and_fail_closed(self):
        context = {}
        with patch.object(probe, "_request", return_value=(200, b'{"data":[]}')):
            self.assertEqual(probe._empty_user_messages(context, "root_A"), (True, 200))
        with patch.object(probe, "_request", return_value=(200, b'{"data":[{}]}')):
            self.assertEqual(probe._empty_user_messages(context, "root_A"), (False, 200))
        with patch.object(probe, "_request", return_value=(404, b"")):
            self.assertEqual(probe._empty_user_messages(context, "root_A"), (False, 404))
        self.assertEqual(probe._empty_user_messages(context, "bad/id"), (False, None))

    def test_session_row_scope_requires_exact_id_project_and_parent_shape(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            root = {"id": "root_A", "parentID": None,
                    "location": {"directory": str(project)}}
            child = {"id": "child_A", "parentID": "root_A",
                     "location": {"directory": str(project)}}
            self.assertTrue(probe._session_row_matches(
                root, "root_A", project, top_level=True))
            self.assertFalse(probe._session_row_matches(
                child, "child_A", project, top_level=True))
            self.assertTrue(probe._session_row_matches(
                child, "child_A", project, top_level=False))
            self.assertFalse(probe._session_row_matches(
                {**child, "location": {"directory": str(project / "elsewhere")}},
                "child_A", project, top_level=False))

    def test_root_listing_requires_expected_roots_and_native_fork_entry(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            rows = [
                {"id": "root_A", "parentID": None,
                 "location": {"directory": str(project)}},
                {"id": "root_B", "parentID": None,
                 "location": {"directory": str(project)}},
            ]
            context = {}
            with patch.object(probe, "_list_sessions", return_value=rows):
                self.assertFalse(probe._root_listing_contains_fork(
                    context, project, "child_A", {"root_A", "root_B"}))
                self.assertFalse(probe._root_listing_contains_fork(
                    context, project, "child_A", {"root_A", "missing"}))
            fork_row = {
                    "id": "child_A", "parentID": None,
                    "location": {"directory": str(project)}}
            with patch.object(probe, "_list_sessions", return_value=rows + [fork_row]):
                self.assertTrue(probe._root_listing_contains_fork(
                    context, project, "child_A", {"root_A", "root_B"}))

    def test_instance_nonce_is_returned_only_for_parser_accepted_records(self):
        nonce = "a" * 32
        project_hash = "f" * 64
        instance = "c" * 32
        record = {
            "schema": 1, "kind": "poll", "client_nonce": nonce,
            "tui_instance": instance, "seq": 1, "monotonic_ms": 100,
            "route_name": "session", "session_id_hash": "b" * 64,
            "session_top_level": True, "project_hash": project_hash,
        }
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "records.jsonl"
            path.write_text(json.dumps(record) + "\n", encoding="utf-8")
            self.assertEqual(probe._instance_ids(path, nonce, {project_hash}), [instance])
            path.write_text(json.dumps({**record, "client_nonce": "d" * 32}) + "\n",
                            encoding="utf-8")
            self.assertEqual(probe._instance_ids(path, nonce, {project_hash}), [])

    def test_transition_stream_preserves_sequence_and_one_client_instance(self):
        nonce = "a" * 32
        instance = "c" * 32
        first_project, second_project = "f" * 64, "e" * 64
        rows = []
        for seq, project_hash, session_hash in (
                (1, first_project, "b" * 64), (2, second_project, "d" * 64)):
            rows.append({
                "schema": 1, "kind": "poll", "client_nonce": nonce,
                "tui_instance": instance, "seq": seq, "monotonic_ms": seq * 100,
                "route_name": "session", "session_id_hash": session_hash,
                "session_top_level": True, "project_hash": project_hash,
            })
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "records.jsonl"
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n",
                            encoding="utf-8")
            records, instances, rejected = probe._read_transition_records(
                path, nonce, {first_project, second_project})
            self.assertEqual(rejected, 0)
            self.assertEqual([record["seq"] for record in records], [1, 2])
            self.assertEqual(instances, [instance, instance])
            self.assertEqual(probe._instance_ids(
                path, nonce, {first_project, second_project}), [instance, instance])
            rows[1]["monotonic_ms"] = 1000
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n",
                            encoding="utf-8")
            self.assertEqual(probe._read_transition_records(
                path, nonce, {first_project, second_project})[2], 0)
            rows[1]["seq"] = 4
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n",
                            encoding="utf-8")
            self.assertEqual(probe._read_transition_records(
                path, nonce, {first_project, second_project})[2], 1)

    def test_transition_stream_allows_unidentified_startup_prefix_only(self):
        nonce = "a" * 32
        instance = "c" * 32
        project_hash = "f" * 64
        rows = [
            {"schema": 1, "kind": "plugin_loaded", "client_nonce": nonce,
             "tui_instance": instance, "seq": 1, "monotonic_ms": 100,
             "route_name": "plugin", "session_id_hash": None,
             "session_top_level": None, "project_hash": None},
            {"schema": 1, "kind": "poll", "client_nonce": nonce,
             "tui_instance": instance, "seq": 2, "monotonic_ms": 200,
             "route_name": "session", "session_id_hash": "b" * 64,
             "session_top_level": True, "project_hash": project_hash},
        ]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "records.jsonl"
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n",
                            encoding="utf-8")
            records, instances, rejected = probe._read_transition_records(
                path, nonce, {project_hash})
            self.assertEqual(rejected, 0)
            self.assertEqual([record["seq"] for record in records], [1, 2])
            self.assertEqual(instances, [instance, instance])
            rows.append({"schema": 1, "kind": "poll", "client_nonce": nonce,
                         "tui_instance": instance, "seq": 3, "monotonic_ms": 300,
                         "route_name": "session", "session_id_hash": "b" * 64,
                         "session_top_level": True, "project_hash": None})
            path.write_text("\n".join(json.dumps(row) for row in rows) + "\n",
                            encoding="utf-8")
            self.assertEqual(probe._read_transition_records(
                path, nonce, {project_hash})[2], 1)

    def test_worker_requires_both_baseline_and_remaining_acceptance_plus_cleanup(self):
        report = {"cleanup_completed": True, "tui": {
            "classification": "proven", "tui_cleanup_completed": True,
            "remaining_identity": {"classification": "proven"},
        }}
        with patch.object(probe.bootstrap, "_run_worker", return_value=report):
            code, result = probe._worker("/private/pinned", "native")
        self.assertEqual(code, 0)
        self.assertIs(result, report)

        report["tui"]["tui_cleanup_completed"] = False
        with patch.object(probe.bootstrap, "_run_worker", return_value=report):
            code, _result = probe._worker("/private/pinned", "native")
        self.assertEqual(code, 2)

    def test_runner_fails_closed_when_launcher_is_unavailable(self):
        code, report = probe.run(which=lambda _name: None, home=Path("/nonexistent"))
        self.assertEqual(code, 2)
        self.assertEqual(report["reason"], "launcher_not_found")


if __name__ == "__main__":
    unittest.main()
