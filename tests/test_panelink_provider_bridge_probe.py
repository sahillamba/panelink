from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

from scripts.probe_panelink_provider_bridge import (
    ServeProcess,
    append_rolling,
    parse_target,
    read_live_managed_binding,
    safe_cli_target,
    sanitized_snapshot,
    select_session,
    session_picker_screen,
    filtered_session_row,
)


SCRIPT = Path(__file__).resolve().parents[1] / "scripts/probe_panelink_provider_bridge.py"
TARGET = "71:2:3:4:5"


class PanelinkProviderBridgeProbeTests(unittest.TestCase):
    def test_pty_transcript_retains_newest_bounded_suffix(self) -> None:
        retained = bytearray(b"old")
        append_rolling(retained, b"-new-content", 8)
        self.assertEqual(retained, b"-content")
        append_rolling(retained, b"123456789", 4)
        self.assertEqual(retained, b"6789")

    def test_picker_guard_accepts_generic_heading_only_with_exact_rows(self) -> None:
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")
        self.assertTrue(session_picker_screen("Sessions\n" + "\n".join(labels), labels))
        self.assertFalse(session_picker_screen("Sessions\n" + labels[0], labels))
        self.assertTrue(filtered_session_row("Sessions\n" + labels[1], labels[1], labels[0]))
        self.assertFalse(filtered_session_row("Sessions\n" + "\n".join(labels), labels[1], labels[0]))
        # The current session title can remain above the modal while the
        # picker itself has a unique filtered result.
        self.assertTrue(filtered_session_row(
            labels[0] + "\nSessions\n" + labels[1], labels[1], labels[0],
        ))

    def test_picker_never_enters_when_attach_and_fresh_child_do_not_filter(self) -> None:
        labels = ("Panelink V2 Identity A", "Panelink V2 Identity B")

        class FakeAttach:
            def __init__(self) -> None:
                self.output = bytearray(b"ready")
                self.total_output_bytes = len(self.output)
                self.sent = bytearray()

            def alive(self) -> bool:
                return True

            def drain(self, _timeout: float = 0.0) -> bytes:
                return b""

            def screen_text(self) -> str:
                # Simulate a transcript that keeps both rows after the query.
                return "Sessions\n" + "\n".join(labels)

            def send(self, payload: bytes) -> None:
                self.sent.extend(payload)
                self.total_output_bytes += 1

        attach = FakeAttach()
        fresh_reads = []

        def fresh_child_read() -> dict[str, object]:
            fresh_reads.append(True)
            return {
                "status": "known", "contains_picker_heading": True,
                "contains_both_fixture_labels": True,
                "contains_target_label": True, "contains_competitor_label": True,
            }

        trace: dict[str, object] = {}
        selected = select_session(
            attach, labels[1], labels[0], labels, time.monotonic() + 0.05,
            trace, pre_read={"status": "known", "contains_opencode_title": True,
                             "screen_bytes": 5},
            fresh_child_read=fresh_child_read,
        )
        self.assertFalse(selected)
        self.assertNotIn(b"\r", attach.sent)
        self.assertEqual(attach.sent, b"\x0f" + labels[1].encode("ascii"))
        self.assertGreaterEqual(len(fresh_reads), 2)
        self.assertFalse(trace.get("filtered_child_read_valid") and selected)

    def test_exact_target_parser_is_bounded_and_canonical(self) -> None:
        self.assertEqual(parse_target(TARGET), (71, 2, 3, 4, 5))
        for value in (
            "latest", "1:2:3:4", "1:2:3:4:5:6", "0:2:3:4:5", "01:2:3:4:5",
            "1:2:3:4:-5", "1:2:3:4:18446744073709551616", None, 4,
        ):
            with self.subTest(value=value):
                self.assertIsNone(parse_target(value))

    def test_historical_cli_target_requires_exact_structured_fields(self) -> None:
        self.assertEqual(safe_cli_target({
            "generation": 71, "workspace": 2, "pane": 3,
            "session": 4, "occupant_revision": 5,
        }), TARGET)
        for value in (
            TARGET,
            {"generation": True, "workspace": 2, "pane": 3,
             "session": 4, "occupant_revision": 5},
            {"generation": 71, "workspace": 2, "pane": 3, "session": 4},
        ):
            with self.subTest(value=value):
                self.assertIsNone(safe_cli_target(value))

    def test_snapshot_accepts_only_fixed_metadata_for_exact_target(self) -> None:
        value = {
            "ok": True,
            "action": "provider-binding",
            "target": TARGET,
            "status": "last_confirmed",
            "last_confirmed_revision": 8,
            "current_at_crash": False,
            "automatic_resume": False,
            "provider_session_id": "do-not-retain",
        }
        clean = sanitized_snapshot(value, TARGET)
        self.assertEqual(clean, {
            "target": TARGET,
            "status": "last_confirmed",
            "last_confirmed_revision": 8,
            "current_at_crash": False,
            "automatic_resume": False,
        })
        self.assertNotIn("provider_session_id", clean)
        self.assertIsNone(sanitized_snapshot({**value, "target": "71:2:3:4:6"}, TARGET))
        self.assertIsNone(sanitized_snapshot({**value, "current_at_crash": True}, TARGET))
        self.assertIsNone(sanitized_snapshot({**value, "last_confirmed_revision": True}, TARGET))

    def test_live_combined_record_requires_exact_scope_and_fixture_id(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-live-slot-") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            project = root / "project"
            project.mkdir(mode=0o700)
            server = root / "server"
            server.mkdir(mode=0o700)
            bindings = server / "provider-bindings"
            bindings.mkdir(mode=0o700)
            target_dir = bindings / TARGET.replace(":", "-")
            target_dir.mkdir(mode=0o700)
            project_bytes = str(project.resolve()).encode("ascii")
            session_bytes = b"ses_fixtureB"
            version = b"2.0.32"
            target_bytes = b"".join(number.to_bytes(8, "big") for number in parse_target(TARGET))
            binding = bytearray(b"PLBIND01" + target_bytes)
            binding.extend((9).to_bytes(8, "big"))
            binding.extend((2).to_bytes(8, "big"))
            binding.extend((10).to_bytes(8, "big"))
            binding.extend((1).to_bytes(2, "big"))
            binding.extend(len(version).to_bytes(2, "big"))
            binding.extend(len(project_bytes).to_bytes(2, "big"))
            binding.extend(len(session_bytes).to_bytes(2, "big"))
            binding.extend(bytes(16))
            binding.extend(version + project_bytes + session_bytes)
            managed = bytearray(b"PLMPRV01")
            managed.extend((parse_target(TARGET)[0]).to_bytes(8, "big"))
            managed.extend((1).to_bytes(8, "big"))
            managed.extend((1).to_bytes(8, "big"))
            managed.extend(bytes([1]) * 32)
            managed.extend(bytes((1, 1)))
            managed.extend(bytes(2))  # no optional executable path
            managed.extend(len(binding).to_bytes(4, "big"))
            managed.extend(binding)
            slot = target_dir / "managed-provider.record"
            slot.write_bytes(managed)
            slot.chmod(0o600)

            valid = read_live_managed_binding(server, TARGET, project, session_bytes.decode())
            self.assertEqual(valid["revision"], 2)
            self.assertEqual(valid["provider_version"], "2.0.32")
            self.assertNotIn(session_bytes.decode(), json.dumps(valid))
            self.assertIsNone(read_live_managed_binding(server, TARGET, project, "ses_other"))
            self.assertIsNone(read_live_managed_binding(
                server, "71:2:3:4:6", project, session_bytes.decode(),
            ))
            self.assertIsNone(read_live_managed_binding(
                server, TARGET, root, session_bytes.decode(),
            ))

    def test_serve_capture_keeps_only_allowlisted_status_and_target(self) -> None:
        with tempfile.TemporaryDirectory(prefix="panelink-p4-probe-unit-") as temporary:
            root = Path(temporary)
            script = root / "fake-serve.py"
            script.write_text(
                "import json\n"
                f"print(json.dumps({{'action':'serve','state':'running','generation':71,'target':'{TARGET}'}}), flush=True)\n"
                f"print(json.dumps({{'action':'provider-binding','target':'{TARGET}','status':'last_confirmed','last_confirmed_revision':8,'current_at_crash':False,'automatic_resume':False,'provider_session_id':'secret'}}), flush=True)\n",
                encoding="utf-8",
            )
            handle = ServeProcess([sys.executable, str(script)], root,
                                  {"PATH": os.defpath}, expected_target="")
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline and handle.latest is None:
                handle.poll(0.02)
            try:
                self.assertEqual(handle.startup, {"target": TARGET, "generation": 71})
                self.assertEqual(handle.latest["status"], "last_confirmed")
                self.assertNotIn("provider_session_id", handle.latest)
                self.assertNotIn("secret", json.dumps(handle.latest))
            finally:
                if handle.process.poll() is None:
                    handle.process.terminate()
                handle.process.wait(timeout=2)
                handle.selector.close()
                if handle.process.stdout is not None:
                    handle.process.stdout.close()

    @unittest.skipUnless(os.environ.get("PANELINK_RUN_P4_MANAGED_BRIDGE") == "1",
                         "real native acceptance is run explicitly after the isolated binary build")
    def test_real_managed_bridge_acceptance(self) -> None:
        binary = os.environ.get(
            "PANELINK_BINARY",
            "/private/tmp/panelink-p4-bridge-store-target/debug/panelink",
        )
        completed = subprocess.run(
            [sys.executable, str(SCRIPT), "--panelink-binary", binary],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=90, check=False, close_fds=True, shell=False,
        )
        self.assertLessEqual(len(completed.stdout), 16 * 1024)
        self.assertLessEqual(len(completed.stderr), 4 * 1024)
        result = json.loads(completed.stdout)
        self.assertEqual(result["classification"], "proven", result.get("reason"))
        self.assertTrue(result["root_a_to_b_to_a"])
        self.assertTrue(result["initial_a_binding_verified"])
        self.assertTrue(result["interim_b_binding_verified"])
        self.assertTrue(result["return_a_binding_verified"])
        self.assertGreaterEqual(result["final_revisions"][0], 2)
        self.assertTrue(result["passive_b_unchanged"])
        self.assertTrue(result["home_stale_and_returned"])
        self.assertTrue(result["historical_identity_matches"])
        self.assertTrue(result["historical_after_stop"])
        self.assertFalse(result["current_at_crash"])
        self.assertFalse(result["automatic_resume"])
        self.assertEqual(result["prompt_submissions"], 0)
        self.assertTrue(result["user_message_absence_verified"])
        self.assertTrue(result["cleanup_completed"])
        self.assertNotIn(b"provider_session_id", completed.stdout)
        self.assertNotIn(b"session-A", completed.stdout)
        self.assertNotIn(b"session-B", completed.stdout)


if __name__ == "__main__":
    unittest.main()
