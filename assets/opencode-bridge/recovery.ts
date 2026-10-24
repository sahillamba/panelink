import { realpathSync } from "node:fs"

const SUPPORTED_VERSION = "2.0.32"
const MAX_ID_BYTES = 128
const MAX_PROJECT_BYTES = 4096
const VALIDATE_HEADER = 8
const NAVIGATE_HEADER = 28
const TICKET_BYTES = 16
// Leave time inside the host's absolute 300 ms framed-I/O deadline.
const DEADLINE_MS = 250

export type RecoveryKind = 6 | 8

type ValidationStatus = 1 | 2 | 3 | 4 | 5 | 6
type NavigationStatus = 1 | 2

const ValidationTag = {
  Verified: 1,
  Missing: 2,
  ForeignProject: 3,
  Child: 4,
  Unsupported: 5,
  Uncertain: 6,
} as const

const NavigationTag = {
  ActionIssued: 1,
  NotIssued: 2,
} as const

const DEADLINE = "deadline" as const
const DISPOSED = "disposed" as const

function protocolFailure(): never {
  throw new Error("recovery protocol failure")
}

function validId(id: string): boolean {
  const bytes = Buffer.from(id, "utf8")
  return bytes.length > 0 && bytes.length <= MAX_ID_BYTES && /^[A-Za-z0-9_.-]+$/.test(id)
}

function canonicalDirectory(value: unknown): Buffer | undefined {
  if (typeof value !== "string" || value.length === 0 || Buffer.byteLength(value, "utf8") > MAX_PROJECT_BYTES) {
    return undefined
  }
  try {
    const path = Buffer.from(realpathSync.native(value), "utf8")
    return path.length > 0 && path.length <= MAX_PROJECT_BYTES ? path : undefined
  } catch {
    return undefined
  }
}

function readSessionId(payload: Buffer): string {
  if (!Buffer.isBuffer(payload) || payload.length < 3) protocolFailure()
  const idLength = payload.readUInt16BE(0)
  if (idLength === 0 || idLength > MAX_ID_BYTES || payload.length !== 2 + idLength) protocolFailure()
  const bytes = payload.subarray(2)
  if (bytes.some((byte) => byte > 0x7f)) protocolFailure()
  const id = bytes.toString("ascii")
  if (!validId(id)) protocolFailure()
  return id
}

function validationReply(status: ValidationStatus, id?: string, project?: Buffer): Buffer {
  const idBytes = id === undefined ? Buffer.alloc(0) : Buffer.from(id, "ascii")
  const projectBytes = project === undefined ? Buffer.alloc(0) : Buffer.from(project)
  if (status === ValidationTag.Verified) {
    if (!id || !projectBytes.length || idBytes.length > MAX_ID_BYTES || projectBytes.length > MAX_PROJECT_BYTES) protocolFailure()
  } else if (idBytes.length !== 0 || projectBytes.length !== 0) {
    protocolFailure()
  }
  const body = Buffer.alloc(VALIDATE_HEADER + idBytes.length + projectBytes.length)
  body[0] = status
  body.writeUInt16BE(idBytes.length, 2)
  body.writeUInt16BE(projectBytes.length, 4)
  idBytes.copy(body, VALIDATE_HEADER)
  projectBytes.copy(body, VALIDATE_HEADER + idBytes.length)
  return body
}

function readNavigation(payload: Buffer): { id: string; revision: bigint; ticket: Buffer } {
  if (!Buffer.isBuffer(payload) || payload.length < NAVIGATE_HEADER + 1 || payload[26] !== 0 || payload[27] !== 0) protocolFailure()
  const revision = payload.readBigUInt64BE(0)
  const ticket = Buffer.from(payload.subarray(8, 24))
  const idLength = payload.readUInt16BE(24)
  if (revision === 0n || ticket.length !== TICKET_BYTES || ticket.every((byte) => byte === 0)
      || idLength === 0 || idLength > MAX_ID_BYTES || payload.length !== NAVIGATE_HEADER + idLength) protocolFailure()
  const bytes = payload.subarray(NAVIGATE_HEADER)
  if (bytes.some((byte) => byte > 0x7f)) protocolFailure()
  const id = bytes.toString("ascii")
  if (!validId(id)) protocolFailure()
  return { id, revision, ticket }
}

