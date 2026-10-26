import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


RECOVERY_MODULE = Path(__file__).resolve().parents[1] / "assets/opencode-bridge/recovery.ts"

NODE_DRIVER = r"""
import assert from 'node:assert/strict'
import { pathToFileURL } from 'node:url'
import { realpathSync } from 'node:fs'

const { createRecoveryController } = await import(pathToFileURL(process.env.PANELINK_RECOVERY_MODULE))
const project = Buffer.from(realpathSync.native(process.env.TEST_PROJECT), 'utf8')
const foreign = Buffer.from(realpathSync.native(process.env.TEST_FOREIGN), 'utf8')
const ascii = (s) => Buffer.from(s, 'ascii')
const validate = (id) => Buffer.concat([Buffer.from([0, Buffer.byteLength(id)]), ascii(id)])
const navigate = (id, revision = 8n, ticket = Buffer.alloc(16, 7)) => {
  const b = Buffer.alloc(28 + Buffer.byteLength(id))
  b.writeBigUInt64BE(revision, 0)
  ticket.copy(b, 8)
  b.writeUInt16BE(Buffer.byteLength(id), 24)
  ascii(id).copy(b, 28)
  return b
}
const resultStatus = (b) => b[0]
const makeInfo = (id, directory = process.env.TEST_PROJECT, parentID = null, extra = {}) => ({
  id, parentID, location: { directory }, ...extra,
})
const makeContext = (get, extra = {}) => ({
  app: { version: '2.0.32' },
  location: extra.location ?? { directory: process.env.TEST_PROJECT },
  client: { session: { get } },
  ui: { router: { navigate: (...args) => { extra.navigations?.push(args); } } },
})
const create = (context) => createRecoveryController(context, project, '2.0.32')

async function validationCases() {
  let calls = 0
  const root = create(makeContext(async ({sessionID}) => {
    calls += 1
    return makeInfo(sessionID, process.env.TEST_PROJECT, null, { fork: { sessionID: 'parent', boundary: 'msg_1' } })
  }))
  const verified = await root.handle(6, validate('ses_root_fork'))
  assert.equal(resultStatus(verified), 1)
  assert.equal(verified.readUInt16BE(2), 13)
  assert.equal(verified.subarray(8, 21).toString('ascii'), 'ses_root_fork')
  assert.deepEqual(verified.subarray(21), project)
  assert.equal(calls, 1)
  root.dispose()

  const omittedParent = create(makeContext(async ({sessionID}) => ({
    id: sessionID, location: { directory: process.env.TEST_PROJECT },
  })))
  assert.equal(resultStatus(await omittedParent.handle(6, validate('ses_root_omitted'))), 1)
  omittedParent.dispose()

  const malformedParent = create(makeContext(async ({sessionID}) => makeInfo(sessionID, process.env.TEST_PROJECT, false)))
  assert.equal(resultStatus(await malformedParent.handle(6, validate('ses_malformed_parent'))), 6)
  malformedParent.dispose()

  const missing = create(makeContext(async () => {
    throw { _tag: 'SessionNotFoundError', sessionID: 'ses_private' }
  }))
  assert.equal(resultStatus(await missing.handle(6, validate('ses_missing'))), 2)
  missing.dispose()

  const wrappedMissing = create(makeContext(async () => {
    throw { _tag: 'FiberFailure', cause: { _tag: 'SessionNotFoundError', sessionID: 'ses_private' } }
  }))
  assert.equal(resultStatus(await wrappedMissing.handle(6, validate('ses_wrapped_missing'))), 2)
  wrappedMissing.dispose()

  const sdkResponseMissing = create(makeContext(async () => ({
    error: { _tag: 'SessionNotFoundError' }, response: { status: 404 },
  })))
  assert.equal(resultStatus(await sdkResponseMissing.handle(6, validate('ses_sdk_response_missing'))), 2)
  sdkResponseMissing.dispose()

  const missingWithoutResponseStatus = create(makeContext(async () => ({
    error: { _tag: 'SessionNotFoundError' }, response: { status: 500 },
  })))
  assert.equal(resultStatus(await missingWithoutResponseStatus.handle(6, validate('ses_bad_sdk_status'))), 6)
  missingWithoutResponseStatus.dispose()

  const genericStatus = create(makeContext(async () => { throw { status: 404 } }))
  assert.equal(resultStatus(await genericStatus.handle(6, validate('ses_http_status'))), 6)
  genericStatus.dispose()
  const genericStatusCode = create(makeContext(async () => { throw { statusCode: 404 } }))
  assert.equal(resultStatus(await genericStatusCode.handle(6, validate('ses_status_code'))), 6)
  genericStatusCode.dispose()
  const wrongTag = create(makeContext(async () => {
    throw { _tag: 'NotFound', httpApiStatus: 404 }
  }))
  assert.equal(resultStatus(await wrongTag.handle(6, validate('ses_wrong_tag'))), 6)
  wrongTag.dispose()
  const wrongStatus = create(makeContext(async () => {
    throw { _tag: 'SessionNotFoundError', httpApiStatus: 500 }
  }))
  assert.equal(resultStatus(await wrongStatus.handle(6, validate('ses_wrong_status'))), 6)
  wrongStatus.dispose()

  const foreignSession = create(makeContext(async ({sessionID}) => makeInfo(sessionID, process.env.TEST_FOREIGN)))
  assert.equal(resultStatus(await foreignSession.handle(6, validate('ses_foreign'))), 3)
  foreignSession.dispose()

  const child = create(makeContext(async ({sessionID}) => makeInfo(sessionID, process.env.TEST_PROJECT, 'ses_parent')))
  assert.equal(resultStatus(await child.handle(6, validate('ses_child'))), 4)
  child.dispose()

  let wrongProjectCalls = 0
  const wrongProject = create(makeContext(async () => { wrongProjectCalls += 1; return makeInfo('ses_x') }, {
    location: { directory: process.env.TEST_FOREIGN },
  }))
  assert.equal(resultStatus(await wrongProject.handle(6, validate('ses_x'))), 3)
  assert.equal(wrongProjectCalls, 0)
  wrongProject.dispose()

  let unsupportedCalls = 0
  const unsupportedContext = makeContext(async () => { unsupportedCalls += 1; return makeInfo('ses_x') })
  unsupportedContext.app.version = '2.0.31'
  const unsupported = create(unsupportedContext)
  assert.equal(resultStatus(await unsupported.handle(6, validate('ses_x'))), 5)
  assert.equal(unsupportedCalls, 0)
  unsupported.dispose()

  const mismatch = create(makeContext(async () => makeInfo('ses_other')))
  assert.equal(resultStatus(await mismatch.handle(6, validate('ses_requested'))), 6)
  mismatch.dispose()
}

async function navigationCases() {
  let gets = 0
  const navigations = []
  const context = makeContext(async ({sessionID}) => {
    gets += 1
    return makeInfo(sessionID)
  }, { navigations })
  const controller = create(context)
  const validation = await controller.handle(6, validate('ses_exact'))
  assert.equal(resultStatus(validation), 1)
  const navBody = navigate('ses_exact', 44n, Buffer.alloc(16, 9))
  const nav = await controller.handle(8, navBody)
  assert.equal(resultStatus(nav), 1)
  assert.equal(nav.readUInt16BE(2), 9)
  assert.equal(nav.readBigUInt64BE(4), 44n)
  assert.deepEqual(nav.subarray(12, 28), Buffer.alloc(16, 9))
  assert.equal(nav.subarray(28).toString('ascii'), 'ses_exact')
  assert.deepEqual(navigations, [[{ type: 'session', sessionID: 'ses_exact' }]])
  assert.equal(gets, 2, 'navigation must revalidate exact ID through SDK')
  assert.equal(resultStatus(await controller.handle(8, navBody)), 2)
  assert.equal(navigations.length, 1)
  assert.equal(gets, 2, 'one-shot controller must not repeat lookup/navigation')
  controller.dispose()

  let deletedGets = 0
  let deletedNavigations = 0
  const deletedContext = makeContext(async ({sessionID}) => {
    deletedGets += 1
    if (deletedGets === 1) return makeInfo(sessionID)
    throw { statusCode: 404 }
  })
  deletedContext.ui.router.navigate = () => { deletedNavigations += 1 }
  const deleted = create(deletedContext)
  await deleted.handle(6, validate('ses_deleted'))
  assert.equal(resultStatus(await deleted.handle(8, navigate('ses_deleted'))), 2)
  assert.equal(deletedNavigations, 0)
  assert.equal(resultStatus(await deleted.handle(8, navigate('ses_deleted'))), 2)
  assert.equal(deletedGets, 2)
  deleted.dispose()

  const noPrior = create(makeContext(async () => { throw new Error('must not lookup') }))
  assert.equal(resultStatus(await noPrior.handle(8, navigate('ses_unvalidated'))), 2)
  noPrior.dispose()
}

async function timeoutAndDisposalCases() {
  let resolveLookup
  let calls = 0
  let navigations = 0
  const context = makeContext(() => {
    calls += 1
    if (calls === 1) return Promise.resolve(makeInfo('ses_slow'))
    return new Promise((resolve) => { resolveLookup = resolve })
  })
  context.ui.router.navigate = () => { navigations += 1 }
  const controller = create(context)
  await controller.handle(6, validate('ses_slow'))
  const started = Date.now()
  assert.equal(resultStatus(await controller.handle(8, navigate('ses_slow'))), 2)
  assert.ok(Date.now() - started >= 200 && Date.now() - started < 800)
  resolveLookup(makeInfo('ses_slow'))
  await new Promise((resolve) => setTimeout(resolve, 30))
  assert.equal(navigations, 0, 'late SDK result must not authorize late navigation')
  assert.equal(resultStatus(await controller.handle(8, navigate('ses_slow'))), 2)
  assert.equal(navigations, 0)
  controller.dispose()

  let resolveDisposed
  const disposable = create(makeContext(() => new Promise((resolve) => { resolveDisposed = resolve })))
  const pending = disposable.handle(6, validate('ses_dispose'))
  await new Promise((resolve) => setTimeout(resolve, 20))
  disposable.dispose()
  assert.equal(resultStatus(await pending), 6)
  resolveDisposed(makeInfo('ses_dispose'))
  await new Promise((resolve) => setTimeout(resolve, 30))

  let lookupNumber = 0
  let resolveNavigationCheck
  const delayedNavigationCalls = []
  const navigating = create(makeContext(({sessionID}) => {
    lookupNumber += 1
    if (lookupNumber === 1) return Promise.resolve(makeInfo(sessionID))
    return new Promise((resolve) => { resolveNavigationCheck = resolve })
  }, { navigations: delayedNavigationCalls }))
  await navigating.handle(6, validate('ses_dispose_nav'))
  const pendingNavigation = navigating.handle(8, navigate('ses_dispose_nav'))
  while (lookupNumber < 2) await new Promise((resolve) => setImmediate(resolve))
  navigating.dispose()
  assert.equal(resultStatus(await pendingNavigation), 2)
  resolveNavigationCheck(makeInfo('ses_dispose_nav'))
  await new Promise((resolve) => setTimeout(resolve, 30))
  assert.equal(delayedNavigationCalls.length, 0)
}

async function malformedCases() {
  let calls = 0
  const controller = create(makeContext(async () => { calls += 1; return makeInfo('ses_ok') }))
  await assert.rejects(controller.handle(7, Buffer.alloc(0)))
  await assert.rejects(controller.handle(6, Buffer.from([0, 0])))
  await assert.rejects(controller.handle(6, Buffer.from([0, 4, 0xff, 0xff, 0xff, 0xff])))
  const badNavigation = navigate('ses_ok')
  badNavigation[27] = 1
  await assert.rejects(controller.handle(8, badNavigation))
  const zeroRevision = navigate('ses_ok', 0n)
  await assert.rejects(controller.handle(8, zeroRevision))
  const zeroTicket = navigate('ses_ok', 1n, Buffer.alloc(16))
  await assert.rejects(controller.handle(8, zeroTicket))
  assert.equal(calls, 0)
  controller.dispose()
}

await validationCases()
await navigationCases()
await timeoutAndDisposalCases()
await malformedCases()
process.stdout.write(JSON.stringify({ ok: true, externalNetwork: 0, prompts: 0, models: 0 }))
"""


class OpenCodeBridgeRecoveryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.node = shutil.which("node")

    def test_exact_validation_navigation_timeout_disposal_and_malformed_inputs(self):
        if self.node is None:
            self.skipTest("Node.js is unavailable for the plugin companion fixture")
        with tempfile.TemporaryDirectory(prefix="panelink-opencode-recovery-") as temp:
            root = Path(temp).resolve()
            project = root / "project"
            foreign = root / "foreign"
            project.mkdir()
            foreign.mkdir()
            env = os.environ.copy()
            env.update({
                "PANELINK_RECOVERY_MODULE": str(RECOVERY_MODULE),
                "TEST_PROJECT": str(project),
                "TEST_FOREIGN": str(foreign),
            })
            result = subprocess.run(
                [self.node, "--no-warnings", "--experimental-strip-types", "--input-type=module", "-e", NODE_DRIVER],
                cwd=root,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=5,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr[:2000])
            self.assertEqual(result.stdout, '{"ok":true,"externalNetwork":0,"prompts":0,"models":0}')


if __name__ == "__main__":
    unittest.main()
