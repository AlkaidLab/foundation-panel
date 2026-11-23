import assert from 'node:assert/strict'
import test from 'node:test'
import { createRenderer, nextTick, ref } from 'vue'

globalThis.localStorage ??= { getItem: () => null, setItem: () => {}, removeItem: () => {} }
globalThis.document ??= { documentElement: {} }
globalThis.window ??= {
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent() {},
  matchMedia: () => ({ addEventListener() {}, matches: false }),
  __TAURI_INTERNALS__: { transformCallback: () => 1, invoke: async () => ({}) },
}

const { emitUsbipErrorReport, formatUsbipFriendlyError, usbipErrorReportCode, useUsbTransport } = await import('./useUsbTransport.js')
const { usbipStatusChannel } = await import('./deviceStatusStore.js')
const { usbip } = await import('../tauri-adapter.js')
const { useDeviceComponentOperation } = await import('./useDeviceComponentOperation.js')
const { useVbCableComponent } = await import('./useVbCableComponent.js')

test('usbipErrorReportCode extracts structured codes and subcodes without user text', () => {
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-016: Windows driver policy blocked (exit code 1) [!!! Error 0xe0000247 in C:\\Users\\Alice\\file]'),
    'usbip-setup-016.0xe0000247',
  )
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-010: the USB/IP installer failed with exit code 1 [pnputil failed]'),
    'usbip-setup-010.exit-1',
  )
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-007: transport installation failed: USBIP-EXEC-006: hash mismatch'),
    'usbip-exec-006',
  )
  assert.equal(usbipErrorReportCode('unexpected failure with C:\\Users\\secret'), 'usbip-unknown')
})

test('formatUsbipFriendlyError unwraps USBIP-SETUP-007 to the underlying specific error code', () => {
  const usbTexts = {
    unknownError: 'fallback',
    errors: {
      'USBIP-SETUP-007': 'generic install failed',
      'USBIP-SETUP-015': 'reboot pending',
      'USBIP-EXEC-006': 'payload mismatch',
    },
  }
  assert.equal(
    formatUsbipFriendlyError('USBIP-SETUP-007: transport installation failed: USBIP-EXEC-006: missing', usbTexts),
    'payload mismatch',
  )
  assert.equal(
    formatUsbipFriendlyError('USBIP-SETUP-015: a Windows restart is pending', usbTexts),
    'reboot pending',
  )
  assert.equal(formatUsbipFriendlyError('USBIP-SETUP-007: raw error', usbTexts), 'generic install failed')
  assert.equal(
    formatUsbipFriendlyError('USBIP-SETUP-018: another driver operation', usbTexts, { operationBusy: 'driver busy' }),
    'driver busy',
  )
  assert.equal(formatUsbipFriendlyError('other', usbTexts), 'fallback')
})

test('emitUsbipErrorReport sends only component, code and severity', () => {
  const sent = []
  const code = emitUsbipErrorReport(
    'USBIP-SETUP-015: pending restart (exit code 3010) [C:\\Users\\bob\\secret]',
    (event) => sent.push(event),
  )
  assert.equal(code, 'usbip-setup-015.exit-3010')
  assert.deepEqual(sent, [{ component: 'usbip', code: 'usbip-setup-015.exit-3010', severity: 'error' }])
})

test('a status response arriving after unmount cannot start automatic maintenance', async (t) => {
  const originalFetch = usbipStatusChannel.fetch
  const originals = { cleanupTransport: usbip.cleanupTransport, installTransport: usbip.installTransport, removeStaleNodes: usbip.removeStaleNodes }
  let calls = 0
  for (const name of Object.keys(originals)) usbip[name] = async () => { calls += 1; return { success: true, data: {} } }
  t.after(() => { usbipStatusChannel.fetch = originalFetch; Object.assign(usbip, originals) })
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  for (const pending of [
    { cleanup_resume: true, cleanup_resume_op: 'cleanup', vhci_residual: true },
    { cleanup_resume: true, cleanup_resume_op: 'stale', vhci_residual: true, residual_kind: 'stale' },
    { install_resume: true, vhci_residual: false },
  ]) {
    let answer
    usbipStatusChannel.fetch = () => new Promise(resolve => { answer = resolve })
    const app = renderer.createApp({ setup() { useUsbTransport(); return () => null } })
    app.mount({})
    assert.equal(typeof answer, 'function')
    app.unmount()
    answer({ success: true, data: { supported: true, externally_managed: false, attached_devices: [], ...pending } })
    await new Promise(resolve => setImmediate(resolve))
    assert.equal(calls, 0)
  }
})

