// One-click fix for a USB/IP tool or driver that is not the version pinned for this Sunshine (USBIP-EXEC-006).
//
// No driver logic lives here. The steps are the ones the component page already runs: end the stream through the host, clean the shared
// transport, install the bundled USB/IP, or, for a full usbip-win2 the person installed, the replacement they confirm. This module only decides
// which of them apply, drives them in order, and turns the outcome into a state the window can show. The calls are injected, so every
// transition is testable without a driver.
import { usbipCanReplaceLegacy, usbipTransportNeedsCleanup } from './deviceHubStatus.js'

export const USBIP_FIX_CODE = 'USBIP-EXEC-006'

/** How long the host gets to drop the stream after it was asked to stop, and how often it is asked. */
export const STREAM_STOP_WAIT_MS = 20000
export const STREAM_STOP_POLL_MS = 500

const codesOf = (error) => String(error || '').match(/USBIP-[A-Z]+-\d{3}/g) || []

/**
 * True for a failed call that reports the version mismatch itself. A code that only appears inside another one (an install that fails with
 * it under USBIP-SETUP-007) is the fix failing, not a reason to start it again.
 */
export function usbipFixOffered(error) {
  return codesOf(error)[0] === USBIP_FIX_CODE
}

/** True while the transport status itself reports the mismatch. */
export function usbipFixOfferedByStatus(status = {}) {
  return Boolean(status.supported !== false && status.error_code === USBIP_FIX_CODE)
}

/**
 * What the fix would do for this status. `kind`:
 *   clean-install  the transport the panel installed (or its leftovers) is cleaned, then the bundled one is installed
 *   install        nothing is installed: the bundled one is installed
 *   replace        a full usbip-win2 of the person: only the replacement the host allows, after the person confirmed it
 *   healthy        nothing differs any more
 *   reboot         Windows has to restart before anything else can change
 *   blocked        the fix cannot start; `reason` says why (unsupported, external, attached, nothing)
 */
export function planUsbipFix(status = {}) {
  if (status.supported === false) return { kind: 'blocked', reason: 'unsupported', steps: [] }
  if (status.cleanup_pending_restart || status.install_pending_restart) {
    return { kind: 'reboot', reason: status.install_pending_restart ? 'install' : 'cleanup', steps: [] }
  }
  const attached = (status.attached_devices || []).length > 0
  if (status.externally_managed) {
    if (!usbipCanReplaceLegacy(status)) return { kind: 'blocked', reason: 'external', steps: [] }
    if (attached) return { kind: 'blocked', reason: 'attached', steps: [] }
    return { kind: 'replace', steps: ['replace'], replacesExternal: true }
  }
  const broken = usbipTransportNeedsCleanup(status)
  if (!broken && status.ready) return { kind: 'healthy', steps: [] }
  if (broken) {
    if (attached) return { kind: 'blocked', reason: 'attached', steps: [] }
    return { kind: 'clean-install', steps: ['cleanup', 'install'] }
  }
  if (!status.installed) return { kind: 'install', steps: ['install'] }
  return { kind: 'blocked', reason: 'nothing', steps: [] }
}

export const initialFix = () => ({
  phase: 'idle',
  plan: null,
  steps: [],
  index: 0,
  outcome: '',
  failedStep: '',
  error: '',
  log: [],
})

const FINISHED = 'finished'

/**
 * The states of one run: idle, checking (the status is read again), confirming (the one question), running (a step at a time), finished.
 * `outcome` of a finished run: done, healthy, cancelled, blocked-<reason>, reboot-cleanup, reboot-install, failed.
 */
export function reduceFix(state, event) {
  switch (event.type) {
    case 'check':
      return { ...initialFix(), phase: 'checking' }
    case 'plan': {
      const plan = event.plan
      if (plan.kind === 'healthy') return { ...state, plan, phase: FINISHED, outcome: 'healthy' }
      if (plan.kind === 'blocked') return { ...state, plan, phase: FINISHED, outcome: `blocked-${plan.reason}` }
      if (plan.kind === 'reboot') return { ...state, plan, phase: FINISHED, outcome: `reboot-${plan.reason}` }
      return { ...state, plan, steps: plan.steps, phase: 'confirming' }
    }
    case 'declined':
      return { ...state, phase: FINISHED, outcome: 'cancelled' }
    case 'confirmed':
      return { ...state, phase: 'running', index: 0 }
    case 'step-done':
      return { ...state, index: state.index + 1, log: [...state.log, { step: event.step, ok: true, raw: event.raw || '' }] }
    case 'step-failed':
      return {
        ...state,
        phase: FINISHED,
        outcome: 'failed',
        failedStep: event.step,
        error: String(event.error || ''),
        log: [...state.log, { step: event.step, ok: false, raw: String(event.error || '') }],
      }
    case 'reboot':
      return { ...state, phase: FINISHED, outcome: `reboot-${event.kind}`, index: state.steps.length }
    case 'done':
      return { ...state, phase: FINISHED, outcome: 'done', index: state.steps.length }
    default:
      return state
  }
}

