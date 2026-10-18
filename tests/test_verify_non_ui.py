import json
import os
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

from scripts import verify_non_ui


class VerifyNonUITests(unittest.TestCase):
    def setUp(self):
        self.root = Path(__file__).resolve().parent.parent

    def test_runs_fixed_commands_in_order_from_repository_root(self):
        calls = []
        expected_commands = (
            ("cargo_test", ("cargo", "test", "--locked", "--offline")),
            (
                "crossterm_input_decoder_tests",
                (
                    "cargo",
                    "test",
                    "--locked",
                    "--offline",
                    "--manifest-path",
                    "vendor/crossterm-input/Cargo.toml",
                ),
            ),
            ("component_sources", ("python3", "scripts/check_component_sources.py")),
            ("cargo_fmt", ("cargo", "fmt", "--check")),
            (
                "cargo_clippy",
                ("cargo", "clippy", "--locked", "--offline", "--all-targets", "--", "-D", "warnings"),
            ),
            ("cargo_build", ("cargo", "build", "--locked", "--offline")),
            (
                "python_unittest",
                ("python3", "-m", "unittest", "discover", "-s", "tests", "-p", "test_*.py"),
            ),
            ("docs_check", ("python3", "scripts/check_docs.py")),
        )

        def runner(argv, *, cwd, env, timeout):
            calls.append((argv, cwd, env, timeout))
            return "passed"

        with tempfile.TemporaryDirectory() as cargo_home:
            code, summary = verify_non_ui.run_checks(
                repo_root=self.root,
                environ={"CARGO_HOME": cargo_home, "TOKEN": "private-value"},
                process_runner=runner,
            )

        self.assertEqual(code, 0)
        self.assertEqual(verify_non_ui.COMMANDS, expected_commands)
        self.assertEqual([item[0] for item in calls], [command for _, command in expected_commands])
        self.assertTrue(all(item[1] == self.root for item in calls))
        self.assertTrue(all(item[2]["CARGO_HOME"] == calls[0][2]["CARGO_HOME"] for item in calls))
        self.assertTrue(all(item[3] == 180 for item in calls))
        self.assertEqual([item["status"] for item in summary["checks"]], ["passed"] * 8)
        self.assertTrue(summary["component_verification_only"])
        self.assertFalse(summary["milestones_complete"])
        self.assertEqual(summary["host_provider_gates"], "not_certified")

    def test_failures_and_timeouts_do_not_skip_later_checks(self):
        seen = []
        outcomes = iter(("failed", "timed_out", "passed", "passed", "error", "passed", "passed", "passed"))

        def runner(argv, **_kwargs):
            seen.append(argv)
            return next(outcomes)

        code, summary = verify_non_ui.run_checks(repo_root=self.root, environ={}, process_runner=runner)
        self.assertEqual(code, 1)
        self.assertEqual(seen, [command for _, command in verify_non_ui.COMMANDS])
        self.assertEqual(
            [item["status"] for item in summary["checks"]],
            ["failed", "timed_out", "passed", "passed", "error", "passed", "passed", "passed"],
        )

    def test_vendor_decoder_failure_returns_one_and_later_checks_still_run(self):
        seen = []
        outcomes = iter(("passed", "failed", "passed", "passed", "passed", "passed", "passed", "passed"))

        def runner(argv, **_kwargs):
            seen.append(argv)
            return next(outcomes)

        code, summary = verify_non_ui.run_checks(
            repo_root=self.root,
            environ={},
            process_runner=runner,
        )

        self.assertEqual(code, 1)
        self.assertEqual(seen, [command for _, command in verify_non_ui.COMMANDS])
        self.assertEqual(
            summary["checks"][1],
            {"name": "crossterm_input_decoder_tests", "status": "failed"},
        )
        self.assertEqual(summary["checks"][-1], {"name": "docs_check", "status": "passed"})

    def test_component_source_failure_does_not_skip_later_checks(self):
        seen = []
        outcomes = iter(("passed", "passed", "failed", "passed", "passed", "passed", "passed", "passed"))

        def runner(argv, **_kwargs):
            seen.append(argv)
            return next(outcomes)

        code, summary = verify_non_ui.run_checks(repo_root=self.root, environ={}, process_runner=runner)

        self.assertEqual(code, 1)
        self.assertEqual(seen, [command for _, command in verify_non_ui.COMMANDS])
        self.assertEqual(summary["checks"][2], {"name": "component_sources", "status": "failed"})
        self.assertEqual(summary["checks"][-1], {"name": "docs_check", "status": "passed"})

    def test_summary_is_fixed_and_does_not_expose_environment_or_arguments(self):
        secret = "private-token-sentinel"

        def runner(_argv, **_kwargs):
            raise subprocess.TimeoutExpired([secret], 180, output=secret)

        with tempfile.TemporaryDirectory(prefix="private-token-sentinel-") as cargo_home:
            code, summary = verify_non_ui.run_checks(
                repo_root=self.root,
                environ={"CARGO_HOME": cargo_home, "SECRET": secret},
                process_runner=runner,
            )
        rendered = json.dumps(summary)
        self.assertEqual(code, 1)
        self.assertNotIn(secret, rendered)
        self.assertEqual(
            set(summary),
            {"component_verification_only", "milestones_complete", "host_provider_gates", "setup_status", "checks"},
        )
        self.assertTrue(all(item["status"] == "timed_out" for item in summary["checks"]))

    def test_invalid_setup_exits_two_without_running_checks(self):
        called = False

        def runner(*_args, **_kwargs):
            nonlocal called
            called = True
            return "passed"

        code, summary = verify_non_ui.run_checks(
            repo_root=self.root / "missing",
            environ={},
            process_runner=runner,
        )
        self.assertEqual(code, 2)
        self.assertFalse(called)
        self.assertEqual(summary["setup_status"], "invalid")

    def test_invalid_caller_cargo_home_is_setup_error(self):
        code, summary = verify_non_ui.run_checks(
            repo_root=self.root,
            environ={"CARGO_HOME": "/path/that/does/not/exist"},
            process_runner=lambda *_args, **_kwargs: "passed",
        )
        self.assertEqual(code, 2)
        self.assertEqual(summary["setup_status"], "invalid")

    def test_relative_cargo_home_is_resolved_before_changing_child_directory(self):
        previous = Path.cwd()
        with tempfile.TemporaryDirectory() as caller_dir:
            cache = Path(caller_dir) / "cache"
            cache.mkdir()
            try:
                os.chdir(caller_dir)
                selected = verify_non_ui._cargo_home({"CARGO_HOME": "cache"})
                self.assertEqual(selected, str(cache.resolve()))
                with self.assertRaises(ValueError):
                    verify_non_ui._cargo_home({"CARGO_HOME": ""})
            finally:
                os.chdir(previous)

    def test_process_timeout_kills_process_group_and_reaps_child(self):
        process = Mock()
        process.pid = 1234
        process.wait.side_effect = [subprocess.TimeoutExpired(["fixed"], 180), 0]
        popen = Mock(return_value=process)

        with patch.object(verify_non_ui.os, "killpg") as killpg:
            status = verify_non_ui.run_process(
                ("fixed",), cwd=self.root, env={}, timeout=180, popen=popen
            )

        self.assertEqual(status, "timed_out")
        killpg.assert_called_once_with(1234, verify_non_ui.signal.SIGKILL)
        self.assertEqual(process.wait.call_count, 2)
        self.assertEqual(process.wait.call_args_list[0].kwargs["timeout"], 180)
        self.assertEqual(process.wait.call_args_list[1].kwargs["timeout"], 2)
        self.assertEqual(popen.call_args.kwargs["stdin"], subprocess.DEVNULL)
        self.assertTrue(popen.call_args.kwargs["start_new_session"])

    def test_unresponsive_child_has_bounded_post_kill_wait(self):
        process = Mock()
        process.pid = 1234
        process.wait.side_effect = [
            subprocess.TimeoutExpired(["fixed"], 180),
            subprocess.TimeoutExpired(["fixed"], 2),
        ]
        with patch.object(verify_non_ui.os, "killpg"):
            status = verify_non_ui.run_process(
                ("fixed",), cwd=self.root, env={}, timeout=180, popen=Mock(return_value=process)
            )

        self.assertEqual(status, "timed_out_cleanup_incomplete")
        self.assertEqual(process.wait.call_count, 2)
        self.assertEqual(process.wait.call_args_list[1].kwargs["timeout"], 2)

    def test_keyboard_interrupt_cleans_child_and_stops_check_sequence(self):
        process = Mock()
        process.pid = 1234
        process.wait.side_effect = [KeyboardInterrupt(), 0]
        calls = []
        popen = Mock(return_value=process)

        def runner(argv, **kwargs):
            calls.append(argv)
            return verify_non_ui.run_process(argv, popen=popen, **kwargs)

        with patch.object(verify_non_ui.os, "killpg") as killpg:
            with self.assertRaises(KeyboardInterrupt):
                verify_non_ui.run_checks(repo_root=self.root, environ={}, process_runner=runner)

        self.assertEqual(len(calls), 1)
        killpg.assert_called_once_with(1234, verify_non_ui.signal.SIGKILL)
        self.assertEqual(process.wait.call_count, 2)
        self.assertEqual(process.wait.call_args_list[1].kwargs["timeout"], 2)

    def test_cli_help_and_unknown_arguments_are_handled_without_echoing_arguments(self):
        output = StringIO()
        with redirect_stdout(output):
            help_code = verify_non_ui.main(["--help"])
        self.assertEqual(help_code, 0)
        self.assertIn("Usage:", output.getvalue())

        output = StringIO()
        secret_argument = "--secret-argument-sentinel"
        with redirect_stdout(output):
            invalid_code = verify_non_ui.main([secret_argument])
        self.assertEqual(invalid_code, 2)
        self.assertNotIn(secret_argument, output.getvalue())
        self.assertIn('"setup_status": "invalid"', output.getvalue())


if __name__ == "__main__":
    unittest.main()