test('automatic maintenance queued before unmount does not start afterwards', async (t) => {
  const originalFetch = usbipStatusChannel.fetch
  const originalCleanup = usbip.cleanupTransport
  const originalQueue = globalThis.queueMicrotask
  const queued = []
  let calls = 0
  usbipStatusChannel.fetch = async () => ({ success: true, data: {
    supported: true, cleanup_resume: true, cleanup_resume_op: 'cleanup', externally_managed: false,
    vhci_residual: true, attached_devices: [],
  } })
  usbip.cleanupTransport = async () => { calls += 1; return { success: true, data: {} } }
  globalThis.queueMicrotask = callback => queued.push(callback)
  t.after(() => { usbipStatusChannel.fetch = originalFetch; usbip.cleanupTransport = originalCleanup; globalThis.queueMicrotask = originalQueue })
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  let transport
  const app = renderer.createApp({ setup() { transport = useUsbTransport(); return () => null } })
  app.mount({})
  await transport.refreshStatus({ force: true })
  assert.equal(queued.length, 1)
  app.unmount()
  for (const callback of queued) callback()
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(calls, 0)
})

test('automatic maintenance waiting for a refresh stops if its component unmounts', async (t) => {
  const originalFetch = usbipStatusChannel.fetch
  const originalCleanup = usbip.cleanupTransport
  const originalQueue = globalThis.queueMicrotask
  let calls = 0
  let queued = 0
  let app
  usbipStatusChannel.fetch = async () => ({ success: true, data: {
    supported: true, cleanup_resume: true, cleanup_resume_op: 'cleanup', externally_managed: false,
    vhci_residual: true, attached_devices: [],
  } })
  usbip.cleanupTransport = async () => { calls += 1; return { success: true, data: {} } }
  globalThis.queueMicrotask = callback => { queued += 1; callback(); app.unmount() }
  t.after(() => { usbipStatusChannel.fetch = originalFetch; usbip.cleanupTransport = originalCleanup; globalThis.queueMicrotask = originalQueue })
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  app = renderer.createApp({ setup() { useUsbTransport(); return () => null } })
  app.mount({})
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(queued, 1)
  assert.equal(calls, 0)
})

test('automatic USB/IP cleanup retries after another driver operation releases the guard', async (t) => {
  const originalFetch = usbipStatusChannel.fetch
  const originalCleanup = usbip.cleanupTransport
  let cleanupCalls = 0
  let firstAttempt
  let completed
  const first = new Promise(resolve => { firstAttempt = resolve })
  const done = new Promise(resolve => { completed = resolve })
  usbipStatusChannel.fetch = async () => ({ success: true, data: {
    supported: true, cleanup_resume: cleanupCalls < 2, cleanup_resume_op: 'cleanup', externally_managed: false,
    vhci_residual: true, attached_devices: [],
  } })
  usbip.cleanupTransport = async () => {
    cleanupCalls += 1
    if (cleanupCalls === 1) {
      firstAttempt()
      return { success: false, message: 'USBIP-SETUP-018: another driver operation' }
    }
    completed()
    return { success: true, data: { cleanup_resume: false, vhci_residual: false } }
  }
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  const app = renderer.createApp({ setup() { useUsbTransport(); return () => null } })
  app.mount({})
  t.after(() => { app.unmount(); usbipStatusChannel.fetch = originalFetch; usbip.cleanupTransport = originalCleanup })
  await first
  assert.equal(cleanupCalls, 1)
  let deadline
  t.after(() => clearTimeout(deadline))
  await Promise.race([done, new Promise((_, reject) => { deadline = setTimeout(() => reject(new Error('cleanup did not retry')), 15000) })])
  clearTimeout(deadline)
  assert.equal(cleanupCalls, 2)
})