function navigationReply(status: NavigationStatus, request: { id: string; revision: bigint; ticket: Buffer }): Buffer {
  const id = Buffer.from(request.id, "ascii")
  const out = Buffer.alloc(NAVIGATE_HEADER + id.length)
  out[0] = status
  out.writeUInt16BE(id.length, 2)
  out.writeBigUInt64BE(request.revision, 4)
  request.ticket.copy(out, 12)
  id.copy(out, NAVIGATE_HEADER)
  return out
}

function typedSessionNotFound(error: unknown, depth = 0): boolean {
  if (error === null || typeof error !== "object") return false
  // In the pinned OpenCode bundle, the session.get endpoint declares its
  // error as SessionNotFoundError, whose generated schema is annotated HTTP
  // 404. httpApiStatus is schema metadata, not a guaranteed instance field.
  const value = error as {
    _tag?: unknown
    name?: unknown
    httpApiStatus?: unknown
    cause?: unknown
    failure?: unknown
  }
  if (value._tag === "SessionNotFoundError") {
    return value.httpApiStatus === undefined || value.httpApiStatus === 404
  }
  if (depth >= 2) return false
  // Effect's Promise boundary can wrap one typed failure in FiberFailure /
  // Cause.Fail containers. Traverse only those tagged wrappers.
  if (value._tag === "FiberFailure" || value.name === "FiberFailure") {
    return typedSessionNotFound(value.cause, depth + 1)
  }
  if (value._tag === "Fail") return typedSessionNotFound(value.failure, depth + 1)
  if (value._tag === "Sequential" || value._tag === "Parallel") {
    return false // multiple causes are ambiguous and remain unavailable
  }
  return false
}

function resultSessionNotFound(result: unknown): boolean {
  if (result === null || typeof result !== "object") return false
  const value = result as {
    error?: unknown
    response?: { status?: unknown } | null
  }
  // Some generated SDK client configurations return { error, response }.
  // Only the SDK response object for this exact session.get call is evidence
  // of an HTTP 404; error.status or arbitrary statusCode fields are not.
  return value.error !== undefined && value.error !== null
    && value.response !== null && typeof value.response === "object"
    && value.response.status === 404
}

type BoundedResult<T> = { value: T } | { failure: typeof DEADLINE | typeof DISPOSED } | { error: unknown }

