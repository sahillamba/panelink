import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts import p4_managed_transition_fixtures as fixtures


def response(data):
    return 200, json.dumps({"data": data}).encode("utf-8")


class ManagedTransitionFixtureTests(unittest.TestCase):
    def test_root_creator_requires_exact_id_title_project_and_root_shape(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            created = {"id": "new_root_1", "parentID": None}
            detail = {"id": "new_root_1", "parentID": None,
                      "title": fixtures.NEW_ROOT_TITLE,
                      "location": {"directory": str(project)}}
            with patch.object(fixtures.identity, "_request",
                              side_effect=[response(created), response(detail)]), \
                    patch.object(fixtures.identity, "_lookup", return_value=True):
                self.assertEqual(fixtures._create_root(
                    {}, fixtures.NEW_ROOT_TITLE, project), "new_root_1")

            foreign_detail = {**detail, "location": {"directory": str(project / "elsewhere")}}
            with patch.object(fixtures.identity, "_request",
                              side_effect=[response(created), response(foreign_detail)]), \
                    patch.object(fixtures.identity, "_lookup", return_value=True):
                self.assertIsNone(fixtures._create_root(
                    {}, fixtures.NEW_ROOT_TITLE, project))

    def test_fixture_builder_reuses_verified_fork_and_outputs_only_hashes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            project.mkdir(mode=0o700)
            ids = iter(("new_root_private", "foreign_root_private"))
            fork_checks = {
                "root_choice_info_verified": True,
                "fork_parent_link_verified": True,
                "fork_child_is_top_level": True,
                "fork_history_hash_verified": True,
                "fork_import_roundtrip_exact": True,
            }
            fork_info = {"id": "fork_root_private", "parentID": None,
                         "title": fixtures.FORK_ROOT_TITLE,
                         "location": {"directory": str(project)}}
            with patch.object(fixtures, "_create_root", side_effect=lambda *_args: next(ids)), \
                    patch.object(fixtures.recovery, "_forked_root_choice",
                                 return_value=("fork_root_private", "fork_parent_private")) as fork_builder, \
                    patch.object(fixtures.identity, "_request",
                                 side_effect=[(204, b""), response(fork_info)]), \
                    patch.object(fixtures.identity, "_lookup", return_value=True), \
                    patch.object(fixtures.identity, "_export",
                                 return_value={"messages": [{"type": "synthetic"}]}), \
                    patch.object(fixtures.identity, "_empty_user_messages",
                                 return_value=(True, 200)), \
                    patch.object(fixtures.identity, "_fork_history_digest",
                                 return_value="a" * 64):
                fork_builder.side_effect = lambda _context, _project, checks: (
                    checks.update(fork_checks) or
                    ("fork_root_private", "fork_parent_private"))
                result, checks = fixtures.create_managed_transition_fixtures(
                    {"root": root}, project)

            self.assertIsNotNone(result)
            self.assertEqual(result.new_root_id, "new_root_private")
            self.assertEqual(result.foreign_root_id, "foreign_root_private")
            evidence = fixtures.safe_evidence(result, checks)
            self.assertTrue(evidence["fixture_set_ready"])
            self.assertEqual(evidence["fork_history_sha256"], "a" * 64)
            serialized = json.dumps(evidence)
            for private_value in (
                    "new_root_private", "fork_root_private", "fork_parent_private",
                    "foreign_root_private", str(root)):
                self.assertNotIn(private_value, serialized)
            self.assertEqual(result.new_root_sha256,
                             hashlib.sha256(b"new_root_private").hexdigest())
            self.assertNotEqual(result.project_sha256, result.foreign_project_sha256)
            rendered = repr(result)
            self.assertEqual(rendered, "ManagedTransitionFixtures(<redacted>)")
            for private_value in (
                    "new_root_private", "fork_root_private", "fork_parent_private",
                    "foreign_root_private", str(root)):
                self.assertNotIn(private_value, rendered)

    def test_foreign_project_refuses_outside_parent_before_creating_path(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "owned"
            outside = Path(temporary) / "outside"
            root.mkdir(mode=0o700)
            outside.mkdir(mode=0o700)
            requested = outside / "must-not-be-created"
            self.assertIsNone(fixtures._foreign_project(
                {"root": root}, requested))
            self.assertFalse(requested.exists())

    def test_builder_refuses_project_outside_fixture_root_before_api_calls(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            root = base / "owned"
            outside = base / "outside-project"
            root.mkdir(mode=0o700)
            outside.mkdir(mode=0o700)
            with patch.object(fixtures, "_create_root") as create_root:
                result, checks = fixtures.create_managed_transition_fixtures(
                    {"root": root}, outside)
            self.assertIsNone(result)
            self.assertFalse(checks["project_verified"])
            create_root.assert_not_called()

    def test_builder_fails_closed_when_fork_history_is_not_verified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            project = root / "project"
            project.mkdir(mode=0o700)
            with patch.object(fixtures, "_create_root", return_value="new_root_private"), \
                    patch.object(fixtures.recovery, "_forked_root_choice",
                                 side_effect=lambda _c, _p, checks: (
                                     checks.update({"root_choice_info_verified": False})
                                     or (None, None))):
                result, checks = fixtures.create_managed_transition_fixtures(
                    {"root": root}, project)
            self.assertIsNone(result)
            self.assertFalse(checks["fork_parent_and_nonempty_history_verified"])
            self.assertFalse(fixtures.safe_evidence(result, checks)["fixture_set_ready"])


if __name__ == "__main__":
    unittest.main()
