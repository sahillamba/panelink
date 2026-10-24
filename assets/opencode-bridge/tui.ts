import { Plugin } from "@opencode/plugin/tui"
import { createRecoveryController } from "./recovery.ts"
import { connect, type Socket } from "node:net"
import { lstatSync, realpathSync } from "node:fs"
import { basename, dirname, join } from "node:path"

const VERSION = 2
const HEADER = 24
const MAX_PAYLOAD = 4388
const MAX_FRAME = HEADER + MAX_PAYLOAD
const MAGIC = Buffer.from("PLBRSN02")
const PROVIDER_VERSION = "2.0.32"
const MAX_PROJECT_BYTES = 4096
const MAX_SESSION_BYTES = 128
const SOCKET_IDLE_MS = 10_000
const WRITE_STALL_MS = 500
const HELLO_MIN = 88

type Target = { generation: bigint; workspace: bigint; pane: bigint; session: bigint; revision: bigint }
type Grant = { target: Target; project: Buffer; version: string; instance: bigint; capability: Buffer }
type Selection = { scope: 1 | 2; tag: 1 | 2 | 3 | 4 | 5; source: 0 | 1; session: Buffer }

function fail(): never { throw new Error("bridge protocol failure") }

function u16(b: Buffer, at: number): number { return b.readUInt16BE(at) }
function u32(b: Buffer, at: number): number { return b.readUInt32BE(at) }
function u64(b: Buffer, at: number): bigint { return b.readBigUInt64BE(at) }

function parseGrant(msg: Buffer): Grant {
  if (msg.length < 32 + HELLO_MIN || msg.length > MAX_FRAME
      || msg.subarray(0, 8).toString("ascii") !== "PLBRDG01"
      || u16(msg, 8) !== 1 || u16(msg, 10) !== 1
      || msg.subarray(16, 32).some((v) => v !== 0)) fail()
  const len = u32(msg, 12)
  if (len !== msg.length - 32 || len < HELLO_MIN || len > 4248) fail()
  const b = msg.subarray(32)
  if (b.subarray(84, 88).some((v) => v !== 0)) fail()
  const versionLen = u16(b, 80)
  const projectLen = u16(b, 82)
  if (versionLen < 1 || versionLen > 64 || projectLen < 1 || projectLen > MAX_PROJECT_BYTES
      || 88 + versionLen + projectLen !== b.length) fail()
  const version = b.subarray(88, 88 + versionLen).toString("utf8")
  const project = Buffer.from(b.subarray(88 + versionLen))
  const target = {
    generation: u64(b, 0), workspace: u64(b, 8), pane: u64(b, 16),
    session: u64(b, 24), revision: u64(b, 32),
  }
  const instance = u64(b, 40)
  const capability = Buffer.from(b.subarray(48, 80))
  if (version !== PROVIDER_VERSION || instance === 0n || capability.every((v) => v === 0)
      || Object.values(target).some((v) => v === 0n) || project.includes(0)) fail()
  return { target, project, version, instance, capability }
}

function packet(kind: number, challenge: bigint, payload: Buffer): Buffer {
  if (payload.length > MAX_PAYLOAD) fail()
  const out = Buffer.alloc(HEADER + payload.length)
  MAGIC.copy(out, 0)
  out.writeUInt16BE(VERSION, 8)
  out.writeUInt16BE(kind, 10)
  out.writeUInt32BE(payload.length, 12)
  out.writeBigUInt64BE(challenge, 16)
  payload.copy(out, HEADER)
  return out
}

function v1Envelope(kind: number, body: Buffer): Buffer {
  const out = Buffer.alloc(32 + body.length)
  out.write("PLBRDG01", 0, "ascii")
  out.writeUInt16BE(1, 8)
  out.writeUInt16BE(kind, 10)
  out.writeUInt32BE(body.length, 12)
  body.copy(out, 32)
  return out
}

function welcomeInstance(payload: Buffer): bigint {
  if (payload.length !== 40 || payload.subarray(0, 8).toString("ascii") !== "PLBRDG01"
      || u16(payload, 8) !== 1 || u16(payload, 10) !== 2 || u32(payload, 12) !== 8
      || payload.subarray(16, 32).some((v) => v !== 0)) fail()
  return u64(payload, 32)
}

function decodeOuter(b: Buffer): { kind: number; challenge: bigint; payload: Buffer } {
  if (b.length < HEADER || !b.subarray(0, 8).equals(MAGIC) || u16(b, 8) !== VERSION) fail()
  const kind = u16(b, 10)
  const length = u32(b, 12)
  const challenge = u64(b, 16)
  if (length > MAX_PAYLOAD || b.length !== HEADER + length) fail()
  if (kind === 2 && (length !== 40 || challenge !== 0n)) fail()
  if (kind === 5 && challenge !== 0n) fail()
  if (kind === 3 && (length !== 0 || challenge === 0n)) fail()
  if ((kind === 6 || kind === 8) && (length === 0 || challenge === 0n)) fail()
  if (kind !== 2 && kind !== 3 && kind !== 5 && kind !== 6 && kind !== 8) fail()
  return { kind, challenge, payload: b.subarray(HEADER) }
}