test('a pending USB/IP cleanup resumes when a VB-CABLE confirmation is cancelled', async (t) => {
  const originalFetch = usbipStatusChannel.fetch
  const originalCleanup = usbip.cleanupTransport
  let cleanupCalls = 0
  let cleanupStarted
  const cleanup = new Promise(resolve => { cleanupStarted = resolve })
  usbipStatusChannel.fetch = async () => ({ success: true, data: {
    supported: true, cleanup_resume: true, cleanup_resume_op: 'cleanup', externally_managed: false,
    vhci_residual: true, attached_devices: [],
  } })
  usbip.cleanupTransport = async () => {
    cleanupCalls += 1
    cleanupStarted()
    return { success: true, data: { cleanup_resume: false, vhci_residual: false } }
  }

  let cancel
  const vb = useVbCableComponent({
    api: { check: async () => ({ success: true, data: { state: 'disabled', archive_ready: false } }) },
    text: ref({}), shared: useDeviceComponentOperation(),
    confirm: () => new Promise((_, reject) => { cancel = reject }), invalidate: () => {},
  })
  await vb.check()
  const confirmation = vb.changeDriver('enable')
  await nextTick()

  let transport
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  const app = renderer.createApp({ setup() { transport = useUsbTransport(); return () => null } })
  app.mount({})
  t.after(() => { app.unmount(); usbipStatusChannel.fetch = originalFetch; usbip.cleanupTransport = originalCleanup })
  await transport.refreshStatus({ force: true })
  assert.equal(cleanupCalls, 0)
  cancel(new Error('cancelled'))
  await confirmation
  await Promise.race([cleanup, new Promise((_, reject) => setTimeout(() => reject(new Error('cleanup did not resume')), 1000))])
  assert.equal(cleanupCalls, 1)
})

// ---- the one-click fix, through the real composable with the host and the driver stood in for ----
async function mountTransport(t, initial) {
  const { sunshine } = await import('../tauri-adapter.js')
  const { useDialogs } = await import('foundation-ui/dialog')
  const { useNotify } = await import('foundation-ui/notify')
  const { useI18n } = await import('../desktop/i18n/index.js')
  const world = { status: { supported: true, installed: true, version_valid: true, ready: false, externally_managed: false, vhci_residual: false, error_code: 'USBIP-EXEC-006', detail: 'USBIP-EXEC-006: the shared transport does not match the tested component combination', attached_devices: [], ...initial }, calls: [], sessions: [{ session_id: 1 }] }
  const saved = { fetch: usbipStatusChannel.fetch, cleanup: usbip.cleanupTransport, install: usbip.installTransport, replace: usbip.replaceLegacyTransport, restart: usbip.restartWindows, ask: sunshine.askActiveSessions, stop: sunshine.stopAllSessions }
  usbipStatusChannel.fetch = async () => ({ success: true, data: { ...world.status } })
  sunshine.askActiveSessions = async () => world.sessions
  sunshine.stopAllSessions = async () => { world.calls.push('stop-stream'); world.sessions = [] }
  usbip.cleanupTransport = async () => { world.calls.push('cleanup'); return world.cleanup ? world.cleanup() : { success: true, data: (world.status = { ...world.status, installed: false, error_code: '', detail: '' }) } }
  usbip.installTransport = async () => { world.calls.push('install'); return world.install ? world.install() : { success: true, data: (world.status = { ...world.status, installed: true, ready: true, error_code: '', detail: '' }) } }
  usbip.restartWindows = async () => { world.calls.push('restart-windows'); return { success: true } }
  t.after(() => {
    usbipStatusChannel.fetch = saved.fetch
    usbip.cleanupTransport = saved.cleanup
    usbip.installTransport = saved.install
    usbip.replaceLegacyTransport = saved.replace
    usbip.restartWindows = saved.restart
    sunshine.askActiveSessions = saved.ask
    sunshine.stopAllSessions = saved.stop
  })
  const renderer = createRenderer({
    patchProp() {}, insert() {}, remove() {}, createElement: () => ({}), createText: () => ({}),
    createComment: () => ({}), setText() {}, setElementText() {}, parentNode: () => null, nextSibling: () => null,
  })
  let transport
  const app = renderer.createApp({ setup() { transport = useUsbTransport(); return () => null } })
  app.mount({})
  let mounted = true
  const unmount = () => { if (mounted) { mounted = false; app.unmount() } }
  t.after(unmount)
  await transport.refreshStatus({ force: true })
  const until = async (read) => {
    for (let i = 0; i < 400; i += 1) { const value = read(); if (value) return value; await new Promise((resolve) => setTimeout(resolve, 5)) }
    throw new Error('timed out')
  }
  const dialogs = useDialogs()
  const notify = useNotify()
  const snack = () => notify.state.queue.find((item) => item.id === 'usbip-fix')
  const answer = async (action) => { const dialog = await until(() => dialogs.state.queue[0]); const asked = { ...dialog }; dialogs.settle(dialog.id, action); return asked }
  return { world, transport, unmount, until, snack, answer, texts: () => useI18n().t.value.deviceHub.usb, notify, dialogs }
}

