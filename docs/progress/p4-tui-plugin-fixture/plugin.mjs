import { appendFileSync, statSync } from "node:fs"
import { createHash } from "node:crypto"
import { performance } from "node:perf_hooks"

const output = process.env.PANELINK_PROBE_RECORDS
const maxBytes = 2 * 1024 * 1024
let sequence = 0
let callbackRegistered = false
let stopped = false

function sessionHash(sessionID) {
  if (typeof sessionID !== "string" || sessionID.length === 0) return null
  return createHash("sha256").update(sessionID, "utf8").digest("hex")
}

function record(api, kind = "poll", selectedSessionID) {
  if (!output || stopped) return
  let route
  try {
    route = api.route.current
  } catch {
    return
  }
  const routeName = typeof route?.name === "string" ? route.name : "unknown"
  const sessionID = selectedSessionID ?? (routeName === "session" ? route?.params?.sessionID : undefined)
  const line = JSON.stringify({
    seq: sequence++,
    monotonic_ms: performance.now(),
    route_name: routeName,
    session_id_hash: routeName === "session" || kind === "route_change_callback"
      ? sessionHash(sessionID)
      : null,
    kind,
    callback_registered: callbackRegistered,
  }) + "\n"
  try {
    if (statSync(output).size + Buffer.byteLength(line) > maxBytes) {
      stopped = true
      return
    }
    appendFileSync(output, line, { encoding: "utf8", mode: 0o600 })
  } catch {
    stopped = true
  }
}

export default {
  id: "panelink-tui-probe",
  tui: async (api) => {
    callbackRegistered = typeof api?.event?.on === "function"
    if (callbackRegistered) api.event.on("tui.session.select", () => record(api, "route_change_callback"))
    record(api, "plugin_loaded")
    const timer = setInterval(() => record(api), 100)
    timer.unref?.()
    api.lifecycle.onDispose(() => {
      clearInterval(timer)
      stopped = true
    })
  },
}