function encodeObservation(grant: Grant, sequence: bigint, selection: Selection): Buffer {
  const version = Buffer.from(grant.version, "utf8")
  const project = grant.project
  const session = selection.session
  if (session.length > MAX_SESSION_BYTES) fail()
  const b = Buffer.alloc(68 + version.length + project.length + session.length)
  const targetValues = [grant.target.generation, grant.target.workspace, grant.target.pane,
    grant.target.session, grant.target.revision]
  targetValues.forEach((v, i) => b.writeBigUInt64BE(v, i * 8))
  b.writeBigUInt64BE(grant.instance, 40)
  b.writeBigUInt64BE(sequence, 48)
  b[56] = selection.scope
  b[57] = selection.tag
  b[58] = selection.source
  b.writeUInt16BE(version.length, 60)
  b.writeUInt16BE(project.length, 62)
  b.writeUInt16BE(session.length, 64)
  version.copy(b, 68)
  project.copy(b, 68 + version.length)
  session.copy(b, 68 + version.length + project.length)
  return b
}

function selected(context: any, grant: Grant): Selection {
  let actualProject: Buffer | undefined
  try {
    const dir = context.location?.directory
    if (typeof dir === "string" && dir.length > 0 && dir.length <= MAX_PROJECT_BYTES) {
      actualProject = Buffer.from(realpathSync.native(dir), "utf8")
    }
  } catch { /* Project identity is unavailable. */ }
  if (!actualProject || !actualProject.equals(grant.project)) {
    return { scope: 1, tag: 4, source: 0, session: Buffer.alloc(0) }
  }

  try {
    const route = context.ui?.router?.current?.()
    if (route?.type === "home") return { scope: 1, tag: 2, source: 0, session: Buffer.alloc(0) }
    if (route?.type !== "session" || typeof route.sessionID !== "string") {
      return { scope: 1, tag: 4, source: 0, session: Buffer.alloc(0) }
    }
    const session = Buffer.from(route.sessionID, "utf8")
    if (session.length === 0 || session.length > MAX_SESSION_BYTES
        || !/^[A-Za-z0-9_.-]+$/.test(route.sessionID)) {
      return { scope: 1, tag: 4, source: 0, session: Buffer.alloc(0) }
    }
    const cached = context.data?.session?.get(route.sessionID)
    if (!cached || typeof cached !== "object") {
      return { scope: 1, tag: 4, source: 0, session: Buffer.alloc(0) }
    }
    const cachedDirectory = cached.location?.directory
    if (cached.id !== route.sessionID || typeof cachedDirectory !== "string"
        || cachedDirectory.length === 0 || cachedDirectory.length > MAX_PROJECT_BYTES
        || !Buffer.from(realpathSync.native(cachedDirectory), "utf8").equals(grant.project)) {
      return { scope: 1, tag: 4, source: 0, session: Buffer.alloc(0) }
    }
    if (cached.parentID != null) return { scope: 2, tag: 3, source: 0, session: Buffer.alloc(0) }
    return { scope: 1, tag: 1, source: 1, session }
  } catch {
    return { scope: 1, tag: 5, source: 0, session: Buffer.alloc(0) }
  }
}

function privateSocketPath(path: string): string {
  if (!path.startsWith("/") || path.length > 4096 || path.includes("\0")) fail()
  const parent = realpathSync.native(dirname(path))
  const leaf = basename(path)
  if (leaf.length === 0 || leaf === "." || leaf === "..") fail()
  const canonical = join(parent, leaf)
  if (canonical !== path) fail()
  const stat = lstatSync(canonical)
  if (!stat.isSocket() || (stat.mode & 0o077) !== 0 || stat.uid !== process.getuid?.()) fail()
  return canonical
}