test('the one-click fix ends the stream, cleans and installs in that order after one question, and reports at the bottom', async (t) => {
  const { world, transport, until, snack, answer, texts } = await mountTransport(t)
  assert.equal(transport.fixOffered.value, true, 'the notice of the transport carries the fix while the status reports the mismatch')
  const running = transport.fixTransport()
  const question = await answer('confirm')
  assert.ok(question.message.startsWith(texts().fixEndsStream), 'the question says the stream ends')
  assert.equal(question.title, texts().fixTitle)
  await running
  assert.deepEqual(world.calls, ['stop-stream', 'cleanup', 'install'])
  const done = await until(() => snack()?.timeout > 0 && snack())
  assert.ok(done.message.includes('USB/IP'), done.message)
  assert.equal(done.action, '')
  assert.equal(transport.fixing.value, false)
  assert.equal(transport.status.ready, true)
  assert.match(transport.fixRaw.value, /end-stream: ok\ncleanup: ok\ninstall: ok/)
})

test('a failed step shows its reason with the raw diagnostics one touch away, and the install is not attempted', async (t) => {
  const { world, transport, until, snack, answer, texts, dialogs } = await mountTransport(t)
  world.sessions = []
  world.cleanup = () => ({ success: false, message: 'USBIP-CLEAN-008: HostController busy (raw detail)' })
  const running = transport.fixTransport()
  assert.equal((await answer('confirm')).message.includes(texts().fixEndsStream), false, 'no stream, nothing about ending one')
  await running
  assert.deepEqual(world.calls, ['cleanup'])
  const failed = await until(() => snack()?.timeout > 0 && snack())
  assert.ok(failed.message.includes(texts().fixStepClean))
  assert.ok(failed.message.includes(texts().errors['USBIP-CLEAN-008']))
  assert.equal(failed.action, texts().diagnosticDetails)
  assert.match(transport.fixRaw.value, /cleanup: failed\nUSBIP-CLEAN-008: HostController busy \(raw detail\)/)
  failed.onAction()
  const raw = await answer('confirm')
  assert.equal(raw.title, texts().diagnosticDetails)
  assert.match(raw.message, /raw detail/)
  assert.equal(raw.cancel, false)
  assert.equal(dialogs.state.queue.length, 0)
})

test('declining the question leaves the system and the bottom message as they were', async (t) => {
  const { world, transport, snack, answer } = await mountTransport(t)
  const running = transport.fixTransport()
  await answer('cancel')
  await running
  assert.deepEqual(world.calls, [])
  await new Promise((resolve) => setTimeout(resolve, 20))
  assert.equal(snack(), undefined, 'the progress sentence that waited behind the question is gone')
  assert.equal(transport.fixing.value, false)
})

