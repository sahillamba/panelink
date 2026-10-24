import os
from pathlib import Path
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import unittest


PLUGIN_PATH = Path(__file__).resolve().parents[1] / "assets/opencode-bridge/tui.ts"
MAGIC = b"PLBRSN02"


def outer(kind, challenge, payload=b""):
    return MAGIC + struct.pack(">HHIQ", 2, kind, len(payload), challenge) + payload


def read_exact(sock, length):
    chunks = bytearray()
    while len(chunks) < length:
        chunk = sock.recv(length - len(chunks))
        if not chunk:
            raise EOFError("socket closed")
        chunks.extend(chunk)
    return bytes(chunks)


def read_message(sock):
    header = read_exact(sock, 24)
    if header[:8] != MAGIC:
        raise AssertionError("wrong bridge magic")
    version, kind, length, challenge = struct.unpack(">HHIQ", header[8:])
    if version != 2 or length > 4388:
        raise AssertionError("invalid bounded bridge header")
    return kind, challenge, read_exact(sock, length)


def send_fragmented(sock, data):
    for start in range(0, len(data), 7):
        sock.sendall(data[start:start + 7])
        time.sleep(0.002)


def hello_message(project, capability=b"P" * 32, instance=16):
    version = b"2.0.32"
    body = struct.pack(">QQQQQ", 11, 12, 13, 14, 15) + struct.pack(">Q", instance)
    body += capability + struct.pack(">HHI", len(version), len(project), 0) + version + project
    return b"PLBRDG01" + struct.pack(">HHI", 1, 1, len(body)) + bytes(16) + body


def v1_message(kind, body):
    return b"PLBRDG01" + struct.pack(">HHI", 1, kind, len(body)) + bytes(16) + body


def observation(payload):
    if payload[:8] != b"PLBRDG01" or struct.unpack(">HHI", payload[8:16]) != (1, 3, len(payload) - 32):
        raise AssertionError("observation must carry its bounded v1 envelope")
    payload = payload[32:]
    target = struct.unpack(">QQQQQ", payload[:40])
    instance, sequence = struct.unpack(">QQ", payload[40:56])
    scope, selection, source = payload[56:59]
    version_len, project_len, session_len = struct.unpack(">HHH", payload[60:66])
    offset = 68
    version = payload[offset:offset + version_len]
    offset += version_len
    project = payload[offset:offset + project_len]
    offset += project_len
    session = payload[offset:offset + session_len]
    assert offset + session_len == len(payload)
    return target, instance, sequence, scope, selection, source, version, project, session


def provision(conn, project, hello=None):
    hello = hello or hello_message(os.fsencode(project))
    send_fragmented(conn, outer(5, 0, hello))
    kind, challenge, received = read_message(conn)
    if (kind, challenge, received) != (1, 0, hello):
        raise AssertionError("plugin did not echo the provisioned scoped Hello")


def validate_request(session_id):
    encoded = session_id.encode("ascii")
    return struct.pack(">H", len(encoded)) + encoded


def navigate_request(session_id, revision=9, ticket=b"T" * 16):
    encoded = session_id.encode("ascii")
    return struct.pack(">Q", revision) + ticket + struct.pack(">HH", len(encoded), 0) + encoded


NODE_DRIVER = r"""
import { pathToFileURL } from 'node:url'
const plugin = (await import(pathToFileURL(process.env.PANELINK_PLUGIN_MODULE))).default
const routes = [
  { type: 'session', sessionID: 'session-A' },
  { type: 'session', sessionID: 'session-B' },
  { type: 'home' },
  { type: 'session', sessionID: 'child-C' },
  { type: 'other' },
]
let current = 0
let routeReads = 0
let recoveryRoute = { type: 'home' }
const cache = new Map([
  ['session-A', { id: 'session-A', parentID: null, location: {directory: process.env.TEST_FOREIGN_CACHE ? '/' : process.env.TEST_PROJECT} }],
  ['session-B', { id: 'session-B', parentID: null, location: {directory: process.env.TEST_PROJECT} }],
  ['child-C', { id: 'child-C', parentID: 'parent', location: {directory: process.env.TEST_PROJECT} }],
])
const context = {
  location: { directory: process.env.TEST_PROJECT },
  app: { version: '2.0.32' },
  ui: { router: {
    current() { routeReads += 1; return process.env.TEST_RECOVERY ? recoveryRoute : routes[current++] },
    navigate(route) { recoveryRoute = route; },
  } },
  data: { session: { get(id) { return cache.get(id) } } },
  client: { session: { get({sessionID}) {
    if (process.env.TEST_SLOW_SDK) return new Promise((resolve) => setTimeout(() => resolve(cache.get(sessionID)), 500))
    return Promise.resolve(cache.get(sessionID))
  } } },
}
plugin.setup(context)
process.stdout.write(JSON.stringify({ started: true }))
setTimeout(() => process.exit(0), 3000)
"""


class OpenCodeBridgePluginTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.node = shutil.which("node")

    def setUp(self):
        if self.node is None:
            self.skipTest("Node.js is unavailable for the native plugin fixture")
        self.temporary = tempfile.TemporaryDirectory(prefix="panelink-opencode-bridge-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.socket_path = self.root / "bridge.sock"
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.socket_path))
        os.chmod(self.socket_path, 0o600)
        self.listener.settimeout(3)
        self.listener.listen(1)
        self.addCleanup(self.listener.close)
        self.loader = self.root / "loader.mjs"
        self.loader.write_text(
            "export async function resolve(specifier, context, nextResolve) {\n"
            " if (specifier === '@opencode/plugin/tui') return {url: "
            "'data:text/javascript,export%20const%20Plugin%20%3D%20%7B%20define%3A%20(v)%20%3D%3E%20v%20%7D', shortCircuit: true};\n"
            " return nextResolve(specifier, context)\n}\n",
            encoding="utf-8",
        )

    def run_node(self, with_expected_hello=True, foreign_cache=False, recovery=False, slow_sdk=False):
        env = os.environ.copy()
        env.update({
            "PANELINK_PLUGIN_MODULE": str(PLUGIN_PATH),
            "PANELINK_BRIDGE_SOCKET": str(self.socket_path),
            "TEST_PROJECT": str(self.root.resolve()),
        })
        if with_expected_hello:
            env["PANELINK_BRIDGE_HELLO"] = hello_message(os.fsencode(self.root.resolve())).hex()
        else:
            env.pop("PANELINK_BRIDGE_HELLO", None)
        if foreign_cache:
            env["TEST_FOREIGN_CACHE"] = "1"
        else:
            env.pop("TEST_FOREIGN_CACHE", None)
        if recovery:
            env["TEST_RECOVERY"] = "1"
        else:
            env.pop("TEST_RECOVERY", None)
        if slow_sdk:
            env["TEST_SLOW_SDK"] = "1"
        else:
            env.pop("TEST_SLOW_SDK", None)
        return subprocess.Popen(
            [self.node, "--no-warnings", "--experimental-loader", str(self.loader),
             "--input-type=module", "-e", NODE_DRIVER],
            cwd=self.root, env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )

    def test_fresh_requests_sample_route_and_encode_exact_scoped_observations(self):
        proc = self.run_node()
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            seen = []
            for challenge in range(1, 6):
                request = outer(3, challenge)
                if challenge == 1:
                    send_fragmented(conn, request)
                else:
                    conn.sendall(request)
                kind, received, payload = read_message(conn)
                self.assertEqual((kind, received), (4, challenge))
                seen.append(observation(payload))
                time.sleep(0.03)
            result, err = proc.communicate(timeout=5)
            self.assertEqual(proc.returncode, 0, err[:500])
            self.assertEqual(result.strip(), '{"started":true}')
            self.assertEqual([row[0] for row in seen], [(11, 12, 13, 14, 15)] * 5)
            self.assertEqual([row[1:6] for row in seen], [
                (16, 1, 1, 1, 1), (16, 2, 1, 1, 1), (16, 3, 1, 2, 0),
                (16, 4, 2, 3, 0), (16, 5, 1, 4, 0),
            ])
            self.assertEqual([row[8] for row in seen], [
                b"session-A", b"session-B", b"", b"", b"",
            ])
            self.assertTrue(all(row[6] == b"2.0.32" and row[7] == os.fsencode(self.root.resolve())
                                for row in seen))
            self.assertNotIn(hello_message(os.fsencode(self.root.resolve())).hex(), result + err)
            self.assertNotIn("session-A", result + err)
        finally:
            conn.close()
            if proc.poll() is None:
                proc.kill()
            proc.communicate(timeout=2)

    def test_pre_welcome_data_and_replayed_challenge_close_channel(self):
        proc = self.run_node()
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            conn.sendall(outer(5, 0, hello_message(os.fsencode(self.root.resolve()))))
            self.assertEqual(read_message(conn)[:2], (1, 0))
            conn.sendall(outer(3, 1))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_replayed_challenge_closes_channel(self):
        proc = self.run_node()
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(3, 1))
            self.assertEqual(read_message(conn)[:2], (4, 1))
            conn.sendall(outer(3, 1))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_server_provision_supplies_scope_when_hello_environment_is_absent(self):
        proc = self.run_node(with_expected_hello=False)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(3, 1))
            response = read_message(conn)
            self.assertEqual(response[:2], (4, 1))
            row = observation(response[2])
            self.assertEqual(row[:6], ((11, 12, 13, 14, 15), 16, 1, 1, 1, 1))
            self.assertEqual(row[8], b"session-A")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_socket_symlink_is_rejected_before_connection(self):
        real_path = self.root / "real-bridge.sock"
        self.socket_path.rename(real_path)
        self.socket_path.symlink_to(real_path.name)
        proc = self.run_node(with_expected_hello=False)
        try:
            _stdout, stderr = proc.communicate(timeout=3)
            self.assertNotEqual(proc.returncode, 0)
            self.assertLessEqual(len(stderr), 4096)
            self.assertNotIn("PANELINK_BRIDGE_SOCKET", stderr)
            self.listener.settimeout(0.1)
            with self.assertRaises(socket.timeout):
                self.listener.accept()
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.communicate(timeout=2)

    def test_malformed_server_provision_closes_without_sending_hello(self):
        proc = self.run_node(with_expected_hello=False)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            conn.sendall(outer(5, 0, b"malformed"))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_foreign_cached_project_cannot_confirm_in_original_project(self):
        proc = self.run_node(with_expected_hello=False, foreign_cache=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(3, 1))
            response = read_message(conn)
            self.assertEqual(response[:2], (4, 1))
            row = observation(response[2])
            self.assertEqual(row[3:6], (1, 4, 0))
            self.assertEqual(row[8], b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_optional_hello_restricts_provision_scope(self):
        proc = self.run_node(with_expected_hello=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            mismatched = hello_message(os.fsencode(self.root.resolve()), instance=17)
            conn.sendall(outer(5, 0, mismatched))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_malformed_oversized_header_is_closed_before_body(self):
        proc = self.run_node()
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            conn.sendall(outer(5, 0, hello_message(os.fsencode(self.root.resolve()))))
            self.assertEqual(read_message(conn)[:2], (1, 0))
            welcome = v1_message(2, struct.pack(">Q", 16))
            conn.sendall(outer(2, 0, welcome))
            time.sleep(0.05)
            conn.sendall(MAGIC + struct.pack(">HHIQ", 2, 3, 4389, 1))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_recovery_validation_shares_challenge_but_not_snapshot_sequence(self):
        proc = self.run_node(recovery=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(6, 1, validate_request("session-A")))
            kind, challenge, payload = read_message(conn)
            self.assertEqual((kind, challenge), (7, 1))
            self.assertEqual(payload[0], 1)
            self.assertEqual(payload[1], 0)
            id_len, project_len = struct.unpack(">HH", payload[2:6])
            self.assertEqual(payload[6:8], b"\x00\x00")
            self.assertEqual(payload[8:8 + id_len], b"session-A")
            self.assertEqual(payload[8 + id_len:8 + id_len + project_len], os.fsencode(self.root.resolve()))
            conn.sendall(outer(3, 2))
            response = read_message(conn)
            self.assertEqual(response[:2], (4, 2))
            row = observation(response[2])
            self.assertEqual((row[2], row[3:6], row[8]), (1, (1, 2, 0), b""))
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_navigation_is_one_shot_and_only_later_snapshot_proves_selection(self):
        proc = self.run_node(recovery=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(6, 1, validate_request("session-B")))
            self.assertEqual(read_message(conn)[:2], (7, 1))
            conn.sendall(outer(8, 2, navigate_request("session-B")))
            kind, challenge, payload = read_message(conn)
            self.assertEqual((kind, challenge), (9, 2))
            self.assertEqual(payload[0:2], b"\x01\x00")
            self.assertEqual(struct.unpack(">H", payload[2:4])[0], len(b"session-B"))
            self.assertEqual(struct.unpack(">Q", payload[4:12])[0], 9)
            self.assertEqual(payload[12:28], b"T" * 16)
            self.assertEqual(payload[28:], b"session-B")
            conn.sendall(outer(3, 3))
            response = read_message(conn)
            self.assertEqual(response[:2], (4, 3))
            row = observation(response[2])
            self.assertEqual((row[2], row[3:6], row[8]), (1, (1, 1, 1), b"session-B"))
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_control_frame_before_welcome_and_malformed_control_are_rejected(self):
        proc = self.run_node(recovery=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            conn.sendall(outer(6, 1, validate_request("session-A")))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_malformed_control_payload_closes_channel(self):
        proc = self.run_node(recovery=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            conn.sendall(outer(6, 1, b"\x00\x00"))
            self.assertEqual(conn.recv(1), b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)

    def test_slow_sdk_returns_uncertain_inside_host_deadline_and_cannot_enable_navigation(self):
        proc = self.run_node(recovery=True, slow_sdk=True)
        conn, _ = self.listener.accept()
        conn.settimeout(2)
        try:
            provision(conn, self.root.resolve())
            conn.sendall(outer(2, 0, v1_message(2, struct.pack(">Q", 16))))
            started = time.monotonic()
            conn.sendall(outer(6, 1, validate_request("session-A")))
            kind, challenge, payload = read_message(conn)
            elapsed = time.monotonic() - started
            self.assertEqual((kind, challenge), (7, 1))
            self.assertEqual(payload[0], 6)
            self.assertLess(elapsed, 0.30)
            conn.sendall(outer(8, 2, navigate_request("session-A")))
            kind, challenge, payload = read_message(conn)
            self.assertEqual((kind, challenge), (9, 2))
            self.assertEqual(payload[0], 2)
            conn.sendall(outer(3, 3))
            row = observation(read_message(conn)[2])
            self.assertEqual(row[8], b"")
        finally:
            conn.close()
            if proc.poll() is None:
                proc.terminate()
            proc.communicate(timeout=3)


if __name__ == "__main__":
    unittest.main()