function setupBridge(context: any): (() => void) | undefined {
  const env = process.env
  const socketEnv = env.PANELINK_BRIDGE_SOCKET
  const helloEnv = env.PANELINK_BRIDGE_HELLO
  delete env.PANELINK_BRIDGE_SOCKET
  delete env.PANELINK_BRIDGE_HELLO
  if (typeof socketEnv !== "string") return

  let socket: Socket | undefined
  let stopped = false
  let provisioned = false
  let welcomed = false
  let nextChallenge = 1n
  let sequence = 0n
  let lastChallenge = 0n
  let pending = Buffer.alloc(0)
  let requestBusy = false
  let timeout: ReturnType<typeof setTimeout> | undefined
  let grant: Grant | undefined
  let recovery: ReturnType<typeof createRecoveryController> | undefined
  const path = privateSocketPath(socketEnv)
  if (typeof helloEnv === "string" && !/^(?:[0-9a-f]{2}){120,4280}$/i.test(helloEnv)) fail()
  const expectedHello = typeof helloEnv === "string" ? Buffer.from(helloEnv, "hex") : undefined

  const stop = () => {
    if (stopped) return
    stopped = true
    if (timeout) clearTimeout(timeout)
    expectedHello?.fill(0)
    recovery?.dispose()
    socket?.destroy()
  }
  const armIdle = () => {
    if (timeout) clearTimeout(timeout)
    timeout = setTimeout(stop, SOCKET_IDLE_MS)
    timeout.unref?.()
  }
  const write = (data: Buffer, onDone?: () => void) => {
    if (!socket || stopped) return
    const stall = setTimeout(stop, WRITE_STALL_MS)
    stall.unref?.()
    const output = Buffer.from(data)
    try {
      socket.write(output, (error?: Error | null) => {
        output.fill(0)
        clearTimeout(stall)
        if (error) stop()
        else onDone?.()
      })
    } catch { output.fill(0); clearTimeout(stall); stop() }
  }
  const consume = () => {
    while (!stopped && pending.length >= HEADER) {
      if (requestBusy) return stop()
      if (!pending.subarray(0, 8).equals(MAGIC) || u16(pending, 8) !== VERSION) return stop()
      const length = u32(pending, 12)
      if (length > MAX_PAYLOAD || HEADER + length > MAX_FRAME) return stop()
      if (pending.length < HEADER + length) return
      const raw = pending.subarray(0, HEADER + length)
      pending = pending.subarray(raw.length)
      let message: ReturnType<typeof decodeOuter>
      try { message = decodeOuter(raw) } catch { return stop() }
      if (!provisioned) {
        if (message.kind !== 5) return stop()
        let provisionedGrant: Grant
        const provisionedBytes = Buffer.from(message.payload)
        try { provisionedGrant = parseGrant(provisionedBytes) }
        catch { provisionedBytes.fill(0); return stop() }
        if (expectedHello && !provisionedBytes.equals(expectedHello)) {
          provisionedBytes.fill(0)
          message.payload.fill(0)
          return stop()
        }
        const helloResponse = packet(1, 0n, provisionedBytes)
        provisionedBytes.fill(0)
        message.payload.fill(0)
        grant = provisionedGrant
        grant.capability.fill(0)
        provisioned = true
        try { recovery = createRecoveryController(context, grant.project, grant.version) }
        catch { return stop() }
        if (expectedHello) expectedHello.fill(0)
        write(helloResponse)
        helloResponse.fill(0)
        armIdle()
        continue
      }
      if (!welcomed) {
        if (!grant || message.kind !== 2 || welcomeInstance(message.payload) !== grant.instance) return stop()
        welcomed = true
        armIdle()
        continue
      }
      if ((message.kind !== 3 && message.kind !== 6 && message.kind !== 8)
          || requestBusy || message.challenge !== nextChallenge
          || message.challenge <= lastChallenge) return stop()
      requestBusy = true
      lastChallenge = message.challenge
      nextChallenge = message.challenge + 1n
      if (nextChallenge > 0xffffffffffffffffn) return stop()
      if (message.kind === 3) {
        if (sequence === 0xffffffffffffffffn) return stop()
        sequence += 1n
        let observation: Buffer
        if (!grant) return stop()
        try { observation = encodeObservation(grant, sequence, selected(context, grant)) }
        catch { return stop() }
        const response = packet(4, message.challenge, v1Envelope(3, observation))
        observation.fill(0)
        write(response, () => {
          requestBusy = false
          armIdle()
        })
        response.fill(0)
      } else {
        if (!recovery) return stop()
        const kind = message.kind
        const replyKind = kind === 6 ? 7 : 9
        const payload = Buffer.from(message.payload)
        message.payload.fill(0)
        void recovery.handle(kind, payload).then((reply) => {
          payload.fill(0)
          if (stopped) { reply.fill(0); return }
          const response = packet(replyKind, message.challenge, reply)
          reply.fill(0)
          write(response, () => {
            requestBusy = false
            armIdle()
          })
          response.fill(0)
        }).catch(() => {
          payload.fill(0)
          stop()
        })
      }
      if (pending.length > 0) return stop()
    }
  }

  try {
    socket = connect({ path })
    socket.on("connect", () => {
      if (stopped) return
      armIdle()
    })
    socket.on("data", (chunk: Buffer) => {
      if (stopped) return
      if (pending.length + chunk.length > MAX_FRAME * 2) return stop()
      pending = pending.length ? Buffer.concat([pending, chunk]) : chunk
      armIdle()
      consume()
    })
    socket.on("error", stop)
    socket.on("end", stop)
    socket.on("close", stop)
  } catch { stop() }
  if (typeof context.cleanup === "function") context.cleanup(stop)
  else if (typeof context.onDispose === "function") context.onDispose(stop)
  return stop
}

export default Plugin.define({
  id: "panelink.opencode-bridge",
  setup(context) { return setupBridge(context) },
})