function createRecoveryController(
  context: any,
  project: Buffer,
  providerVersion: string,
): { handle(kind: RecoveryKind, payload: Buffer): Promise<Buffer>; dispose(): void } {
  if (!Buffer.isBuffer(project) || project.length === 0 || project.length > MAX_PROJECT_BYTES || project.includes(0)) {
    throw new Error("recovery project scope is invalid")
  }
  const expectedProject = Buffer.from(project)
  let disposed = false
  let busy = false
  let navigationAttempted = false
  let validatedSessionId: string | undefined
  const activeCancels = new Set<() => void>()

  const withBounded = async <T>(operation: Promise<T>, deadline: number): Promise<BoundedResult<T>> => {
    let cancel!: () => void
    let timer: ReturnType<typeof setTimeout> | undefined
    const remaining = Math.max(0, deadline - Date.now())
    if (disposed) return { failure: DISPOSED }
    if (remaining === 0) return { failure: DEADLINE }
    const cancelled = new Promise<BoundedResult<T>>((resolve) => {
      cancel = () => resolve({ failure: DISPOSED })
      activeCancels.add(cancel)
      timer = setTimeout(() => resolve({ failure: DEADLINE }), remaining)
    })
    const result = await Promise.race([
      operation.then((value) => ({ value }) as BoundedResult<T>, (error) => ({ error }) as BoundedResult<T>),
      cancelled,
    ])
    if (timer) clearTimeout(timer)
    activeCancels.delete(cancel)
    return result
  }

  const versionStatus = (): ValidationStatus | undefined => {
    if (providerVersion !== SUPPORTED_VERSION || context?.app?.version !== SUPPORTED_VERSION) return ValidationTag.Unsupported
    return undefined
  }

  const lookup = async (sessionId: string, deadline: number): Promise<ValidationStatus> => {
    const unsupported = versionStatus()
    if (unsupported) return unsupported
    if (disposed) return ValidationTag.Uncertain
    const currentProject = canonicalDirectory(context?.location?.directory)
    if (!currentProject || !currentProject.equals(expectedProject)) return ValidationTag.ForeignProject

    let session: any
    const lookupResult = await withBounded(
      Promise.resolve().then(() => context?.client?.session?.get?.({ sessionID: sessionId })),
      deadline,
    )
    if ("failure" in lookupResult) return ValidationTag.Uncertain
    if ("error" in lookupResult) {
      return typedSessionNotFound(lookupResult.error)
        ? ValidationTag.Missing : ValidationTag.Uncertain
    }
    const value = lookupResult.value
    if (resultSessionNotFound(value)) return ValidationTag.Missing
    if (value && typeof value === "object" && "error" in value) {
      const result = value as { data?: unknown; error?: unknown }
      if (result.error !== undefined && result.error !== null) return ValidationTag.Uncertain
      session = result.data
    } else {
      session = value
    }
    if (!session || typeof session !== "object" || session.id !== sessionId) return ValidationTag.Uncertain
    // Session.Info omits parentID for native roots; forks use a separate origin.
    if (session.parentID != null) {
      return typeof session.parentID === "string" && validId(session.parentID)
        ? ValidationTag.Child : ValidationTag.Uncertain
    }
    const sessionProject = canonicalDirectory(session.location?.directory)
    if (!sessionProject) return ValidationTag.Uncertain
    if (!sessionProject.equals(expectedProject)) return ValidationTag.ForeignProject
    return ValidationTag.Verified
  }

  const validate = async (sessionId: string): Promise<Buffer> => {
    validatedSessionId = undefined
    const deadline = Date.now() + DEADLINE_MS
    const status = await lookup(sessionId, deadline)
    if (disposed) return validationReply(ValidationTag.Uncertain)
    if (status !== ValidationTag.Verified) return validationReply(status)
    validatedSessionId = sessionId
    return validationReply(ValidationTag.Verified, sessionId, expectedProject)
  }

  const navigate = async (request: { id: string; revision: bigint; ticket: Buffer }): Promise<Buffer> => {
    const notIssued = () => navigationReply(NavigationTag.NotIssued, request)
    if (navigationAttempted || disposed) return notIssued()
    navigationAttempted = true
    if (validatedSessionId !== request.id) return notIssued()
    const deadline = Date.now() + DEADLINE_MS
    const status = await lookup(request.id, deadline)
    if (disposed || status !== ValidationTag.Verified) return notIssued()
    if (Date.now() >= deadline) return notIssued()
    try {
      const router = context?.ui?.router
      if (typeof router?.navigate !== "function") return notIssued()
      const routePromise = Promise.resolve().then(() => {
        if (disposed || Date.now() >= deadline) return false
        return router.navigate({ type: "session", sessionID: request.id })
      })
      const routeResult = await withBounded(routePromise, deadline)
      if (disposed || "failure" in routeResult) return notIssued()
      if ("error" in routeResult || routeResult.value === false) return notIssued()
      return navigationReply(NavigationTag.ActionIssued, request)
    } catch {
      return notIssued()
    }
  }

  const handle = async (kind: RecoveryKind, payload: Buffer): Promise<Buffer> => {
    if (disposed || busy) protocolFailure()
    busy = true
    try {
      if (kind === 6) {
        const id = readSessionId(payload)
        return await validate(id)
      }
      if (kind === 8) {
        const request = readNavigation(payload)
        return await navigate(request)
      }
      return protocolFailure()
    } finally {
      busy = false
    }
  }

  const dispose = () => {
    if (disposed) return
    disposed = true
    validatedSessionId = undefined
    for (const cancel of activeCancels) cancel()
    activeCancels.clear()
  }

  return { handle, dispose }
}

export { createRecoveryController }
