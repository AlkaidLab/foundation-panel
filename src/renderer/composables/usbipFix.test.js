import assert from 'node:assert/strict'
import test from 'node:test'
import 'vue' // loaded before the shim below, as the other tests of this folder do: Vue must not see the fake document
import { DESKTOP_LOCALES } from '../desktop/i18n/merge.js'
import {
  describeFixResult,
  fixConfirmText,
  fixProgressText,
  initialFix,
  planUsbipFix,
  reduceFix,
  runUsbipFix,
  STREAM_STOP_WAIT_MS,
  usbipFixOffered,
  usbipFixOfferedByStatus,
  usbipFixRawText,
} from './usbipFix.js'
import { en } from '../desktop/i18n/en.js'
import { zh } from '../desktop/i18n/zh.js'
import { withUsbipOwnership } from '../desktop/i18n/usbipOwnership.js'

globalThis.localStorage ??= { getItem: () => null, setItem: () => {}, removeItem: () => {} }
globalThis.document ??= { documentElement: {} }
globalThis.window ??= {
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent() {},
  matchMedia: () => ({ addEventListener() {}, matches: false }),
  __TAURI_INTERNALS__: { transformCallback: () => 1, invoke: async () => ({}) },
}
const { formatUsbipFriendlyError } = await import('./useUsbTransport.js')

const VERSION = '0.9.8.1'
const base = { supported: true, installed: true, version_valid: true, ready: true, externally_managed: false, replaceable: false, vhci_residual: false, attached_devices: [] }
const mismatch = { ...base, ready: false, error_code: 'USBIP-EXEC-006', detail: 'USBIP-EXEC-006: the shared transport does not match the tested component combination' }

test('the fix is offered for the mismatch itself, not for an install that fails with it inside another code', () => {
  assert.equal(usbipFixOffered('USBIP-EXEC-006: the shared transport does not match'), true)
  assert.equal(usbipFixOffered('USBIP-EXEC-006: usbip.exe reported an ABI mismatch'), true)
  assert.equal(usbipFixOffered('USBIP-SETUP-007: transport installation failed: USBIP-EXEC-006: hash mismatch'), false)
  assert.equal(usbipFixOffered('USBIP-EXEC-004: the exporter refused'), false)
  assert.equal(usbipFixOffered(''), false)
  assert.equal(usbipFixOfferedByStatus(mismatch), true)
  assert.equal(usbipFixOfferedByStatus({ ...mismatch, error_code: 'USBIP-EXEC-007' }), false)
  assert.equal(usbipFixOfferedByStatus({ ...mismatch, supported: false }), false)
})

test('the plan cleans and installs what the panel manages, replaces only what the host allows, and leaves the rest alone', () => {
  const cases = [
    ['mismatched tool or driver', mismatch, 'clean-install', ['cleanup', 'install']],
    ['version of another build', { ...base, ready: false, version_valid: false }, 'clean-install', ['cleanup', 'install']],
    ['controller leftovers without a working install', { ...base, installed: false, ready: false, vhci_residual: true }, 'clean-install', ['cleanup', 'install']],
    ['nothing installed', { ...base, installed: false, ready: false }, 'install', ['install']],
    ['everything matches', base, 'healthy', []],
    ['a full usbip-win2 the host may replace', { ...base, ready: false, externally_managed: true, replaceable: true }, 'replace', ['replace']],
  ]
  for (const [name, status, kind, steps] of cases) {
    const plan = planUsbipFix(status)
    assert.equal(plan.kind, kind, name)
    assert.deepEqual(plan.steps, steps, name)
  }
  assert.equal(planUsbipFix({ ...base, externally_managed: true, replaceable: true, ready: false }).replacesExternal, true)
  assert.equal(planUsbipFix(mismatch).replacesExternal, undefined)
})

test('an installation of the person that the host does not offer to replace is never touched', () => {
  const plan = planUsbipFix({ ...mismatch, externally_managed: true, replaceable: false })
  assert.deepEqual([plan.kind, plan.reason, plan.steps], ['blocked', 'external', []])
  // even a leftover flag does not turn it into a cleanup: the cleanup path is for what the panel installed
  assert.equal(planUsbipFix({ ...mismatch, externally_managed: true, vhci_residual: true }).kind, 'blocked')
})