const rawOf = (status = {}) => [status.detail, status.last_install_error, status.last_cleanup_error].filter(Boolean).join('\n')

/**
 * Runs the fix. `deps`:
 *   refresh()              reads the transport status again and resolves it
 *   sessions()             the live streams, or null when the host cannot be asked
 *   confirm({ plan, streaming })   the one question; resolves true to go on
 *   endStream()            asks the host to end the stream: { ok, error? }
 *   cleanup() / install() / replace()   the steps of the component page: { ok, data?, error?, skipped? }
 *   sleep(ms), now()       injectable clock
 * Resolves the finished state. Nothing here throws: a step that throws is a failed step.
 */
export async function runUsbipFix(deps, onState = () => {}) {
  let state = initialFix()
  const apply = (event) => {
    state = reduceFix(state, event)
    onState(state)
    return state
  }
  const fail = (step, error) => apply({ type: 'step-failed', step, error })
  const guarded = async (call, fallback) => {
    try {
      return await call()
    } catch (error) {
      return { ok: false, error: String(error || fallback) }
    }
  }
  apply({ type: 'check' })
  let status
  try {
    status = await deps.refresh()
  } catch (error) {
    return fail('check', error?.message || error)
  }
  let plan = planUsbipFix(status)
  let sessions = null
  if (plan.steps.length) {
    try { sessions = await deps.sessions() } catch { sessions = null }
    const streaming = Array.isArray(sessions) && sessions.length > 0
    plan = { ...plan, streaming, steps: streaming ? ['end-stream', ...plan.steps] : plan.steps }
  }
  apply({ type: 'plan', plan })
  if (state.phase !== 'confirming') return state
  let accepted = false
  try { accepted = await deps.confirm({ plan, streaming: Boolean(plan.streaming) }) } catch { accepted = false }
  if (!accepted) return apply({ type: 'declined' })
  apply({ type: 'confirmed' })

  for (const step of plan.steps) {
    if (step === 'end-stream') {
      const stopped = await guarded(() => deps.endStream(), 'the host did not accept the request')
      if (!stopped?.ok) return fail(step, stopped?.error || 'the host did not accept the request')
      // The stream counts as ended only when the host answers with an empty list. An answer that cannot be read (null) is not one: the
      // wait goes on, and at its limit the fix stops instead of changing the transport under a stream that may still run.
      const started = deps.now()
      let answered = false
      for (;;) {
        let answer = null
        try { answer = await deps.sessions() } catch { answer = null }
        if (Array.isArray(answer) && answer.length === 0) break
        answered = Array.isArray(answer)
        if (deps.now() - started >= STREAM_STOP_WAIT_MS) {
          return fail(step, answered
            ? 'USBIP-SETUP-006: finish the active Sunshine stream before changing the transport'
            : 'USBIP-SETUP-006: unable to verify that the Sunshine stream ended: the host did not answer')
        }
        await deps.sleep(STREAM_STOP_POLL_MS)
      }
      apply({ type: 'step-done', step })
      continue
    }
    const run = step === 'cleanup' ? deps.cleanup : step === 'replace' ? deps.replace : deps.install
    const result = await guarded(() => run(), 'the step did not finish')
    if (result?.skipped) return fail(step, 'USBIP-SETUP-018: another driver operation is in progress')
    if (!result?.ok) return fail(step, result?.error || 'the step did not finish')
    const data = result.data || {}
    if (step === 'cleanup') {
      if (data.cleanup_pending_restart) {
        apply({ type: 'step-done', step, raw: rawOf(data) })
        return apply({ type: 'reboot', kind: 'cleanup' })
      }
      if (data.vhci_residual) return fail(step, 'USBIP-CLEAN-002: the USB/IP host controller is still there after the cleanup')
      apply({ type: 'step-done', step, raw: rawOf(data) })
      continue
    }
    // install and replace: ready, or Windows has to restart first
    if (data.install_pending_restart) {
      apply({ type: 'step-done', step, raw: rawOf(data) })
      return apply({ type: 'reboot', kind: 'install' })
    }
    if (!data.ready) {
      if (data.reboot_recommended) {
        apply({ type: 'step-done', step, raw: rawOf(data) })
        return apply({ type: 'reboot', kind: 'install' })
      }
      return fail(step, data.detail || data.last_install_error || 'USBIP-SETUP-008: USB/IP is installed but not ready')
    }
    apply({ type: 'step-done', step, raw: rawOf(data) })
  }
  return apply({ type: 'done' })
}

