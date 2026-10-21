"""Pure identity classification and bounded-runner tests; no OpenCode process."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from scripts import probe_opencode_identity as probe
from scripts.probe_opencode import CaptureResult


class IdentityProbeTests(unittest.TestCase):
    def test_version_parsing_rejects_noise(self):
        self.assertEqual(probe.parse_version(b"1.19.0\n"), "1.19.0")
        self.assertIsNone(probe.parse_version(b"API_KEY=private\n"))

    def test_session_parser_rejects_child_and_malformed_ids(self):
        self.assertEqual(probe.session_id(b'{"id":"ses_abc-1"}'), "ses_abc-1")
        self.assertIsNone(probe.session_id(b'{"id":"ses_abc","parentID":"ses_parent"}'))
        self.assertIsNone(probe.session_id(b'{"id":"bad/identifier"}'))
        self.assertIsNone(probe.session_id(b'{"id":123}'))
        self.assertIsNone(probe.session_id(b'not json'))

    def test_creation_does_not_prove_selection(self):
        self.assertEqual(probe.classify("one", "two", None),
                         "two_sessions_created_selection_not_proven")
        self.assertEqual(probe.classify("one", "two", "other"),
                         "two_sessions_created_selection_unattributable")
        self.assertEqual(probe.classify("one", "two", "two"),
                         "selected_snapshot_only_switch_not_proven")
        self.assertEqual(probe.classify("one", "one", "one"), "two_sessions_not_proven")

    def test_environment_is_private_and_has_no_inherited_auth(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = probe.environment(root)
            self.assertEqual(env["HOME"], directory)
            self.assertEqual(env["PATH"], os.defpath)
            self.assertTrue(all(Path(env[key]).is_dir() for key in (
                "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME")))
            self.assertNotIn("OPENAI_API_KEY", env)

    def test_runner_caps_output_and_kills_on_timeout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            env = probe.environment(root)
            noisy = probe.bounded_command([sys.executable, "-c",
                                           "import os; os.write(1,b'x'*20000)"], root, env)
            self.assertEqual(noisy.reason, "output_limit")
            self.assertLessEqual(len(noisy.stdout) + len(noisy.stderr), 4096)
            original = probe.TIMEOUT
            try:
                probe.TIMEOUT = 0.2
                slow = probe.bounded_command([sys.executable, "-c",
                                              "import time; time.sleep(5)"], root, env)
            finally:
                probe.TIMEOUT = original
            self.assertEqual(slow.reason, "timed_out")

    def test_missing_executable_does_not_start_process(self):
        with patch.object(probe, "resolve_executable", return_value=(None, None)), \
             patch.object(probe, "bounded_command", side_effect=AssertionError("no process")):
            result = probe.run("/absent/panelink/identity/fixture")
        self.assertEqual(result["version_status"], "missing_executable")
        self.assertIsNone(result["launcher"])
        self.assertEqual(result["switch"], "not_proven")

    def test_resolution_order_explicit_then_path_then_home_then_wrapper(self):
        root = Path("/isolated-test-home")
        explicit = str(root / "explicit" / "opencode")
        path_cli = str(root / "path" / "opencode")
        installed = str(root / ".opencode" / "bin" / "opencode")
        wrapper = str(root / "path" / "tescode")
        paths = {explicit, path_cli, installed, wrapper}
        seen = []

        def which(name):
            seen.append(name)
            return {"opencode": path_cli, "tescode": wrapper}.get(name)

        def usable(path):
            return str(path) in paths

        options = {"which": which, "home": root, "executable": usable}
        self.assertEqual(probe.resolve_executable(explicit, **options), (explicit, "opencode"))
        self.assertEqual(seen, [])
        self.assertEqual(probe.resolve_executable(**options), (path_cli, "opencode"))
        self.assertEqual(seen, ["opencode"])
        paths.remove(path_cli)
        seen.clear()
        self.assertEqual(probe.resolve_executable(**options), (installed, "opencode"))
        self.assertEqual(seen, ["opencode"])
        paths.remove(installed)
        seen.clear()
        self.assertEqual(probe.resolve_executable(**options), (wrapper, "tescode"))
        self.assertEqual(seen, ["opencode", "tescode"])
        paths.remove(wrapper)
        self.assertEqual(probe.resolve_executable(**options), (None, None))
        self.assertEqual(probe.resolve_executable(explicit + "-missing", **options), (None, None))

    def test_explicit_wrapper_is_identified_without_executing(self):
        self.assertEqual(probe.resolve_executable("/fixture/tescode", which=lambda _: None,
                         executable=lambda _: True), ("/fixture/tescode", "tescode"))

    def test_tescode_fallback_uses_only_safe_commands_and_blocks_identity_steps(self):
        invocations = []

        def runner(argv, *_args, **_kwargs):
            invocations.append(argv[1:])
            return CaptureResult(0, b"tescode 1.2.3\n", b"")

        with patch.object(probe, "resolve_executable", return_value=("/fixture/tescode", "tescode")), \
             patch.object(probe, "bounded_command", side_effect=runner), \
             patch.object(probe.subprocess, "Popen", side_effect=AssertionError("no server")):
            result = probe.run()
        self.assertEqual(invocations, [["version"], ["sessions", "--help"]])
        self.assertEqual(result["launcher"], "tescode")
        self.assertEqual(result["version_status"], "wrapper_version_command_passed")
        self.assertEqual(result["wrapper_sessions_help"], "exit_0")
        for key in ("server", "serve_help", "session_list", "cli_session_list",
                    "cli_export", "event_sse", "selected_route", "two_sessions"):
            self.assertEqual(result[key], "blocked_wrapper_only")
        self.assertEqual(result["switch"], "not_proven")
        self.assertIsNone(result["version"])


if __name__ == "__main__":
    unittest.main()
