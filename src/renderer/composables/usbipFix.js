// One-click repair of the USB/IP transport: every state the panel and the host can detect, one flow.
//
// No driver logic lives here. The steps are the ones the component page already runs: end the stream through the host, release the attached
// devices, clean the shared transport, remove the extra controllers an older version left, install the bundled USB/IP, or, for a full
// usbip-win2 of the person, the replacement they confirm. This module decides which of them apply (the inventory of states is in
// usbipFixInventory.js), drives them in order, checks the result, and turns every outcome into a state the window can show. The calls are
// injected, so every transition is testable without a driver.
import { usbipCanReplaceLegacy, usbipResidualIsStale, usbipTransportNeedsCleanup } from './deviceHubStatus.js'

export const USBIP_FIX_CODE = 'USBIP-EXEC-006'

/** How long the host gets to drop the stream after it was asked to stop, and how often it is asked. */
export const STREAM_STOP_WAIT_MS = 20000
export const STREAM_STOP_POLL_MS = 500

const codesOf = (error) => String(error || '').match(/USBIP-[A-Z]+-\d{3}/g) || []

/** The first code of an error, looking through the install wrapper (USBIP-SETUP-007: transport installation failed: <real code>). */
export function primaryCode(error) {
  const codes = codesOf(error)
  return (codes[0] === 'USBIP-SETUP-007' && codes[1]) ? codes[1] : (codes[0] || '')
}

/**
 * The failures of an ordinary call (list, attach, detach, status) that one of the repairs answers: the tool or the driver is missing, damaged,
 * of another version or does not run. An error that only appears inside another one (an install that fails with the mismatch under
 * USBIP-SETUP-007) is the repair failing, not a reason to start it again; the exporter's own errors (004, 005) and bad input are not the transport's.
 */
const OFFERED_CODES = new Set([
  'USBIP-EXEC-001', 'USBIP-EXEC-002', 'USBIP-EXEC-003', 'USBIP-EXEC-006', 'USBIP-EXEC-007',
  'USBIP-SETUP-001', 'USBIP-SETUP-002', 'USBIP-SETUP-003', 'USBIP-SETUP-008',
])

export function usbipFixOffered(error) {
  return OFFERED_CODES.has(codesOf(error)[0])
}

/** True while the transport status shows a problem the repair knows (the plan has something to do, or something to tell). */
export function usbipFixOfferedByStatus(status = {}) {
  if (status.supported === false) return false
  const plan = planUsbipFix(status)
  return plan.kind !== 'healthy' && !(plan.kind === 'blocked' && ['unsupported', 'nothing'].includes(plan.reason))
}

/** The errors the host's tool and the driver answer when the transport itself does not run: one repair covers them. */
const TOOL_ERRORS = new Set(['USBIP-EXEC-001', 'USBIP-EXEC-002', 'USBIP-EXEC-003', 'USBIP-EXEC-007'])
const badSignature = (status) => ['unsigned', 'invalid'].includes(status.driver_signature)

/**
 * What the repair would do for this status (`driver_signature` is added by the caller from the DualSense status when it was read).
 *   kind  clean-install  the transport the panel installed (or its leftovers) is cleaned, then the bundled one is installed
 *         install        nothing is installed: the bundled one is installed
 *         remove-stale   a working transport with older controllers beside it: only the extra ones go
 *         replace        a full usbip-win2 of the person: only the replacement the host allows, named in the one question
 *         healthy        nothing differs
 *         reboot         Windows has to restart before anything else can change
 *         blocked        the repair cannot run; `reason` says why and `guide` names the instruction the person gets
 *   id    the row of the inventory (usbipFixInventory.js)
 *   steps in order; `detach` and `verify` are added around them
 */
