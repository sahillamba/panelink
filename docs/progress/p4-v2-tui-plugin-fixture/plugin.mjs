import { Plugin } from "@opencode/plugin/tui"
import { createHash, randomBytes } from "node:crypto"
import { appendFileSync, readFileSync, realpathSync, statSync } from "node:fs"

const POLL_MS = 100
const MAX_RUN_MS = 60_000
const MAX_RECORDS = 1_000
const MAX_BYTES = 2 * 1024 * 1024
const HASH_RE = /^[0-9a-f]{32}$/

function sha256(value) {
  return createHash("sha256").update(value, "utf8").digest("hex")
}

function routeName(route) {
  if (!route || typeof route !== "object") return "other"
  if (route.type === "session" || route.type === "home" || route.type === "plugin") {
    return route.type
  }
  return "other"
}

function sessionObservation(context, route) {
  if (route?.type !== "session" || typeof route.sessionID !== "string"
      || route.sessionID.length < 1 || route.sessionID.length > 128) {
    return { session_id_hash: null, session_top_level: null }
  }

  let topLevel = null
  try {
    const selected = context.data?.session?.get(route.sessionID)
    if (selected && typeof selected === "object") {
      topLevel = selected.parentID == null
    }
  } catch {
    // The cache can be empty during initial route activation. A later poll may resolve it.
  }

  return {
    session_id_hash: sha256(route.sessionID),
    session_top_level: topLevel,
  }
}

function projectHash(context) {
  const directory = context.location?.directory
  if (typeof directory !== "string" || directory.length === 0 || directory.length > 4096) {
    return null
  }
  try {
    const canonical = realpathSync.native(directory)
    return sha256(canonical)
  } catch {
    return null
  }
}

export default Plugin.define({
  id: "panelink.v2-identity-fixture",
  setup(context) {
    const output = process.env.PANELINK_PROBE_RECORDS
    if (typeof output !== "string" || output.length === 0) return
    const control = process.env.PANELINK_PROBE_CONTROL

    const suppliedClientNonce = process.env.PANELINK_PROBE_CLIENT_NONCE
    const clientNonce = typeof suppliedClientNonce === "string"
      && HASH_RE.test(suppliedClientNonce)
      ? suppliedClientNonce
      : null
    const tuiInstance = randomBytes(16).toString("hex")
    const startedAt = performance.now()
    let sequence = 0
    let recordCount = 0
    let stopped = false
    let timer

    const stop = () => {
      if (stopped) return
      stopped = true
      if (timer !== undefined) clearInterval(timer)
    }

    const observe = (kind) => {
      if (stopped) return
      const now = performance.now()
      if (now - startedAt > MAX_RUN_MS || recordCount >= MAX_RECORDS) {
        stop()
        return
      }
      if (typeof control === "string" && control.length > 0) {
        try {
          const directive = readFileSync(control)
          if (directive.length === 5 && directive.equals(Buffer.from("pause", "utf8"))) return
          stop()
          return
        } catch (error) {
          if (error?.code !== "ENOENT") {
            stop()
            return
          }
        }
      }
      try {
        const route = context.ui.router.current()
        const session = sessionObservation(context, route)
        const record = {
          schema: 1,
          kind,
          tui_instance: tuiInstance,
          client_nonce: clientNonce,
          seq: sequence + 1,
          monotonic_ms: Math.floor(now),
          route_name: routeName(route),
          session_id_hash: session.session_id_hash,
          session_top_level: session.session_top_level,
          project_hash: projectHash(context),
        }
        const line = `${JSON.stringify(record)}\n`
        const bytes = Buffer.byteLength(line, "utf8")
        let existingBytes = 0
        try {
          existingBytes = statSync(output).size
        } catch (error) {
          if (error?.code !== "ENOENT") {
            stop()
            return
          }
        }
        if (existingBytes + bytes > MAX_BYTES) {
          stop()
          return
        }
        appendFileSync(output, line, { encoding: "utf8", flag: "a", mode: 0o600 })
        sequence += 1
        recordCount += 1
        if (recordCount >= MAX_RECORDS) stop()
      } catch {
        // Fail closed without logging exception text or any route data.
        stop()
      }
    }

    observe("plugin_loaded")
    if (stopped) return
    timer = setInterval(() => observe("poll"), POLL_MS)
    if (typeof timer?.unref === "function") timer.unref()
    return stop
  },
})