test('a restart that the cleanup or the install asks for is offered, and Windows restarts only after a second answer', async (t) => {
  const { world, transport, until, snack, answer, texts, notify } = await mountTransport(t)
  world.sessions = []
  world.install = () => ({ success: true, data: (world.status = { ...world.status, installed: true, ready: false, reboot_recommended: true, error_code: '', detail: '' }) })
  const running = transport.fixTransport()
  await answer('confirm')
  await running
  const offer = await until(() => snack()?.timeout > 0 && snack())
  assert.equal(offer.action, texts().fixRestartNow)
  assert.ok(Object.values(notify.state.systems).some((item) => item.actions?.[0]?.label === texts().fixRestartNow), 'the offer also stays in the status of the window')
  offer.onAction()
  assert.equal(world.calls.includes('restart-windows'), false, 'nothing restarts before the person says so')
  const second = await answer('confirm')
  assert.equal(second.message, texts().fixRestartConfirm)
  await until(() => world.calls.includes('restart-windows'))
})

test('only the version mismatch toast carries the fix, and not for an installation the person manages', async (t) => {
  const { transport, notify, until, texts } = await mountTransport(t)
  let message
  const toast = () => notify.state.queue.find((item) => item.message === message)
  message = transport.friendlyError('USBIP-EXEC-006: the shared transport does not match')
  transport.notifyError('USBIP-EXEC-006: the shared transport does not match')
  assert.equal((await until(toast)).action, texts().fixAction)
  notify.state.queue.length = 0
  message = transport.friendlyError('USBIP-EXEC-004: refused')
  transport.notifyError('USBIP-EXEC-004: refused')
  assert.equal((await until(toast)).action, '')
})

test('an installation the person manages gets no button and no change; the fix only tells them so', async (t) => {
  const { transport, notify, until, texts, dialogs } = await mountTransport(t, { externally_managed: true, replaceable: false })
  assert.equal(transport.fixOffered.value, false)
  const message = transport.friendlyError('USBIP-EXEC-006: the shared transport does not match')
  transport.notifyError('USBIP-EXEC-006: the shared transport does not match')
  assert.equal((await until(() => notify.state.queue.find((item) => item.message === message))).action, '')
  await transport.fixTransport()
  assert.equal(dialogs.state.queue.length, 0, 'no question: nothing will be changed')
  assert.ok((await until(() => notify.state.queue.find((item) => item.id === 'usbip-fix' && item.timeout > 0))).message === texts().externalDecision)
})

test('refreshStatus says how the read went: ok, failed, or disposed', async (t) => {
  const { transport, unmount } = await mountTransport(t)
  assert.equal(await transport.refreshStatus({ force: true }), 'ok')
  usbipStatusChannel.fetch = async () => ({ success: false, message: 'probe down' })
  assert.equal(await transport.refreshStatus({ force: true }), 'failed')
  usbipStatusChannel.fetch = async () => { throw new Error('bridge gone') }
  assert.equal(await transport.refreshStatus({ force: true }), 'failed')
  unmount()
  assert.equal(await transport.refreshStatus({ force: true }), 'disposed')
})

test('the fix does not start from a cached status when the read fails, and says why', async (t) => {
  const { world, transport, until, snack, texts, dialogs } = await mountTransport(t)
  usbipStatusChannel.fetch = async () => ({ success: false, message: 'USBIP-EXEC-007: probe down' })
  await transport.fixTransport()
  assert.equal(dialogs.state.queue.length, 0, 'no question is asked on a status that was not read')
  assert.deepEqual(world.calls, [], 'nothing was stopped, cleaned or installed')
  const failed = await until(() => snack()?.timeout > 0 && snack())
  assert.ok(failed.message.includes(texts().fixStepCheck))
  assert.ok(failed.message.includes(texts().errors['USBIP-EXEC-007']), 'the reason is what the probe said, translated')
  assert.equal(failed.action, texts().diagnosticDetails)
  assert.match(transport.fixRaw.value, /check: failed\nUSB\/IP status refresh failed/)
})

test('the fix does not start on a page that was closed', async (t) => {
  const { world, transport, unmount, dialogs } = await mountTransport(t)
  unmount()
  await transport.fixTransport()
  assert.equal(dialogs.state.queue.length, 0)
  assert.deepEqual(world.calls, [])
  assert.match(transport.fixRaw.value, /check: failed\nUSB\/IP status refresh was abandoned/)
})
