import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


PLUGIN_PATH = Path(__file__).resolve().parents[1] / "docs/progress/p4-v2-tui-plugin-fixture/plugin.mjs"
NODE_DRIVER = r"""
import { pathToFileURL } from "node:url"
import { readFile, unlink, writeFile } from "node:fs/promises"

const pluginModule = await import(pathToFileURL(process.env.PANELINK_PLUGIN_MODULE))
const plugin = pluginModule.default
let route = { type: "session", sessionID: "ses_a" }
let now = 0
let intervalCallback
let clearCount = 0
const sessions = new Map([
  ["ses_a", { id: "ses_a" }],
  ["ses_b", { id: "ses_b", parentID: null }],
  ["ses_child", { id: "ses_child", parentID: "ses_parent" }],
  ["ses_parent", { id: "ses_parent", parentID: null }],
])
const getCalls = []
const context = {
  ui: { router: { current: () => route } },
  location: { directory: process.env.PANELINK_PROBE_TEST_DIRECTORY },
  data: {
    session: { get(id) { getCalls.push(id); return sessions.get(id) } },
    listen() { throw new Error("event listener must not be used") },
    on() { throw new Error("event listener must not be used") },
  },
}
Object.defineProperty(globalThis.performance, "now", {
  configurable: true,
  value: () => now,
})
globalThis.setInterval = (callback, delay) => {
  if (delay !== 100) throw new Error("unexpected poll interval")
  intervalCallback = callback
  return { unref() {} }
}
globalThis.clearInterval = () => { clearCount += 1 }

const dispose = plugin.setup(context)
if (typeof dispose !== "function" || typeof intervalCallback !== "function") {
  throw new Error("fixture did not expose timer cleanup")
}
const poll = () => {
  now += Number(process.env.PANELINK_PROBE_TEST_STEP_MS || 100)
  intervalCallback()
}
let pausedCount = null
let pauseClearCount = null
let resumedCount = null
if (process.env.PANELINK_PROBE_TEST_PAUSE === "1") {
  await writeFile(process.env.PANELINK_PROBE_CONTROL, "pause", { mode: 0o600 })
  for (let index = 0; index < 3; index += 1) poll()
  pausedCount = (await readFile(process.env.PANELINK_PROBE_RECORDS, "utf8"))
    .trim().split("\n").length
  pauseClearCount = clearCount
  await unlink(process.env.PANELINK_PROBE_CONTROL)
  poll()
  poll()
  resumedCount = (await readFile(process.env.PANELINK_PROBE_RECORDS, "utf8"))
    .trim().split("\n").length
} else if (process.env.PANELINK_PROBE_TEST_CAP === "1") {
  for (let index = 0; index < 1005; index += 1) poll()
} else if (process.env.PANELINK_PROBE_TEST_CAP === "time") {
  for (let index = 0; index < 10; index += 1) poll()
} else {
  // Missing initial cache resolves from the selected route on a later poll.
  sessions.delete("ses_a")
  poll()
  sessions.set("ses_a", { id: "ses_a" })
  poll()

  // Switch A-B-A, then let unrelated child-session activity arrive while B remains selected.
  route = { type: "session", sessionID: "ses_b" }
  poll()
  sessions.set("ses_child", { id: "ses_child", parentID: "ses_parent", updated: true })
  poll()
  route = { type: "session", sessionID: "ses_a" }
  poll()
  route = { type: "session", sessionID: "ses_child" }
  poll()
}

dispose()
const records = (await (await import("node:fs/promises")).readFile(
  process.env.PANELINK_PROBE_RECORDS, "utf8"))
  .trim().split("\n").map((line) => JSON.parse(line))
poll()
process.stdout.write(JSON.stringify({ records, getCalls, clearCount, pausedCount, resumedCount, pauseClearCount }))
"""


