import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from scripts import probe_opencode


class ProbeOpenCodeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def executable(self, body="print('OpenCode 1.2.3')\n", executable=True):
        path = self.root / "fake-opencode"
        path.write_text(f"#!{sys.executable}\n" + body, encoding="utf-8")
        path.chmod(0o755 if executable else 0o644)
        return str(path)

    def test_missing_executable_is_blocked(self):
        result = probe_opencode.probe(which=lambda _name: None)
        self.assertEqual((result["status"], result["reason"]), ("blocked", "missing_executable"))
        self.assertIsNone(result["version"])
        self.assertFalse(result["live_identity_tested"])

    def test_valid_local_executable_reports_version_only(self):
        result = probe_opencode.probe(self.executable())
        self.assertEqual((result["status"], result["version"]), ("available", "1.2.3"))
        self.assertEqual(result["discovery"], "explicit")
        self.assertFalse(result["live_identity_tested"])

    def test_invalid_non_executable_is_blocked(self):
        result = probe_opencode.probe(self.executable(executable=False))
        self.assertEqual(result["reason"], "invalid_executable")

    def test_malformed_version_is_blocked(self):
        result = probe_opencode.probe(self.executable("print('development build')\n"))
        self.assertEqual((result["status"], result["reason"]), ("blocked", "unrecognized_version"))

    def test_nonzero_exit_is_blocked_without_exposing_output(self):
        def failed(*_args, **_kwargs):
            return probe_opencode.CaptureResult(7, b"api_key=hunter2", b"failed")

        result = probe_opencode.probe(self.executable(), runner=failed)
        self.assertEqual((result["status"], result["reason"]), ("blocked", "execution_failed"))
        self.assertNotIn("hunter2", json.dumps(result))
        self.assertEqual(result["diagnostic"], "blocked:execution_failed")

    def test_version_result_never_includes_other_command_output(self):
        noisy = self.executable("print('OpenCode 1.2.3')\nprint('api_key=hunter2')\n")
        result = probe_opencode.probe(noisy)
        self.assertEqual(result["version"], "1.2.3")
        self.assertEqual(result["diagnostic"], "version_only")
        self.assertNotIn("hunter2", json.dumps(result))

    def test_child_environment_is_isolated_from_user_configuration(self):
        captured = {}

        def inspect(*_args, **kwargs):
            captured.update(kwargs)
            child_env = kwargs["env"]
            captured["temporary_state_isolated"] = (
                Path(child_env["HOME"]).is_dir()
                and kwargs["cwd"] == child_env["HOME"]
                and all(
                    Path(child_env[key]).is_dir()
                    for key in ("XDG_CONFIG_HOME", "XDG_CACHE_HOME", "XDG_DATA_HOME")
                )
            )
            return probe_opencode.CaptureResult(0, b"v1.2.3", b"")

        with patch.dict(os.environ, {"OPENAI_API_KEY": "private-value", "PANELINK_TEST_SENTINEL": "keep"}):
            before = dict(os.environ)
            result = probe_opencode.probe(self.executable(), runner=inspect)
            self.assertEqual(os.environ, before)

        child_env = captured["env"]
        self.assertNotIn("OPENAI_API_KEY", child_env)
        self.assertNotIn("PANELINK_TEST_SENTINEL", child_env)
        self.assertEqual(child_env["OPENCODE_DISABLE_MODELS_FETCH"], "1")
        self.assertEqual(child_env["PATH"], os.defpath)
        self.assertTrue(captured["temporary_state_isolated"])
        self.assertEqual(captured["shell"], False)
        self.assertEqual(captured["stdin"], subprocess.DEVNULL)
        self.assertEqual(result["version"], "1.2.3")

    def test_real_noisy_executable_hits_total_capture_cap(self):
        path = self.executable("import os\nwhile True:\n    os.write(1, b'x' * 8192)\n")
        result = probe_opencode.probe(path, timeout=2.0)
        self.assertEqual((result["status"], result["reason"]), ("blocked", "output_limit"))
        self.assertEqual(result["diagnostic"], "blocked:output_limit")
        self.assertNotIn("x" * 20, json.dumps(result))

    def test_real_timeout_kills_descendant_holding_output_pipe(self):
        child_pid_file = self.root / "descendant.pid"
        body = (
            "import os, time\n"
            "pid = os.fork()\n"
            "if pid == 0:\n"
            "    time.sleep(30)\n"
            "    os._exit(0)\n"
            f"open({str(child_pid_file)!r}, 'w').write(str(pid))\n"
            "os._exit(0)\n"
        )
        result_holder = {}
        executable = self.executable(body)
        worker = threading.Thread(
            target=lambda: result_holder.setdefault(
                "result", probe_opencode.probe(executable, timeout=2.5)
            ),
            daemon=True,
        )
        started = time.monotonic()
        worker.start()
        ready_deadline = started + 2.0
        while not child_pid_file.exists() and time.monotonic() < ready_deadline:
            time.sleep(0.01)
        worker.join(timeout=4.0)
        elapsed = time.monotonic() - started
        self.assertFalse(worker.is_alive(), "bounded probe must return after killing its process group")
        self.assertTrue(child_pid_file.exists(), "the fake executable must have created its descendant")
        result = result_holder["result"]
        self.assertEqual((result["status"], result["reason"]), ("blocked", "timed_out"))
        self.assertLess(elapsed, 4.0, "descendant-held pipes must not extend the deadline")


if __name__ == "__main__":
    unittest.main()
