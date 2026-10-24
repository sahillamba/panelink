from pathlib import Path
import contextlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import base64
import urllib.error
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from scripts import probe_opencode_v2_identity as probe
from scripts.probe_opencode_v2_identity import (
    EXPECTED_SHA256,
    SANDBOX_PROFILE,
    bounded_worker,
    launcher_argv,
    main,
    request as authenticated_request,
    _run_worker,
    server_owns_port,
    session_payload_id,
    verify_no_user_messages,
)


class OpenCodeV2IdentityPreflightTests(unittest.TestCase):
    def test_isolation_scenario_dispatch_installs_the_required_hook(self):
        import types
        for scenario, module_name, result_key in (
                ("child-routes", "probe_opencode_v2_child_routes", "child_routes"),
                ("project-isolation", "probe_opencode_v2_project_isolation", "project_isolation")):
            hook = lambda context: {"classification": "not_proven"}
            module = types.ModuleType("scripts." + module_name)
            module.after_routes = hook
            with patch.dict(sys.modules, {"scripts." + module_name: module}), \
                    patch("scripts.probe_opencode_v2_tui_identity.after_bootstrap",
                          side_effect=lambda context: context):
                context = probe.scenario_callback(scenario)({"fixture": True})
            self.assertEqual(context["scenario"], scenario)
            self.assertEqual(context["hook_result_key"], result_key)
            self.assertIs(context["after_routes"], hook)
            self.assertFalse(context["new_session_key_verified"])

    def test_route_lookup_matches_exact_hash_instead_of_newest_session(self):
        with tempfile.TemporaryDirectory() as temporary:
            project = Path(temporary)
            rows = [{"id": "ses_newest", "location": {"directory": str(project)}},
                    {"id": "ses_exact", "location": {"directory": str(project)}}]
            context = {"port": 1234, "server_pid": 123, "project": project,
                       "base": "http://127.0.0.1:1234", "password": "fixture-secret"}
            verified = ["ses_a", "ses_b"]
            import hashlib
            target = hashlib.sha256(b"ses_exact").hexdigest()
            with patch.object(probe, "server_owns_port", return_value=True), \
                    patch.object(probe, "request", return_value=(200, json.dumps({"data": rows}).encode())):
                result = probe.lookup_route_hash(context, target, verified)
            self.assertEqual(result["status"], "found")
            self.assertTrue(result["project_matches"])
            self.assertEqual(verified, ["ses_a", "ses_b", "ses_exact"])
            self.assertNotIn("ses_exact", json.dumps(result))
            with patch.object(probe, "server_owns_port", return_value=False), \
                    patch.object(probe, "request") as request:
                self.assertEqual(probe.lookup_route_hash(context, target, verified)["status"], "error")
                request.assert_not_called()

    def test_empty_message_verification_includes_a_new_third_session_and_is_bounded(self):
        with patch.object(probe, "request", return_value=(200, b'{"data":[]}')) as read:
            self.assertTrue(verify_no_user_messages("http://127.0.0.1:1234", "fixture", ("a", "b", "c"))[0])
            self.assertEqual(read.call_count, 3)
            self.assertEqual(verify_no_user_messages("http://127.0.0.1:1234", "fixture", ("a", "b", "c", "d")), (False, []))
            self.assertEqual(read.call_count, 3)

    def test_user_message_absence_requires_successful_empty_reads_for_both_sessions(self):
        for responses, expected in (([(200, b'{"data":[]}')] * 2, True),
                                    ([(200, b'{"data":[]}'), (200, b'{"data":[{}]}')], False),
                                    ([(200, b'{"data":[]}'), (404, b'')], False)):
            with patch("scripts.probe_opencode_v2_identity.request", side_effect=responses) as read:
                verified, statuses = verify_no_user_messages(
                    "http://127.0.0.1:1234", "fixture-secret", ("ses_a", "ses_b"))
            self.assertEqual(verified, expected)
            self.assertEqual(statuses, [row[0] for row in responses])
            self.assertTrue(all(call.args[1] == "GET" for call in read.call_args_list))

    def test_pinned_native_binary_digest_has_exact_sha256_shape(self):
        self.assertRegex(EXPECTED_SHA256, re.compile(r"^[0-9a-f]{64}$"))

    def test_sandbox_denies_network_and_only_allows_loopback(self):
        self.assertIn("(deny network*)", SANDBOX_PROFILE)
        self.assertIn('(remote ip "localhost:*")', SANDBOX_PROFILE)
        self.assertIn('(local ip "localhost:*")', SANDBOX_PROFILE)
        self.assertNotIn('remote ip "*:*"', SANDBOX_PROFILE)

    def test_session_id_requires_v2_response_and_top_level_session(self):
        self.assertEqual(session_payload_id(b'{"data":{"id":"ses_abc","parentID":null}}'), "ses_abc")
        self.assertEqual(session_payload_id(b'{"data":{"id":"ses_abc"}}'), "ses_abc")
        self.assertIsNone(session_payload_id(b'{"data":{"id":"ses_child","parentID":"ses_parent"}}'))
        self.assertIsNone(session_payload_id(b'{"id":"not-v2-response"}'))

    def test_request_sends_basic_auth_and_never_forwards_it_on_redirect(self):
        class RedirectResponse:
            def __init__(self):
                self.requests = []

            def open(self, req, timeout):
                self.requests.append((req, timeout))
                raise urllib.error.HTTPError(
                    req.full_url, 302, "redirect", {"Location": "https://outside.invalid/"}, None)

        fake_opener = RedirectResponse()
        with patch("scripts.probe_opencode_v2_identity.urllib.request.build_opener",
                   return_value=fake_opener):
            status, body = authenticated_request(
                "http://127.0.0.1:12345", "GET", "/redirect",
                password="fixture-secret")
        self.assertEqual((status, body), (302, b""))
        expected = "Basic " + base64.b64encode(b"opencode:fixture-secret").decode("ascii")
        self.assertEqual(len(fake_opener.requests), 1)
        self.assertEqual(fake_opener.requests[0][0].get_header("Authorization"), expected)
        req = fake_opener.requests[0][0]
        self.assertIsNone(probe._NoRedirectHandler().redirect_request(
            req, None, 302, "redirect", {"Location": "https://outside.invalid/"},
            "https://outside.invalid/"))

    def test_server_ownership_requires_exact_single_pid(self):
        result = subprocess.CompletedProcess([], 0, stdout=b"123\n", stderr=b"")
        with patch("scripts.probe_opencode_v2_identity.subprocess.run", return_value=result):
            self.assertTrue(server_owns_port(8123, 123))
        with patch("scripts.probe_opencode_v2_identity.subprocess.run", return_value=result):
            self.assertFalse(server_owns_port(8123, 456))
        duplicate = subprocess.CompletedProcess([], 0, stdout=b"123\n456\n", stderr=b"")
        with patch("scripts.probe_opencode_v2_identity.subprocess.run", return_value=duplicate):
            self.assertFalse(server_owns_port(8123, 123))
        failed = subprocess.CompletedProcess([], 1, stdout=b"", stderr=b"")
        with patch("scripts.probe_opencode_v2_identity.subprocess.run", return_value=failed):
            self.assertFalse(server_owns_port(8123, 123))

    def test_worker_output_collector_is_bounded(self):
        with tempfile.TemporaryDirectory() as temporary:
            code, output, error = bounded_worker(
                [sys.executable, "-c", "import sys; sys.stdout.write('x' * 20000)"],
                Path(temporary), {}, timeout=3.0)
        self.assertEqual(code, 2)
        self.assertEqual(output, b"")
        self.assertEqual(error, "sandbox_worker_output_limit")

    def test_tescode_wrapper_passes_opencode_arguments_through_custom_workflow(self):
        self.assertEqual(launcher_argv("/bin/tescode", "tescode", ["--version"]),
                         ["/bin/tescode", "workflow", "custom", "--version"])

    def _worker_mocks(self, request_side_effect, *, ownership=True):
        capture = SimpleNamespace(stdout=b"OpenCode 2.0.32", stderr=b"", returncode=0, reason=None)
        patches = [
            patch("scripts.probe_opencode_v2_identity.sha256_file", return_value=EXPECTED_SHA256),
            patch("scripts.probe_opencode_v2_identity.environment", side_effect=lambda root: {
                "HOME": str(root), "PATH": "/usr/bin"}),
            patch("scripts.probe_opencode_v2_identity.bounded_command", return_value=capture),
            patch("scripts.probe_opencode_v2_identity.free_loopback_port", return_value=8123),
            patch("scripts.probe_opencode_v2_identity.request", side_effect=request_side_effect),
            patch("scripts.probe_opencode_v2_identity.server_owns_port", return_value=ownership),
            patch("scripts.probe_opencode_v2_identity._kill_group", return_value=True),
            patch("scripts.probe_opencode_v2_identity.subprocess.Popen")
        ]
        started = [p.start() for p in patches]
        process = started[-1].return_value
        process.pid = 123
        process.poll.return_value = None
        self.addCleanup(lambda: [p.stop() for p in reversed(patches)])
        return started[4], started[5], process

    def test_not_ready_does_not_post_and_reports_owned_listener_state(self):
        request, ownership, _process = self._worker_mocks(lambda *_args, **_kwargs: (None, b""), ownership=False)
        with patch("scripts.probe_opencode_v2_identity.SERVER_DEADLINE_SECONDS", 0.0):
            result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["reason"], "local_server_not_ready")
        self.assertFalse(any(call.args[1] == "POST" for call in request.call_args_list))
        self.assertFalse(result["listener_owned_at_deadline"])

    def test_listener_ownership_failure_does_not_post(self):
        request, _ownership, _process = self._worker_mocks(lambda _base, method, path, *_a, **_kw:
            (200, b"{}") if method == "GET" and path == "/api/info" else (None, b""), ownership=False)
        with patch("scripts.probe_opencode_v2_identity.SERVER_DEADLINE_SECONDS", 0.0):
            result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["reason"], "local_server_not_ready")
        self.assertEqual(request.call_count, 0)
        self.assertFalse(any(call.args[1] == "POST" for call in request.call_args_list))

    def test_foreign_pid_in_api_info_does_not_post(self):
        request, _ownership, _process = self._worker_mocks(
            lambda _base, method, path, **_kw:
                (200, b'{"version":"2.0.32","pid":999}') if method == "GET" and path == "/api/info"
                else (None, b""))
        result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["reason"], "local_server_identity_mismatch")
        self.assertFalse(any(call.args[1] == "POST" for call in request.call_args_list))

    def test_listener_ownership_loss_after_readiness_does_not_post(self):
        request, ownership, _process = self._worker_mocks(
            lambda _base, method, path, **_kw:
                (200, b'{"version":"2.0.32","pid":123}') if method == "GET" and path == "/api/info"
                else (None, b""))
        ownership.side_effect = [True, False]
        result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["reason"], "local_server_ownership_unproven")
        self.assertFalse(any(call.args[1] == "POST" for call in request.call_args_list))

    def test_own_server_auth_rejection_stops_before_session_creation(self):
        request, _ownership, _process = self._worker_mocks(
            lambda *_args, **_kwargs: (401, b""))
        result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["reason"], "local_server_authentication_required")
        self.assertEqual(result["readiness_http_status"], 401)
        self.assertEqual(request.call_count, 1)
        self.assertEqual(result["sessions_created_count"], 0)
        self.assertTrue(result["cleanup_completed"])

    def test_bootstrap_success_still_reports_unproven_identity_and_nonzero_cli(self):
        ids = iter(("ses_aa", "ses_bb"))
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")
        def response(_base, method, path, body=None, **_kwargs):
            if method == "GET" and path == "/api/info":
                return 200, b'{"version":"2.0.32","pid":123}'
            if method == "POST" and path == "/api/session":
                identifier = next(ids)
                return 200, json.dumps({"data": {"id": identifier, "parentID": None}}).encode()
            if method == "GET" and path.startswith("/api/session?"):
                rows = [{"id": sid, "title": title, "parentID": None}
                        for sid, title in zip(("ses_aa", "ses_bb"), labels)]
                return 200, json.dumps({"data": rows}).encode()
            return 404, b""
        request, _ownership, _process = self._worker_mocks(response)
        result = _run_worker("/fake/opencode", "opencode")
        self.assertEqual(result["classification"], "preflight_passed_identity_not_proven")
        self.assertEqual(result["sessions_created_count"], 2)
        self.assertFalse(result["switch_A_B_A_observed"])
        self.assertEqual(result["identity_proof"], "not_attempted_tui_switch_unverified")
        self.assertEqual(sum(call.args[1] == "POST" for call in request.call_args_list), 2)
        child_env = probe.subprocess.Popen.call_args.kwargs["env"]
        self.assertTrue(child_env["OPENCODE_PASSWORD"])
        self.assertNotIn(child_env["OPENCODE_PASSWORD"], json.dumps(result))
        output = io.StringIO()
        with patch.dict(os.environ, {"PANELINK_OPENCODE_V2_SANDBOX_WORKER": "1"}), \
                patch("scripts.probe_opencode_v2_identity._run_worker", return_value=result), \
                contextlib.redirect_stdout(output):
            self.assertEqual(main(["--sandbox-worker", "/fake/opencode", "opencode"]), 2)
        self.assertEqual(json.loads(output.getvalue())["classification"],
                         "preflight_passed_identity_not_proven")

    def test_optional_after_bootstrap_callback_receives_private_fixture_context(self):
        ids = iter(("ses_aa", "ses_bb"))

        def response(_base, method, path, body=None, **_kwargs):
            if method == "GET" and path == "/api/info":
                return 200, b'{"version":"2.0.32","pid":123}'
            if method == "POST" and path == "/api/session":
                return 200, json.dumps({"data": {"id": next(ids), "parentID": None}}).encode()
            if method == "GET" and path.startswith("/api/session?"):
                return 200, json.dumps({"data": [
                    {"id": "ses_aa", "title": "Panelink V2 Identity A", "parentID": None},
                    {"id": "ses_bb", "title": "Panelink V2 Identity B", "parentID": None},
                ]}).encode()
            return 404, b""

        request, _ownership, process = self._worker_mocks(response)
        captured = []

        def callback(context):
            captured.append(context)
            self.assertIsInstance(context["root"], Path)
            self.assertIsInstance(context["project"], Path)
            self.assertEqual(context["project"].parent, context["root"])
            self.assertEqual(context["session_ids"], ("ses_aa", "ses_bb"))
            self.assertTrue(context["password"])
            self.assertEqual(context["deadline"] > 0, True)
            self.assertIsNone(process.poll())
            self.assertEqual([call.args[1] for call in request.call_args_list].count("POST"), 2)
            return {"classification": "blocked", "reason": "fixture_not_present"}

        result = _run_worker("/fake/opencode", "opencode", after_bootstrap=callback)

        self.assertEqual(len(captured), 1)
        self.assertEqual(result["tui"], {"classification": "blocked", "reason": "fixture_not_present"})
        self.assertEqual(result["classification"], "preflight_passed_identity_not_proven")
        self.assertNotIn(captured[0]["password"], json.dumps(result))
        self.assertTrue(result["cleanup_completed"])

    def test_tui_success_is_downgraded_when_owned_server_cleanup_fails(self):
        ids = iter(("ses_aa", "ses_bb"))

        def response(_base, method, path, body=None, **_kwargs):
            if method == "GET" and path == "/api/info":
                return 200, b'{"version":"2.0.32","pid":123}'
            if method == "POST" and path == "/api/session":
                return 200, json.dumps({"data": {"id": next(ids), "parentID": None}}).encode()
            if method == "GET" and path.startswith("/api/session?"):
                return 200, json.dumps({"data": [
                    {"id": "ses_aa", "title": "Panelink V2 Identity A", "parentID": None},
                    {"id": "ses_bb", "title": "Panelink V2 Identity B", "parentID": None},
                ]}).encode()
            return 404, b""

        request, _ownership, _process = self._worker_mocks(response, ownership=True)
        cleanup = patch("scripts.probe_opencode_v2_identity._kill_group", return_value=False)
        cleanup.start()
        self.addCleanup(cleanup.stop)
        result = _run_worker(
            "/fake/opencode", "opencode",
            after_bootstrap=lambda _context: {"classification": "proven", "reason": "test"})
        self.assertEqual(result["classification"], "blocked")
        self.assertEqual(result["reason"], "fixture_server_cleanup_unverified")
        self.assertFalse(result["cleanup_completed"])
        self.assertEqual(request.call_count, 4)

    def test_tui_worker_exit_zero_requires_both_owned_cleanup_receipts(self):
        successful = {
            "classification": "selected_tui_identity_observed",
            "cleanup_completed": True,
            "tui": {"tui_cleanup_completed": True},
        }
        output = io.StringIO()
        with patch.dict(os.environ, {"PANELINK_OPENCODE_V2_SANDBOX_WORKER": "1"}), \
                patch("scripts.probe_opencode_v2_identity._run_worker", return_value=successful), \
                contextlib.redirect_stdout(output):
            self.assertEqual(main(["--sandbox-worker", "/fake/opencode", "opencode", "--tui-worker"]), 0)
        incomplete = {**successful, "tui": {"tui_cleanup_completed": False}}
        with patch.dict(os.environ, {"PANELINK_OPENCODE_V2_SANDBOX_WORKER": "1"}), \
                patch("scripts.probe_opencode_v2_identity._run_worker", return_value=incomplete), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main(["--sandbox-worker", "/fake/opencode", "opencode", "--tui-worker"]), 2)


if __name__ == "__main__":
    unittest.main()