export function planUsbipFix(status = {}) {
  // `ids`: the rows of the inventory that this plan answers (attached devices are a row of their own, on top of the cause)
  const make = (patch) => ({ steps: [], ...patch, ids: [patch.id, ...(patch.attachedCount && patch.steps?.includes('detach') ? ['attached-ports'] : [])] })
  if (status.supported === false) return make({ kind: 'blocked', id: 'unsupported-os', reason: 'unsupported' })
  if (status.cleanup_pending_restart) return make({ kind: 'reboot', id: 'pending-restart-cleanup', reason: 'cleanup' })
  if (status.install_pending_restart) return make({ kind: 'reboot', id: 'pending-restart-install', reason: 'install' })
  const attachedCount = (status.attached_devices || []).length
  const withDetach = (steps) => (attachedCount ? ['detach', ...steps] : steps)
  const staleOnly = usbipResidualIsStale(status) && status.version_valid !== false && !status.error_code && !badSignature(status)
  if (status.externally_managed) {
    const sound = status.ready && status.version_valid !== false && !status.error_code && !badSignature(status)
    if (sound) {
      return status.reboot_recommended ? make({ kind: 'reboot', id: 'reboot-recommended', reason: 'install' }) : make({ kind: 'healthy', id: 'external-compatible-healthy' })
    }
    if (staleOnly) return make({ kind: 'remove-stale', id: 'external-stale-controller', steps: ['remove-stale', 'verify'] })
    if (!usbipCanReplaceLegacy(status)) return make({ kind: 'blocked', id: 'external-incompatible-manual', reason: 'external', guide: 'external' })
    return make({ kind: 'replace', id: 'external-incompatible-replaceable', steps: [...withDetach(['replace']), 'verify'], replacesExternal: true, attachedCount })
  }
  if (staleOnly) return make({ kind: 'remove-stale', id: 'stale-controller', steps: ['remove-stale', 'verify'] })
  const broken = usbipTransportNeedsCleanup(status) || badSignature(status) || Boolean(status.cleanup_resume)
  if (!broken && status.ready) {
    return status.reboot_recommended ? make({ kind: 'reboot', id: 'reboot-recommended', reason: 'install' }) : make({ kind: 'healthy', id: 'healthy' })
  }
  if (broken) {
    const id = status.error_code === USBIP_FIX_CODE ? 'mismatch-hash-abi'
      : status.error_code === 'USBIP-SETUP-002' ? 'damaged-install'
        : badSignature(status) ? 'unsigned-driver'
          : status.cleanup_resume ? 'cleanup-resume'
            : status.installed && status.version_valid === false ? 'old-version'
              : status.installed || status.vhci_residual ? 'residual-broken' : 'not-installed'
    return make({ kind: 'clean-install', id, steps: [...withDetach(['cleanup', 'install']), 'verify'], attachedCount })
  }
  if (!status.installed) return make({ kind: 'install', id: status.install_resume ? 'install-resume' : 'not-installed', steps: ['install', 'verify'] })
  if (TOOL_ERRORS.has(status.error_code)) return make({ kind: 'clean-install', id: 'tool-failed', steps: [...withDetach(['cleanup', 'install']), 'verify'], attachedCount })
  if (status.reboot_recommended) return make({ kind: 'reboot', id: 'reboot-recommended', reason: 'install' })
  return make({ kind: 'blocked', id: 'unexplained', reason: 'nothing' })
}

/**
 * The instruction the person gets for a failure that no software step can finish (the policy of Windows, a root hub that was rolled back, a
 * system disk on USB, an installation of their own) or that needs them: the id of the guide, or '' when the plain message is enough.
 */
export function guideFor(error) {
  switch (primaryCode(error)) {
    case 'USBIP-SETUP-016': return 'policy'
    case 'USBIP-SETUP-017': return 'sac'
    case 'USBIP-SETUP-011': return 'roothub'
    case 'USBIP-SETUP-012': case 'USBIP-REPLACE-003': return 'roothub-broken'
    case 'USBIP-SETUP-013': return 'usb-boot'
    case 'USBIP-SETUP-014': return 'guard'
    case 'USBIP-CLEAN-007': return 'access'
    case 'USBIP-SETUP-008': case 'USBIP-REPLACE-001': case 'USBIP-CLEAN-005': return 'external'
    case 'USBIP-SETUP-010': case 'USBIP-SETUP-005': case 'USBIP-REPLACE-004': case 'USBIP-CLEAN-001': case 'USBIP-CLEAN-002':
    case 'USBIP-CLEAN-003': case 'USBIP-CLEAN-004': case 'USBIP-VERIFY-001': case 'USBIP-VERIFY-002': return 'generic'
    default: return ''
  }
}