test('attached devices, an unsupported system, a pending restart and an unexplained state stop the plan with their reason', () => {
  const device = { port: 1 }
  assert.deepEqual(planUsbipFix({ ...mismatch, attached_devices: [device] }), { kind: 'blocked', reason: 'attached', steps: [] })
  assert.equal(planUsbipFix({ ...base, ready: false, externally_managed: true, replaceable: true, attached_devices: [device] }).reason, 'attached')
  assert.equal(planUsbipFix({ ...base, supported: false }).reason, 'unsupported')
  assert.deepEqual(planUsbipFix({ ...mismatch, cleanup_pending_restart: true }), { kind: 'reboot', reason: 'cleanup', steps: [] })
  assert.equal(planUsbipFix({ ...mismatch, install_pending_restart: true }).reason, 'install')
  assert.equal(planUsbipFix({ ...base, ready: false }).reason, 'nothing')
})

test('the reducer walks check, plan, question, steps and an end, and keeps the log of every step', () => {
  let state = reduceFix(initialFix(), { type: 'check' })
  assert.equal(state.phase, 'checking')
  state = reduceFix(state, { type: 'plan', plan: { kind: 'clean-install', steps: ['cleanup', 'install'] } })
  assert.deepEqual([state.phase, state.steps], ['confirming', ['cleanup', 'install']])
  state = reduceFix(state, { type: 'confirmed' })
  assert.deepEqual([state.phase, state.index], ['running', 0])
  state = reduceFix(state, { type: 'step-done', step: 'cleanup', raw: 'cleaned' })
  assert.equal(state.index, 1)
  state = reduceFix(state, { type: 'step-failed', step: 'install', error: 'USBIP-SETUP-010: failed' })
  assert.deepEqual([state.phase, state.outcome, state.failedStep], ['finished', 'failed', 'install'])
  assert.deepEqual(state.log.map((entry) => [entry.step, entry.ok]), [['cleanup', true], ['install', false]])
  assert.equal(reduceFix(initialFix(), { type: 'unknown' }).phase, 'idle')
  for (const [plan, outcome] of [
    [{ kind: 'healthy', steps: [] }, 'healthy'],
    [{ kind: 'blocked', reason: 'external', steps: [] }, 'blocked-external'],
    [{ kind: 'reboot', reason: 'cleanup', steps: [] }, 'reboot-cleanup'],
  ]) assert.equal(reduceFix(reduceFix(initialFix(), { type: 'check' }), { type: 'plan', plan }).outcome, outcome)
  assert.equal(reduceFix(state, { type: 'declined' }).outcome, 'cancelled')
})

/** Fake world: records the order of the calls and lets each test choose the answers. */
function world(overrides = {}) {
  const calls = []
  let clock = 0
  let sessions = overrides.sessions === undefined ? [{ session_id: 1 }] : overrides.sessions
  const answer = (name, fallback) => async () => {
    calls.push(name)
    const value = overrides[name]
    if (value instanceof Error) throw value
    return typeof value === 'function' ? value() : value ?? fallback
  }
  const deps = {
    refresh: async () => { calls.push('refresh'); return overrides.status || mismatch },
    sessions: async () => {
      calls.push('sessions')
      if (sessions instanceof Error) throw sessions
      const now = sessions
      if (overrides.streamEndsAfter !== undefined && calls.filter((c) => c === 'sessions').length > overrides.streamEndsAfter) sessions = []
      return now
    },
    confirm: async (question) => { calls.push('confirm'); world.questions.push(question); return overrides.confirm ?? true },
    endStream: answer('endStream', { ok: true }),
    cleanup: answer('cleanup', { ok: true, data: { vhci_residual: false } }),
    install: answer('install', { ok: true, data: { ready: true } }),
    replace: answer('replace', { ok: true, data: { ready: true } }),
    sleep: async (ms) => { clock += ms },
    now: () => clock,
  }
  world.questions = []
  return { deps, calls }
}
world.questions = []