/** The raw diagnostics of a run: what each step answered, as the host and the driver said it. */
export function usbipFixRawText(state, status = {}) {
  const lines = (state.log || []).map((entry) => `${entry.step}: ${entry.ok ? 'ok' : 'failed'}${entry.raw ? `\n${entry.raw}` : ''}`)
  const rest = rawOf(status)
  if (rest) lines.push(rest)
  // the transport status repeats what a step already said: every line once
  const seen = new Set()
  return lines.join('\n').split('\n').filter((line) => !seen.has(line) && seen.add(line)).join('\n')
}

/**
 * The words of a finished run: { tone, text, action }. `friendly` maps a raw error to its translated message; `texts` is deviceHub.usb.
 * `action` names what the person can do next from the same message: view the raw diagnostics, or restart Windows.
 */
export function describeFixResult(state, texts, friendly, version = '') {
  const step = (id) => (id === 'end-stream' ? texts.fixStepStream : id === 'cleanup' ? texts.fixStepClean : id === 'replace' ? texts.fixStepReplace : id === 'install' ? fill(texts.fixStepInstall, { version }) : texts.fixStepCheck)
  switch (state.outcome) {
    case 'done': return { tone: 'success', text: fill(texts.fixDone, { version }), action: '' }
    case 'healthy': return { tone: 'info', text: texts.fixHealthy, action: '' }
    case 'cancelled': return { tone: 'info', text: '', action: '' }
    case 'reboot-cleanup': return { tone: 'warning', text: texts.fixRebootCleanup, action: 'restart' }
    case 'reboot-install': return { tone: 'warning', text: fill(texts.fixRebootInstall, { version }), action: 'restart' }
    case 'blocked-external': return { tone: 'warning', text: texts.externalDecision, action: '' }
    case 'blocked-attached': return { tone: 'warning', text: texts.fixBlockedAttached, action: '' }
    case 'blocked-unsupported': return { tone: 'warning', text: texts.transportUnsupported, action: '' }
    case 'blocked-nothing': return { tone: 'warning', text: texts.fixNothing, action: 'diagnostics' }
    case 'failed': {
      const coded = codesOf(state.error).length > 0
      const reason = state.failedStep === 'end-stream' && !coded ? texts.fixStopFailed : state.failedStep === 'check' && !coded ? texts.statusUnavailable : friendly(state.error)
      return { tone: 'error', text: fill(texts.fixFailed, { step: step(state.failedStep), reason }), action: 'diagnostics' }
    }
    default: return { tone: 'info', text: '', action: '' }
  }
}

/** The label of the step that is running, for the progress sentence. */
export function fixStepLabel(id, texts, version = '') {
  return id === 'end-stream' ? texts.fixStepStream : id === 'cleanup' ? texts.fixStepClean : id === 'replace' ? texts.fixStepReplace : fill(texts.fixStepInstall, { version })
}

/** The sentence of a running fix: which step of how many, and what it is doing. */
export function fixProgressText(state, texts, version = '') {
  const total = state.steps.length
  const id = state.steps[Math.min(state.index, total - 1)]
  if (!total || !id) return ''
  return fill(texts.fixProgress, { n: Math.min(state.index + 1, total), total, step: fixStepLabel(id, texts, version) })
}

/** The text of the one question: what will be done, and that the stream ends when one is running. */
export function fixConfirmText(plan, texts, version = '') {
  const body = plan.replacesExternal ? texts.replaceConfirm : fill(texts.fixConfirm, { version })
  return plan.streaming ? `${texts.fixEndsStream}\n${body}` : body
}

function fill(text, values) {
  return String(text || '').replace(/\{(\w+)\}/g, (match, key) => (key in values ? String(values[key]) : match))
}