export const initialFix = () => ({
  phase: 'idle',
  plan: null,
  steps: [],
  index: 0,
  outcome: '',
  failedStep: '',
  error: '',
  note: '',
  resumed: false,
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
      return { ...initialFix(), phase: 'checking', resumed: Boolean(event.resumed) }
    case 'plan': {
      const plan = event.plan
      if (plan.kind === 'healthy') return { ...state, plan, phase: FINISHED, outcome: state.resumed ? 'done' : 'healthy' }
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
      return { ...state, phase: FINISHED, outcome: 'done', note: event.note || '', index: state.steps.length }
    default:
      return state
  }
}

const rawOf = (status = {}) => [status.detail, status.last_install_error, status.last_cleanup_error].filter(Boolean).join('\n')

/**
 * Runs the repair. `deps`:
 *   refresh()              reads the transport status again and resolves it (it rejects when it could not be read)
 *   sessions()             the live streams, or null when the host cannot be asked
 *   confirm({ plan, streaming })   the one question; resolves true to go on
 *   endStream()            asks the host to end the stream: { ok, error? }
 *   detach() / removeStale() / cleanup() / install() / replace()   the steps of the component page: { ok, data?, error?, skipped? }
 *   verify()               the check after the steps: { ok, error?, note? }
 *   sleep(ms), now()       injectable clock
 * `options.resume` ({ steps }) continues a repair that waited for a restart: it asks nothing when everything left to do was in the plan the
 * person confirmed, and stops without a word otherwise.
 * Resolves the finished state. Nothing here throws: a step that throws is a failed step.
 */
export async function runUsbipFix(deps, onState = () => {}, options = {}) {
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
  const resume = options.resume || null
  apply({ type: 'check', resumed: Boolean(resume) })
  let status
  try {
    status = await deps.refresh()
  } catch (error) {
    return fail('check', error?.message || error)
  }
  let plan = planUsbipFix(status)
  // after a restart that finished the cleanup, only the check is left
  if (resume && plan.kind === 'healthy') plan = { kind: 'verify-only', id: plan.id, steps: ['verify'] }
  let sessions = null
  if (plan.steps.some((step) => step !== 'verify')) {
    try { sessions = await deps.sessions() } catch { sessions = null }
    const streaming = Array.isArray(sessions) && sessions.length > 0
    plan = { ...plan, streaming, steps: streaming ? ['end-stream', ...plan.steps] : plan.steps }
  }
  apply({ type: 'plan', plan })
  if (state.phase !== 'confirming') return state
  let accepted = false
  if (resume) {
    // the person said yes to this much before the restart; anything beyond it asks again, from the button
    accepted = plan.steps.every((step) => step === 'end-stream' || step === 'verify' || resume.steps.includes(step))
    if (!accepted) return apply({ type: 'declined' })
  } else {
    try { accepted = await deps.confirm({ plan, streaming: Boolean(plan.streaming) }) } catch { accepted = false }
    if (!accepted) return apply({ type: 'declined' })
  }
  apply({ type: 'confirmed' })
  const rebootWith = (kind) => {
    deps.onReboot?.({ plan, kind })
    return apply({ type: 'reboot', kind })
  }

  for (const step of plan.steps) {
    if (step === 'end-stream') {
      const stopped = await guarded(() => deps.endStream(), 'the host did not accept the request')
      if (!stopped?.ok) return fail(step, stopped?.error || 'the host did not accept the request')
      // The stream counts as ended only when the host answers with an empty list. An answer that cannot be read (null) is not one: the
      // wait goes on, and at its limit the repair stops instead of changing the transport under a stream that may still run.
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
    if (step === 'verify') {
      const checked = await guarded(() => deps.verify(), 'the check did not finish')
      if (!checked?.ok) return fail(step, checked?.error || 'USBIP-VERIFY-001: USB/IP is not ready after the repair')
      apply({ type: 'step-done', step, raw: checked.raw || '' })
      return apply({ type: 'done', note: checked.note || '' })
    }
    const run = { detach: deps.detach, 'remove-stale': deps.removeStale, cleanup: deps.cleanup, replace: deps.replace, install: deps.install }[step]
    const result = await guarded(() => run(), 'the step did not finish')
    if (result?.skipped) return fail(step, 'USBIP-SETUP-018: another driver operation is in progress')
    if (!result?.ok) return fail(step, result?.error || 'the step did not finish')
    const data = result.data || {}
    if (step === 'detach') {
      apply({ type: 'step-done', step, raw: result.raw || '' })
      continue
    }
    if (step === 'cleanup' || step === 'remove-stale') {
      if (data.cleanup_pending_restart) {
        apply({ type: 'step-done', step, raw: rawOf(data) })
        return rebootWith('cleanup')
      }
      if (data.vhci_residual) return fail(step, 'USBIP-CLEAN-002: the USB/IP host controller is still there after the cleanup')
      apply({ type: 'step-done', step, raw: rawOf(data) })
      continue
    }
    // install and replace: ready, or Windows has to restart first
    if (data.install_pending_restart) {
      apply({ type: 'step-done', step, raw: rawOf(data) })
      return rebootWith('install')
    }
    if (!data.ready) {
      if (data.reboot_recommended) {
        apply({ type: 'step-done', step, raw: rawOf(data) })
        return rebootWith('install')
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
 * The words of a finished run: { tone, text, action, guide }. `friendly` maps a raw error to its translated message; `texts` is deviceHub.usb.
 * `action` names what the person can do next from the same message: view the raw diagnostics, the instruction (`guide`), or restart Windows.
 */
export function describeFixResult(state, texts, friendly, version = '') {
  const guided = (text, guide, tone = 'warning') => ({ tone, text, action: guide ? 'guide' : 'diagnostics', guide })
  switch (state.outcome) {
    case 'done': return { tone: 'success', text: state.note === 'host-unreachable' ? texts.fixDoneHostDown : texts.fixDone, action: '', guide: '' }
    case 'healthy': return { tone: 'info', text: texts.fixHealthy, action: '', guide: '' }
    case 'cancelled': return { tone: 'info', text: '', action: '', guide: '' }
    case 'reboot-cleanup': return { tone: 'warning', text: texts.fixRebootCleanup, action: 'restart', guide: '' }
    case 'reboot-install': return { tone: 'warning', text: fill(texts.fixRebootInstall, { version }), action: 'restart', guide: '' }
    case 'blocked-external': return guided(texts.externalDecision, 'external')
    case 'blocked-unsupported': return { tone: 'warning', text: texts.transportUnsupported, action: '', guide: '' }
    case 'blocked-nothing': return { tone: 'warning', text: texts.fixNothing, action: 'diagnostics', guide: '' }
    case 'failed': {
      const coded = codesOf(state.error).length > 0
      const reason = state.failedStep === 'end-stream' && !coded ? texts.fixStopFailed : state.failedStep === 'check' && !coded ? texts.statusUnavailable : friendly(state.error)
      const text = fill(texts.fixFailed, { step: fixStepLabel(state.failedStep, texts, version), reason })
      return guided(text, guideFor(state.error), 'error')
    }
    default: return { tone: 'info', text: '', action: '', guide: '' }
  }
}

/** The label of a step, for the progress sentence and for the failure. */
export function fixStepLabel(id, texts, version = '') {
  switch (id) {
    case 'end-stream': return texts.fixStepStream
    case 'detach': return texts.fixStepDetach
    case 'cleanup': return texts.fixStepClean
    case 'remove-stale': return texts.fixStepStale
    case 'replace': return texts.fixStepReplace
    case 'install': return fill(texts.fixStepInstall, { version })
    case 'verify': return texts.fixStepVerify
    default: return texts.fixStepCheck
  }
}

/** The sentence of a running repair: which step of how many, and what it is doing. */
export function fixProgressText(state, texts, version = '') {
  const total = state.steps.length
  const id = state.steps[Math.min(state.index, total - 1)]
  if (!total || !id) return ''
  return fill(texts.fixProgress, { n: Math.min(state.index + 1, total), total, step: fixStepLabel(id, texts, version) })
}

/**
 * The text of the one question: everything that will be ended, released or removed, then what is installed, and that Windows may ask for a
 * restart. A full usbip-win2 of the person is named only through the replacement text of the component page (it says its GUI and SDK go too).
 */
export function fixConfirmText(plan, texts, version = '') {
  const lines = []
  if (plan.streaming) lines.push(texts.fixEndsStream)
  if (plan.attachedCount && plan.steps.includes('detach')) lines.push(fill(texts.fixWillDetach, { count: plan.attachedCount }))
  if (plan.replacesExternal) lines.push(texts.replaceConfirm)
  else if (plan.kind === 'remove-stale') lines.push(texts.fixConfirmStale)
  else if (plan.kind === 'install') lines.push(fill(texts.fixConfirmInstall, { version }))
  else lines.push(fill(texts.fixConfirm, { version }))
  lines.push(texts.fixMayReboot)
  return lines.join('\n')
}

// ---- a repair that waits for a restart continues afterwards ----
const RESUME_KEY = 'foundation.usbip.fix.resume'
const RESUME_MAX_AGE_MS = 24 * 60 * 60 * 1000

/** Keeps what was confirmed, so the steps left run after Windows restarted. `storage` is localStorage (a read or write that fails keeps nothing). */
export function saveFixResume(storage, plan, now = Date.now()) {
  try { storage?.setItem(RESUME_KEY, JSON.stringify({ id: plan.id, steps: plan.steps.filter((step) => step !== 'end-stream'), written: now })) } catch { /* nothing kept: the person presses the button again */ }
}

export function clearFixResume(storage) {
  try { storage?.removeItem(RESUME_KEY) } catch { /* nothing to clear */ }
}

/**
 * The kept plan when this run of the panel started after it was written (Windows restarted, or the panel did) and it is not older than a day;
 * otherwise null (and the stale record is dropped).
 */
export function loadFixResume(storage, { processStart = 0, now = Date.now() } = {}) {
  let value = null
  try { value = JSON.parse(storage?.getItem(RESUME_KEY) || 'null') } catch { value = null }
  if (!value || !Array.isArray(value.steps) || !Number.isFinite(value.written)) return null
  if (now - value.written > RESUME_MAX_AGE_MS || value.written >= processStart) {
    if (now - value.written > RESUME_MAX_AGE_MS) clearFixResume(storage)
    return null
  }
  return { id: String(value.id || ''), steps: value.steps.map(String), written: value.written }
}

export const FIX_GUIDE_KEYS = {
  policy: 'fixGuidePolicy',
  sac: 'fixGuideSac',
  roothub: 'fixGuideRootHub',
  'roothub-broken': 'fixGuideRootHubBroken',
  'usb-boot': 'fixGuideUsbBoot',
  guard: 'fixGuideGuard',
  access: 'fixGuideAccess',
  external: 'fixGuideExternal',
  generic: 'fixGuideGeneric',
}

/** The numbered steps of a guide, in the person's language; '' for an id that has none. */
export function fixGuideText(id, texts, version = '') {
  const key = FIX_GUIDE_KEYS[id]
  return key ? fill(texts[key], { version }) : ''
}

export const FIX_GUIDE_IDS = Object.keys(FIX_GUIDE_KEYS)

function fill(text, values) {
  return String(text || '').replace(/\{(\w+)\}/g, (match, key) => (key in values ? String(values[key]) : match))
}