test('a live stream is ended first, then the shared transport is cleaned and the bundled one installed, after one question', async () => {
  const { deps, calls } = world({ streamEndsAfter: 2 })
  const seen = []
  const state = await runUsbipFix(deps, (next) => seen.push(`${next.phase}:${next.index}`))
  assert.equal(state.outcome, 'done')
  assert.deepEqual(calls.filter((c) => c !== 'sessions'), ['refresh', 'confirm', 'endStream', 'cleanup', 'install'])
  assert.equal(calls.filter((c) => c === 'confirm').length, 1, 'the person is asked once')
  assert.ok(calls.indexOf('endStream') < calls.indexOf('cleanup'), 'the stream ends before the transport changes')
  assert.equal(world.questions.at(-1).streaming, true)
  assert.deepEqual(state.steps, ['end-stream', 'cleanup', 'install'])
  assert.deepEqual(seen.slice(0, 3), ['checking:0', 'confirming:0', 'running:0'])
  assert.deepEqual(state.log.map((entry) => entry.step), ['end-stream', 'cleanup', 'install'])
})

test('without a stream nothing is ended and the question does not mention one', async () => {
  const { deps, calls } = world({ sessions: [] })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'done')
  assert.equal(calls.includes('endStream'), false)
  assert.equal(world.questions.at(-1).streaming, false)
  assert.deepEqual(state.steps, ['cleanup', 'install'])
  const unknown = world({ sessions: new Error('host unreachable') })
  assert.deepEqual((await runUsbipFix(unknown.deps)).steps, ['cleanup', 'install'], 'a host that cannot be asked has no stream to end; the native step checks again')
})

test('declining the question changes nothing', async () => {
  const { deps, calls } = world({ confirm: false })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'cancelled')
  assert.deepEqual(calls.filter((c) => c !== 'sessions'), ['refresh', 'confirm'])
})

test('a stream that does not end in time fails the end-stream step with the stream code, before anything is cleaned', async () => {
  const { deps, calls } = world({ streamEndsAfter: 10_000 })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'failed')
  assert.equal(state.failedStep, 'end-stream')
  assert.match(state.error, /^USBIP-SETUP-006:/)
  assert.equal(calls.includes('cleanup'), false)
  assert.ok(calls.filter((c) => c === 'sessions').length > 3)
  assert.ok(STREAM_STOP_WAIT_MS >= 10_000)
})

test('a host that refuses to end the stream fails that step with what it said', async () => {
  const { deps, calls } = world({ endStream: { ok: false, error: 'HTTP 500' } })
  const state = await runUsbipFix(deps)
  assert.deepEqual([state.outcome, state.failedStep, state.error], ['failed', 'end-stream', 'HTTP 500'])
  assert.equal(calls.includes('cleanup'), false)
  const thrown = world({ endStream: new Error('connection reset') })
  assert.equal((await runUsbipFix(thrown.deps)).failedStep, 'end-stream')
})

test('a failed cleanup stops the fix before the install and keeps the raw answer', async () => {
  const { deps, calls } = world({ sessions: [], cleanup: { ok: false, error: 'USBIP-CLEAN-008: the host controller is in use' } })
  const state = await runUsbipFix(deps)
  assert.deepEqual([state.outcome, state.failedStep], ['failed', 'cleanup'])
  assert.equal(calls.includes('install'), false)
  assert.match(usbipFixRawText(state, { detail: 'the status detail' }), /cleanup: failed\nUSBIP-CLEAN-008: the host controller is in use\nthe status detail/)
})

test('a cleanup that waits for a restart ends the fix with the restart offer, not with an install that cannot work', async () => {
  const { deps, calls } = world({ sessions: [], cleanup: { ok: true, data: { cleanup_pending_restart: true } } })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'reboot-cleanup')
  assert.equal(calls.includes('install'), false)
  const residual = world({ sessions: [], cleanup: { ok: true, data: { vhci_residual: true } } })
  const left = await runUsbipFix(residual.deps)
  assert.deepEqual([left.outcome, left.failedStep], ['failed', 'cleanup'])
  assert.match(left.error, /^USBIP-CLEAN-002:/)
})

test('an install that needs a restart, or is not ready, is told apart from a finished one', async () => {
  const restart = await runUsbipFix(world({ sessions: [], install: { ok: true, data: { ready: false, reboot_recommended: true } } }).deps)
  assert.equal(restart.outcome, 'reboot-install')
  const pending = await runUsbipFix(world({ sessions: [], install: { ok: true, data: { ready: false, install_pending_restart: true } } }).deps)
  assert.equal(pending.outcome, 'reboot-install')
  const notReady = await runUsbipFix(world({ sessions: [], install: { ok: true, data: { ready: false, detail: 'USBIP-EXEC-006: still differs' } } }).deps)
  assert.deepEqual([notReady.outcome, notReady.failedStep, notReady.error], ['failed', 'install', 'USBIP-EXEC-006: still differs'])
})

