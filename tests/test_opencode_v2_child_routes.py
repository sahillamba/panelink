import hashlib
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import probe_opencode_v2_child_routes as probe


class OpenCodeV2ChildRoutesTests(unittest.TestCase):
    def test_child_metadata_requires_exact_parent_and_project(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            project_id = "parent_A"
            base = {"id": "child_A", "parentID": project_id,
                    "location": {"directory": str(project)}}
            self.assertEqual(probe._child_metadata_matches(
                base, parent_id=project_id, project=project), "child_A")
            self.assertIsNone(probe._child_metadata_matches(
                {**base, "parentID": "parent_B"}, parent_id=project_id, project=project))
            self.assertIsNone(probe._child_metadata_matches(
                {**base, "location": {"directory": str(project / "elsewhere")}},
                parent_id=project_id, project=project))
            self.assertIsNone(probe._child_metadata_matches(
                {**base, "id": "bad/id"}, parent_id=project_id, project=project))
            self.assertIsNone(probe._child_metadata_matches(
                {"id": "child_A", "parentID": project_id,
                 "directory": str(project)}, parent_id=project_id, project=project))

    def test_top_level_listing_fails_closed_on_child_promotion_unknowns_and_scope(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            expected = {"parent_A", "parent_B"}

            def payload(rows):
                return json.dumps({"data": rows}).encode()

            parents = [
                {"id": item, "parentID": None,
                 "location": {"directory": str(project)}}
                for item in sorted(expected)
            ]
            self.assertTrue(probe._top_level_excludes_child(
                payload(parents), child_id="child_A", project=project,
                expected_parents=expected))
            promoted = parents + [{"id": "child_A", "parentID": None,
                                   "location": {"directory": str(project)}}]
            self.assertFalse(probe._top_level_excludes_child(
                payload(promoted), child_id="child_A", project=project,
                expected_parents=expected))
            unknown = parents + [{"id": "unknown", "parentID": None,
                                  "location": {"directory": str(project)}}]
            self.assertFalse(probe._top_level_excludes_child(
                payload(unknown), child_id="child_A", project=project,
                expected_parents=expected))

    def test_route_history_rejects_top_level_child_unknown_and_missing_return(self):
        def row(identifier, top_level):
            return {"route_name": "session", "session_id_hash": identifier,
                    "session_top_level": top_level, "project_hash": "f" * 64}

        child, parent, unknown = "c" * 64, "a" * 64, "b" * 64
        valid_child = [row(child, False)]
        self.assertTrue(probe._child_history_matches(
            valid_child, child_hash=child, parent_hash=parent, project_hash="f" * 64))
        self.assertFalse(probe._child_history_matches(
            [row(child, True)], child_hash=child, parent_hash=parent,
            project_hash="f" * 64))
        self.assertFalse(probe._child_history_matches(
            [row(unknown, False)], child_hash=child, parent_hash=parent,
            project_hash="f" * 64))
        self.assertFalse(probe._child_history_matches(
            valid_child, child_hash=child, parent_hash=parent, project_hash="f" * 64,
            require_return=True))
        self.assertFalse(probe._child_history_matches(
            valid_child + [row(unknown, False)], child_hash=child, parent_hash=parent,
            project_hash="f" * 64))
        self.assertFalse(probe._child_history_matches(
            valid_child + [{**row(child, False), "route_name": "home"}, row(parent, True)],
            child_hash=child, parent_hash=parent, project_hash="f" * 64,
            require_return=True))
        self.assertFalse(probe._child_history_matches(
            [row(unknown, False), *valid_child], child_hash=child, parent_hash=parent,
            project_hash="f" * 64))
        self.assertTrue(probe._child_history_matches(
            valid_child + [row(parent, True)], child_hash=child, parent_hash=parent,
            project_hash="f" * 64, require_return=True))

    def test_child_startup_allows_unresolved_cache_but_rejects_foreign_identity(self):
        child, parent, project = "c" * 64, "a" * 64, "f" * 64
        warmup = {"route_name": "session", "session_id_hash": child,
                  "session_top_level": None, "project_hash": None}
        selected = {**warmup, "session_top_level": False, "project_hash": project}
        self.assertTrue(probe._child_history_matches(
            [warmup, selected], child_hash=child, parent_hash=parent, project_hash=project))
        for foreign in ({**warmup, "session_id_hash": parent},
                        {**warmup, "project_hash": "b" * 64},
                        {**warmup, "session_top_level": True}):
            self.assertFalse(probe._child_history_matches(
                [foreign, selected], child_hash=child, parent_hash=parent, project_hash=project))

    def test_hook_scope_failure_is_fail_closed_and_redacted(self):
        result = probe.after_routes({})
        self.assertEqual(result["classification"], "not_proven")
        self.assertEqual(result["reason"], "fixture_scope_unavailable")
        self.assertNotIn("child_A", json.dumps(result))
        self.assertNotIn("/private/", json.dumps(result))

    def test_owned_request_skips_auth_when_exact_listener_is_lost(self):
        context = {"port": 8765, "server_pid": 123, "base": "http://127.0.0.1:8765",
                   "password": "secret"}
        with patch.object(probe.bootstrap, "server_owns_port", return_value=False), \
                patch.object(probe.bootstrap, "request") as request:
            self.assertEqual(probe._owned_request(context, "GET", "/api/session"),
                             (None, b""))
        request.assert_not_called()

    def test_cleanup_and_final_empty_read_failures_downgrade_acceptance(self):
        proven = {"classification": "proven", "reason": "ok",
                  "user_messages_empty": True, "passive_identity_unchanged": True}
        with patch.object(probe, "_cleanup_tui", return_value=False):
            probe._finish_result(proven, object(), {}, ("parent_A", "child_A"),
                                 [1, 1], ["a" * 64, "b" * 64], "f" * 64)
        self.assertEqual(proven["classification"], "blocked")
        self.assertEqual(proven["reason"], "child_tui_cleanup_unverified")

        proven = {"classification": "proven", "reason": "ok",
                  "user_messages_empty": True, "passive_identity_unchanged": True}
        with patch.object(probe, "_cleanup_tui", return_value=True), \
                patch.object(probe, "_empty_messages", return_value=False), \
                patch.object(probe, "_originals_advanced", return_value=True):
            probe._finish_result(proven, object(), {}, ("parent_A", "child_A"),
                                 [1, 1], ["a" * 64, "b" * 64], "f" * 64)
        self.assertEqual(proven["classification"], "not_proven")
        self.assertEqual(proven["reason"], "post_cleanup_acceptance_not_proven")
        self.assertFalse(proven["user_messages_empty"])

    def test_after_routes_runs_bounded_owned_linked_child_lifecycle(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            physical_project = root / "physical-project"
            physical_project.mkdir(mode=0o700)
            project = root / "project-alias"
            project.symlink_to(physical_project, target_is_directory=True)
            config_dir = root / "opencode-config"
            config_dir.mkdir(mode=0o700)
            plugin_dir = root / "identity-plugin"
            plugin_dir.mkdir(mode=0o700)
            fixture_plugin = (Path(probe.__file__).resolve().parent.parent / "docs" /
                              "progress" / "p4-v2-tui-plugin-fixture" / "plugin.mjs")
            if not fixture_plugin.exists():
                self.skipTest("the pinned TUI fixture source is unavailable")

            parent_ids = ("parent_A", "parent_B")
            parent_hashes = [hashlib.sha256(item.encode()).hexdigest()
                             for item in parent_ids]
            project_hash = hashlib.sha256(str(project.resolve()).encode()).hexdigest()
            original_paths = [root / "tui-a.jsonl", root / "tui-b.jsonl"]
            nonces = ["a" * 32, "b" * 32]

            def record(seq, nonce, session_hash, *, top_level=True, route="session"):
                return {
                    "schema": 1, "kind": "poll", "client_nonce": nonce,
                    "tui_instance": ("d" if nonce == "a" * 32 else "e") * 32,
                    "seq": seq, "monotonic_ms": seq * 100.0,
                    "route_name": route, "session_id_hash": session_hash,
                    "session_top_level": top_level,
                    "project_hash": project_hash,
                }

            def write_rows(path, rows):
                path.write_bytes(b"\n".join(json.dumps(item).encode() for item in rows))

            for index, path in enumerate(original_paths):
                write_rows(path, [record(1, nonces[index], parent_hashes[index])])

            child_id = "child_A"
            child_hash = hashlib.sha256(child_id.encode()).hexdigest()
            child_tui_path = root / "child-route.jsonl"
            original_tuis = [type("OriginalTui", (), {"alive": lambda self: True})()
                             for _ in range(2)]

            class FakePty:
                def __init__(self, argv, cwd, env):
                    self.master = -1
                    self.output = bytearray()
                    self.child = object()
                    self.selector = None
                    path = Path(env["PANELINK_PROBE_RECORDS"])
                    nonce = env["PANELINK_PROBE_CLIENT_NONCE"]
                    write_rows(path, [
                        record(1, nonce, None, top_level=None, route="plugin"),
                        record(2, nonce, child_hash, top_level=False),
                    ])

                def alive(self):
                    return True

                def screen_text(self):
                    return "Sessions Panelink V2 Identity A"

                def close(self):
                    pass

            def fake_send(tui, payload, deadline):
                if payload == b"\r":
                    path = root / "child-route.jsonl"
                    rows = [json.loads(line) for line in path.read_bytes().splitlines()]
                    child_nonce = rows[0]["client_nonce"]
                    rows.append(record(3, child_nonce, parent_hashes[0]))
                    path.write_bytes(b"\n".join(json.dumps(item).encode() for item in rows))
                    for index, old in enumerate(original_paths):
                        write_rows(old, [record(1, nonces[index], parent_hashes[index]),
                                         record(2, nonces[index], parent_hashes[index])])
                    child_rows, child_rejected = probe._read_records(path, child_nonce,
                                                                     project_hash)
                    self.assertEqual(child_rejected, 0, child_rows)
                    self.assertTrue(probe._child_history_matches(
                        child_rows, child_hash=child_hash, parent_hash=parent_hashes[0],
                        project_hash=project_hash, require_return=True), child_rows)
                    self.assertTrue(probe._originals_advanced(
                        context, [1, 1], parent_hashes, project_hash))
                return True

            calls = []

            def fake_request(base, method, path, *, password, body=None, timeout=2.0):
                calls.append((method, path, body))
                if path.endswith("/fork"):
                    return 400, b""
                if method == "POST" and path == "/api/session":
                    return 200, json.dumps({"data": {
                        "id": child_id, "parentID": parent_ids[0],
                        "location": {"directory": str(project)},
                        "title": probe.CHILD_TITLE,
                    }}).encode()
                if method == "GET" and path == f"/api/session/{child_id}":
                    return 200, json.dumps({"data": {
                        "id": child_id, "parentID": parent_ids[0],
                        "location": {"directory": str(project)},
                        "title": probe.CHILD_TITLE,
                    }}).encode()
                if method == "GET" and "/message?type=user" in path:
                    return 200, b'{"data":[]}'
                if method == "GET" and path.startswith("/api/session?"):
                    from urllib.parse import parse_qs, urlsplit
                    self.assertEqual(parse_qs(urlsplit(path).query)["directory"], [str(project)])
                    rows = [{"id": identifier, "parentID": None,
                             "location": {"directory": str(project)}}
                            for identifier in parent_ids]
                    return 200, json.dumps({"data": rows}).encode()
                return None, b""

            context = {
                "root": root, "project": project, "base": "http://127.0.0.1:8765",
                "password": "fixture-secret", "port": 8765, "server_pid": os.getpid(),
                "executable": root / "opencode", "launcher": "opencode",
                "deadline": time_monotonic_plus(20.0), "version": probe.PINNED_VERSION,
                "session_ids": parent_ids, "tuis": original_tuis,
                "record_paths": original_paths, "nonces": nonces,
                "control_paths": [root / "a.control", root / "b.control"],
                "project_hash": project_hash, "hashes": parent_hashes,
                "env": {"OPENCODE_PASSWORD": "fixture-secret"},
            }

            with patch.object(probe.bootstrap, "sha256_file", return_value=probe.PINNED_SHA256), \
                    patch.object(probe.bootstrap, "server_owns_port", return_value=True), \
                    patch.object(probe.bootstrap, "request", side_effect=fake_request), \
                    patch.object(probe, "PrivatePty", FakePty), \
                    patch.object(probe.bootstrap, "_kill_group", return_value=True), \
                    patch.object(probe.tui_probe, "_tick", return_value=None), \
                    patch.object(probe.tui_probe, "_wait_until", side_effect=lambda pred, _t, _d: pred()), \
                    patch.object(probe.tui_probe, "_send_control", side_effect=fake_send), \
                    patch.object(probe.tui_probe, "_picker_screen_text", side_effect=lambda text: text):
                result = probe.after_routes(context)

            self.assertEqual(result["classification"], "proven", result)
            self.assertEqual(result["child_creation_path"], "parent_id")
            self.assertEqual(result["fork_outcome"], "empty_parent_rejected_status_400")
            self.assertTrue(result["child_selected"])
            self.assertTrue(result["child_not_top_level"])
            self.assertTrue(result["top_level_list_excludes_child"])
            self.assertTrue(result["passive_identity_unchanged"])
            self.assertTrue(result["original_observers_unchanged"])
            self.assertTrue(result["returned_to_parent"])
            self.assertTrue(result["user_messages_empty"])
            self.assertTrue(result["extra_tui_cleanup_completed"])
            self.assertEqual(result["prompt_submissions"], 0)
            self.assertTrue(all(call[0] != "POST" or call[1].endswith("/fork")
                                or call[1] == "/api/session" for call in calls))
            self.assertNotIn(child_id, json.dumps(result))


def time_monotonic_plus(seconds):
    import time
    return time.monotonic() + seconds


if __name__ == "__main__":
    unittest.main()