class OpenCodeV2PluginFixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.node = shutil.which("node")

    def setUp(self):
        if self.node is None:
            self.skipTest("Node.js is unavailable for behavioral fixture mocks")
        self.temporary = tempfile.TemporaryDirectory(prefix="panelink-v2-plugin-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.records = self.root / "records.jsonl"
        self.loader = self.root / "loader.mjs"
        self.loader.write_text(
            "export async function resolve(specifier, context, nextResolve) {\n"
            "  if (specifier === '@opencode/plugin/tui') return {\n"
            "    url: 'data:text/javascript,export%20const%20Plugin%20%3D%20%7B%20define%3A%20(value)%20%3D%3E%20value%20%7D',\n"
            "    shortCircuit: true,\n"
            "  }\n"
            "  return nextResolve(specifier, context)\n"
            "}\n",
            encoding="utf-8",
        )

    def run_fixture(self, driver, extra_env=None):
        environment = os.environ.copy()
        environment.update({
            "PANELINK_PLUGIN_MODULE": str(PLUGIN_PATH),
            "PANELINK_PROBE_RECORDS": str(self.records),
            "PANELINK_PROBE_CLIENT_NONCE": "0123456789abcdef0123456789abcdef",
            "PANELINK_PROBE_TEST_DIRECTORY": str(self.root),
        })
        if extra_env and extra_env.get("PANELINK_PROBE_TEST_PAUSE") == "1":
            control = self.root / "control"
            environment["PANELINK_PROBE_CONTROL"] = str(control)
            self.addCleanup(lambda: control.unlink(missing_ok=True))
        environment.update(extra_env or {})
        result = subprocess.run(
            [self.node, "--no-warnings", "--experimental-loader", str(self.loader),
             "--input-type=module", "-e", driver],
            cwd=self.root,
            env=environment,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        self.assertEqual(result.returncode, 0, "fixture Node mock failed: " + result.stderr[:500])
        return json.loads(result.stdout)

    def test_selected_route_records_aba_and_child_route_without_event_inference(self):
        result = self.run_fixture(NODE_DRIVER)
        records = result["records"]
        self.assertEqual([record["kind"] for record in records],
                         ["plugin_loaded", "poll", "poll", "poll", "poll", "poll", "poll"])
        a_hash, b_hash, child_hash = (
            records[2]["session_id_hash"], records[3]["session_id_hash"],
            records[6]["session_id_hash"],
        )
        self.assertEqual([records[2]["session_id_hash"], records[3]["session_id_hash"],
                          records[5]["session_id_hash"]], [a_hash, b_hash, a_hash])
        self.assertEqual(records[3]["session_top_level"], True)
        self.assertEqual(records[4]["session_id_hash"], b_hash)
        self.assertEqual(records[4]["session_top_level"], True)
        self.assertEqual(records[6]["session_top_level"], False)
        self.assertNotEqual(child_hash, b_hash)
        self.assertEqual(result["getCalls"],
                         ["ses_a", "ses_a", "ses_a", "ses_b", "ses_b", "ses_a", "ses_child"])
        self.assertEqual(result["clearCount"], 1)
        self.assertEqual([record["seq"] for record in records], list(range(1, len(records) + 1)))
        self.assertIsNone(records[1]["session_top_level"])
        self.assertTrue(records[2]["session_top_level"])
        self.assertTrue(all(record["client_nonce"] == "0123456789abcdef0123456789abcdef"
                            for record in records))
        self.assertTrue(all(record["project_hash"] == records[0]["project_hash"]
                            for record in records))
        encoded = self.records.read_bytes()
        self.assertLessEqual(len(encoded), 2 * 1024 * 1024)
        self.assertNotIn(b"ses_a", encoded)
        self.assertNotIn(b"ses_b", encoded)
        self.assertNotIn(b"ses_child", encoded)
        self.assertNotIn(str(self.root).encode(), encoded)

    def test_record_cap_disposes_timer_at_one_thousand(self):
        result = self.run_fixture(NODE_DRIVER, {
            "PANELINK_PROBE_TEST_CAP": "1",
            "PANELINK_PROBE_TEST_STEP_MS": "1",
        })
        self.assertEqual(len(result["records"]), 1_000)
        self.assertEqual(result["records"][-1]["seq"], 1_000)
        self.assertEqual(result["clearCount"], 1)
        self.assertLessEqual(self.records.stat().st_size, 2 * 1024 * 1024)

    def test_runtime_cap_disposes_timer_after_sixty_seconds(self):
        result = self.run_fixture(NODE_DRIVER, {
            "PANELINK_PROBE_TEST_CAP": "time",
            "PANELINK_PROBE_TEST_STEP_MS": "10000",
        })
        self.assertEqual(len(result["records"]), 7)
        self.assertEqual(result["records"][-1]["monotonic_ms"], 60_000)
        self.assertEqual(result["clearCount"], 1)

    def test_optional_control_pause_suspends_writes_then_resumes_contiguous_sequence(self):
        result = self.run_fixture(NODE_DRIVER, {"PANELINK_PROBE_TEST_PAUSE": "1"})
        self.assertEqual(result["pausedCount"], 1)
        self.assertEqual(result["resumedCount"], 3)
        self.assertEqual([row["seq"] for row in result["records"]], [1, 2, 3])
        self.assertEqual(result["clearCount"], 1)

    def test_runtime_limit_still_disposes_timer_while_transport_is_paused(self):
        result = self.run_fixture(NODE_DRIVER, {"PANELINK_PROBE_TEST_PAUSE": "1",
                                               "PANELINK_PROBE_TEST_STEP_MS": "30000"})
        self.assertEqual(result["pauseClearCount"], 1)
        self.assertEqual(result["resumedCount"], 1)
        self.assertEqual(len(result["records"]), 1)


if __name__ == "__main__":
    unittest.main()