test('a full usbip-win2 of the person is replaced only after the question, by the replacement and by nothing else', async () => {
  const status = { ...base, ready: false, externally_managed: true, replaceable: true }
  const { deps, calls } = world({ status, sessions: [] })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'done')
  assert.deepEqual(calls.filter((c) => c !== 'sessions'), ['refresh', 'confirm', 'replace'])
  assert.equal(world.questions.at(-1).plan.replacesExternal, true)
  const declined = world({ status, sessions: [], confirm: false })
  await runUsbipFix(declined.deps)
  assert.equal(declined.calls.includes('replace'), false)
})

test('an installation the person manages and the host will not replace is left alone and no question is asked', async () => {
  const { deps, calls } = world({ status: { ...mismatch, externally_managed: true, replaceable: false } })
  const state = await runUsbipFix(deps)
  assert.equal(state.outcome, 'blocked-external')
  assert.deepEqual(calls, ['refresh'])
})

test('a healthy status, attached devices and a pending restart end before any question', async () => {
  for (const [status, outcome] of [
    [base, 'healthy'],
    [{ ...mismatch, attached_devices: [{ port: 2 }] }, 'blocked-attached'],
    [{ ...mismatch, cleanup_pending_restart: true }, 'reboot-cleanup'],
  ]) {
    const { deps, calls } = world({ status })
    assert.equal((await runUsbipFix(deps)).outcome, outcome)
    assert.equal(calls.includes('confirm'), false)
  }
})

test('a step that did not start because another driver operation runs, or that throws, fails with its reason', async () => {
  const busy = await runUsbipFix(world({ sessions: [], cleanup: { ok: false, skipped: true } }).deps)
  assert.deepEqual([busy.outcome, busy.failedStep], ['failed', 'cleanup'])
  assert.match(busy.error, /^USBIP-SETUP-018:/)
  const thrown = await runUsbipFix(world({ sessions: [], install: new Error('boom') }).deps)
  assert.deepEqual([thrown.outcome, thrown.failedStep], ['failed', 'install'])
  const unreadable = await runUsbipFix({ ...world().deps, refresh: async () => { throw new Error('probe down') } })
  assert.deepEqual([unreadable.outcome, unreadable.failedStep], ['failed', 'check'])
})

// ---- the words ----
// the tables as the window reads them: the language files with the ownership messages laid over them
const usb = { en: withUsbipOwnership(en, 'en').deviceHub.usb, zh: withUsbipOwnership(zh, 'zh').deviceHub.usb }
const friendly = (texts) => (raw) => formatUsbipFriendlyError(raw, texts)

test('error mapping: a failed step names the step and gives the translated reason of its code, in each language', () => {
  for (const [name, texts] of Object.entries(usb)) {
    const text = (state) => describeFixResult({ ...initialFix(), ...state }, texts, friendly(texts), VERSION)
    const cleanup = text({ outcome: 'failed', failedStep: 'cleanup', error: 'USBIP-CLEAN-008: busy' })
    assert.equal(cleanup.tone, 'error', name)
    assert.equal(cleanup.action, 'diagnostics', name)
    assert.ok(cleanup.text.includes(texts.fixStepClean), name)
    assert.ok(cleanup.text.includes(texts.errors['USBIP-CLEAN-008']), name)
    const install = text({ outcome: 'failed', failedStep: 'install', error: 'USBIP-SETUP-007: transport installation failed: USBIP-SETUP-016: blocked' })
    assert.ok(install.text.includes(texts.errors['USBIP-SETUP-016']), `${name}: the underlying code of an install is the reason`)
    assert.ok(install.text.includes(VERSION), `${name}: the install step names the version`)
    const stream = text({ outcome: 'failed', failedStep: 'end-stream', error: 'HTTP 500: stop failed' })
    assert.ok(stream.text.includes(texts.fixStopFailed), `${name}: a host answer without a code says the stream could not be ended`)
    const timeout = text({ outcome: 'failed', failedStep: 'end-stream', error: 'USBIP-SETUP-006: finish the active Sunshine stream' })
    assert.ok(timeout.text.includes(texts.errors['USBIP-SETUP-006']), name)
    const restart = text({ outcome: 'failed', failedStep: 'cleanup', error: 'USBIP-RESTART-001: no shutdown.exe' })
    assert.ok(restart.text.includes(texts.errors['USBIP-RESTART-001']), name)
  }
})

test('results: done and healthy are plain, a restart offers the restart, a refusal explains itself, a decline says nothing', () => {
  const texts = usb.en
  const text = (outcome) => describeFixResult({ ...initialFix(), outcome }, texts, friendly(texts), VERSION)
  assert.deepEqual(text('done'), { tone: 'success', text: `USB/IP ${VERSION} is ready to use.`, action: '' })
  assert.equal(text('healthy').tone, 'info')
  assert.equal(text('reboot-cleanup').action, 'restart')
  assert.equal(text('reboot-install').action, 'restart')
  assert.ok(text('reboot-install').text.includes(VERSION))
  assert.equal(text('blocked-external').text, texts.externalDecision)
  assert.equal(text('blocked-attached').text, texts.fixBlockedAttached)
  assert.equal(text('blocked-nothing').action, 'diagnostics')
  assert.equal(text('cancelled').text, '')
})

test('the question and the progress sentence say what will happen', () => {
  const texts = usb.zh
  const clean = fixConfirmText({ kind: 'clean-install', streaming: false }, texts, VERSION)
  assert.ok(clean.includes(VERSION) && !clean.includes(texts.fixEndsStream))
  const streaming = fixConfirmText({ kind: 'clean-install', streaming: true }, texts, VERSION)
  assert.ok(streaming.startsWith(texts.fixEndsStream), 'the stream that will end is named first')
  assert.equal(fixConfirmText({ kind: 'replace', replacesExternal: true, streaming: false }, texts, VERSION), texts.replaceConfirm)
  const state = { ...initialFix(), steps: ['end-stream', 'cleanup', 'install'], index: 1, phase: 'running' }
  assert.equal(fixProgressText(state, texts, VERSION), '修复中 2/3：清理共享传输…')
  assert.equal(fixProgressText({ ...state, index: 2 }, texts, VERSION), `修复中 3/3：安装 USB/IP ${VERSION}…`)
  assert.equal(fixProgressText({ ...initialFix() }, texts, VERSION), '')
})

test('every language has the words of the fix, with the placeholders its sentences need', async () => {
  const placeholders = {
    fixConfirm: ['{version}'],
    fixProgress: ['{n}', '{total}', '{step}'],
    fixStepInstall: ['{version}'],
    fixDone: ['{version}'],
    fixRebootInstall: ['{version}'],
    fixFailed: ['{step}', '{reason}'],
  }
  const keys = ['fixAction', 'fixTitle', 'fixConfirm', 'fixEndsStream', 'fixStart', 'fixProgress', 'fixStepCheck', 'fixStepStream', 'fixStepClean', 'fixStepInstall', 'fixStepReplace', 'fixDone', 'fixHealthy', 'fixFailed', 'fixRebootCleanup', 'fixRebootInstall', 'fixBlockedAttached', 'fixNothing', 'fixStopFailed', 'fixRestartNow', 'fixRestartConfirm']
  assert.equal(DESKTOP_LOCALES.length, 20)
  for (const locale of DESKTOP_LOCALES) {
    const table = locale === 'en' ? en : locale === 'zh' ? zh : (await import(`../desktop/i18n/${locale}.js`))[locale]
    const texts = table.deviceHub.usb
    for (const key of keys) {
      assert.equal(typeof texts[key], 'string', `${locale}.${key}`)
      assert.ok(texts[key].length > 0, `${locale}.${key}`)
    }
    for (const [key, list] of Object.entries(placeholders)) for (const token of list) assert.ok(texts[key].includes(token), `${locale}.${key} keeps ${token}`)
    assert.ok(texts.errors['USBIP-RESTART-001'], `${locale} explains a restart that did not start`)
    assert.ok(!texts.fixConfirm.includes('0.9.8.1'), `${locale}: the version comes from the pinned contract, not from the text`)
  }
})
