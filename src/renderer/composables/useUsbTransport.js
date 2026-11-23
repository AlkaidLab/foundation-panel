import { computed, onActivated, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from './notifyMessage.js'
import { sunshine, usbip } from '../tauri-adapter.js'
import compatibility from '../../../src-tauri/component-compatibility.json' with { type: 'json' }
import { useI18n } from '../desktop/i18n/index.js'
import { useDeviceComponentOperation } from './useDeviceComponentOperation.js'
import { usbipStatusChannel } from './deviceStatusStore.js'
import { usbipCanReplaceLegacy, usbipResidualIsStale, usbipTransportNeedsCleanup } from './deviceHubStatus.js'
import { describeFixResult, fixConfirmText, fixProgressText, runUsbipFix, usbipFixOffered, usbipFixOfferedByStatus, usbipFixRawText } from './usbipFix.js'

// what the last one-click fix answered, as raw text: shared by every panel that shows the transport, kept until the next fix
const fixRaw = ref('')
const FIX_MESSAGE_ID = 'usbip-fix'

/** Extracts a fixed-vocabulary telemetry error code (never user text or paths). */
export function usbipErrorReportCode(error) {
  const message = String(error || '')
  const codes = message.match(/USBIP-[A-Z]+-\d{3}/g) || []
  const primary = (codes[0] === 'USBIP-SETUP-007' && codes[1]) ? codes[1] : (codes[0] || '')
  if (!primary) return 'usbip-unknown'
  const hex = message.match(/\b0x[0-9a-f]+\b/i)?.[0]?.toLowerCase()
  const exit = message.match(/\bexit code (-?\d+)\b/i)?.[1]
  const sub = hex || (exit ? `exit-${exit}` : '')
  const raw = sub ? `${primary}.${sub}` : primary
  return raw.toLowerCase().replace(/[^a-z0-9._-]+/g, '-').slice(0, 40).replace(/-+$/, '')
}

/** Dispatches a structured USB/IP error event without any user text. */
export function emitUsbipErrorReport(error, reporter) {
  const code = usbipErrorReportCode(error)
  if (typeof reporter === 'function') {
    reporter({ component: 'usbip', code, severity: 'error' })
    return code
  }
  if (typeof globalThis.window?.dispatchEvent === 'function' && typeof globalThis.CustomEvent === 'function') {
    globalThis.window.dispatchEvent(new globalThis.CustomEvent('foundation-error-event', {
      detail: { component: 'usbip', code, severity: 'error' },
    }))
  }
  return code
}

/** Resolves the most specific translated user message for a structured USB/IP error. */
export function formatUsbipFriendlyError(error, usbTexts = {}, componentTexts = {}) {
  const message = String(error || '')
  const codes = message.match(/USBIP-[A-Z]+-\d{3}/g) || []
  const code = (codes[0] === 'USBIP-SETUP-007' && codes[1]) ? codes[1] : codes[0]
  if (code === 'USBIP-SETUP-018' && componentTexts.operationBusy) return componentTexts.operationBusy
  const translated = code && usbTexts.errors?.[code]
  return translated || usbTexts.unknownError || 'The USB/IP operation did not complete.'
}

/** Coordinates shared transport maintenance and refreshes actual state after failed operations. */
export function useUsbTransport(options = {}) {
  const { t } = useI18n()
  const sharedOperation = useDeviceComponentOperation()
  const status = reactive({
    supported: true,
    installed: false,
    externally_managed: false,
    replaceable: false,
    ready: false,
    version: '',
    version_valid: false,
    reboot_recommended: false,
    vhci_residual: false,
    residual_kind: '',
    cleanup_pending_restart: false,
    cleanup_resume: false,
    cleanup_resume_op: '',
    last_cleanup_error: '',
    last_install_error: '',
    install_pending_restart: false,
    install_resume: false,
    error_code: '',
    attached_devices: [],
    detail: '',
  })
  const statusLoading = ref(false)
  const statusLoaded = ref(false)
  const statusProbeFailed = ref(false)
  const installing = ref(false)
  const cleaning = ref(false)
  const replacing = ref(false)
  const exporting = ref(false)
  const fixing = ref(false)
  // The cleanup that waited for a restart is continued once per run of the panel, so a node that never goes cannot loop.
  let resumeStarted = false
  let resumeAnnounced = false
  let resumeRetryTimer
  let resumeBusyRetries = 0
  let disposed = false
  let statusRefreshPromise = null

  function retryBusyResume(error, auto) {
    if (!auto || !String(error || '').includes('USBIP-SETUP-018') || resumeBusyRetries >= 36) return false
    if (!resumeRetryTimer && !disposed) {
      resumeBusyRetries += 1
      // The native preflight checks the mutex before UAC, so retrying a busy
      // automatic recovery does not repeatedly ask for administrator access.
      resumeRetryTimer = setTimeout(() => {
        resumeRetryTimer = undefined
        resumeStarted = false
        if (!disposed) void refreshStatus({ force: true })
      }, 5000)
    }
    return true
  }
  onBeforeUnmount(() => { disposed = true; clearTimeout(resumeRetryTimer) })

  const attachedDevices = computed(() => status.attached_devices || [])
  const needsCleanup = computed(() => usbipTransportNeedsCleanup(status))
  const canReplaceLegacy = computed(() => usbipCanReplaceLegacy(status))
  const staleResidual = computed(() => usbipResidualIsStale(status))
  // the operations of this window that change the transport or read it for a change; the fix runs them itself, so they do not wait for it
  const operationBlocked = () => Boolean(sharedOperation.busyElsewhere.value || installing.value || cleaning.value || replacing.value || exporting.value)
  const transportBusy = computed(() => Boolean(operationBlocked() || fixing.value))
  // an installation of the person that the host will not replace is theirs to repair: no button for it
  const fixable = () => !status.externally_managed || canReplaceLegacy.value
  const fixOffered = computed(() => usbipFixOfferedByStatus(status) && !statusProbeFailed.value && fixable())
  const statusClass = computed(() => {
    if (status.ready) return 'state-ready'
    if (!statusLoaded.value) return ''
    if (statusProbeFailed.value) return 'state-error'
    if (!status.supported) return ''
    return status.installed ? 'state-error' : 'state-update_available'
  })
  const statusTitle = computed(() => {
    if (!statusLoaded.value) return t.value.deviceHub.usb.checkingTransport
    if (statusProbeFailed.value) return t.value.deviceHub.usb.statusUnavailable
    if (!status.supported) return t.value.deviceHub.usb.transportUnsupported
    if (status.externally_managed) return t.value.deviceHub.usb.externallyManaged
    if (status.ready) return t.value.deviceHub.usb.transportReady
    if (!status.installed) return t.value.deviceHub.usb.transportMissing
    return t.value.deviceHub.usb.transportNeedsRepair
  })

  function applyStatus(next) {
    if (next) Object.assign(status, next)
  }

  function friendlyError(error) {
    return formatUsbipFriendlyError(error, t.value.deviceHub.usb, t.value.deviceHub.vbComponent)
  }

  function reportError(error) {
    emitUsbipErrorReport(error, options?.onErrorEvent)
  }

  /** The text of a cleanup failure: what Windows said and the next step; the codes themselves are in the diagnostics. */
  function cleanupError(error) {
    const text = friendlyError(error)
    const code = String(error || '').match(/^(USBIP-CLEAN-\d{3})/)?.[1]
    if (!code) return text
    const usb = t.value.deviceHub.usb
    // 006 to 008 already say what to do; the others get the generic next step and, for a refusal, the manual way.
    if (['USBIP-CLEAN-006', 'USBIP-CLEAN-008'].includes(code)) return text
    return `${text} ${code === 'USBIP-CLEAN-007' ? usb.manualRemoval : usb.cleanupFailedHint}`
  }

  function applyProbeFailure(detail) {
    statusProbeFailed.value = true
    status.ready = false
    status.attached_devices = []
    status.vhci_residual = false
    status.detail = detail
  }

  async function performStatusRefresh({ force = false } = {}) {
    statusLoading.value = true
    try {
      const result = await usbipStatusChannel.fetch({ force })
      if (disposed) return
      if (result?.success) {
        statusProbeFailed.value = false
        applyStatus(result.data)
      } else {
        applyProbeFailure(result?.message || '')
      }
    } catch (error) {
      if (disposed) return
      applyProbeFailure(String(error || ''))
    } finally {
      statusLoaded.value = true
      statusLoading.value = false
    }
    if (disposed) return
    // Windows restarted since a cleanup waited for it and the leftover is still there: continue the same operation (one prompt for
    // administrator rights). The one chance is spent only when the operation can start now; while another one holds the transport
    // the next refresh asks again.
    const resumeStale = status.cleanup_resume_op === 'stale'
    if (status.cleanup_resume && !resumeStarted && !statusProbeFailed.value && !transportBusy.value && (resumeStale || !status.externally_managed)) {
      resumeStarted = true
      if (!resumeAnnounced) ElMessage.info(t.value.deviceHub.usb.resumeCleanup)
      resumeAnnounced = true
      queueMicrotask(() => {
        if (!disposed) void (resumeStale ? removeStale({ auto: true }) : cleanupResidual({ auto: true }))
      })
    } else if (status.install_resume && !resumeStarted && !statusProbeFailed.value && !transportBusy.value && !status.externally_managed && !status.vhci_residual) {
      resumeStarted = true
      if (!resumeAnnounced) ElMessage.info(t.value.deviceHub.usb.resumeInstall)
      resumeAnnounced = true
      queueMicrotask(() => { if (!disposed) void installTransport({ auto: true }) })
    }
  }

  async function refreshStatus(options = {}) {
    if (disposed) return
    const afterCurrent = options?.afterCurrent === true
    const force = options?.force === true
    const activeRefresh = statusRefreshPromise
    if (activeRefresh) {
      await activeRefresh
      if (disposed || !afterCurrent) return
    }

    const nextRefresh = performStatusRefresh({ force })
    statusRefreshPromise = nextRefresh
    try {
      await nextRefresh
    } finally {
      if (statusRefreshPromise === nextRefresh) statusRefreshPromise = null
    }
  }

  // The three steps below answer { ok, data | error }, or { ok: false, skipped: true } when they did not start. `quiet` keeps their own messages
  // away (the fix has its own), `confirmed` means the person already answered the question.
  const installRaw = async (installOptions = {}) => {
    if (operationBlocked() || status.externally_managed) return { ok: false, skipped: true }
    const auto = installOptions?.auto === true
    const quiet = installOptions?.quiet === true
    if (auto && disposed) return { ok: false, skipped: true }
    installing.value = true
    if (!auto && installOptions?.confirmed !== true) {
      try {
        await ElMessageBox.confirm(
          t.value.deviceHub.usb.installConfirm,
          t.value.deviceHub.usb.installTransport,
          { confirmButtonText: t.value.deviceHub.usb.continue, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning' },
        )
      } catch { installing.value = false; return { ok: false, skipped: true } }
    }
    // Do not let an in-flight status refresh apply stale data over the result
    // of the operation that is about to run.
    if (statusRefreshPromise) await statusRefreshPromise
    if (auto && disposed) { installing.value = false; return { ok: false, skipped: true } }
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.installTransport()
    } catch (error) {
      if (retryBusyResume(error, auto)) return { ok: false, skipped: true }
      reportError(error)
      await refreshStatus({ force: true })
      if (!status.last_install_error && error) status.last_install_error = String(error)
      if (!quiet) ElMessage.error(friendlyError(error))
      return { ok: false, error: String(error) }
    } finally {
      installing.value = false
    }
    if (!result?.success) {
      if (retryBusyResume(result?.message, auto)) return { ok: false, skipped: true }
      reportError(result?.message)
      await refreshStatus({ force: true })
      if (!status.last_install_error && result?.message) status.last_install_error = String(result.message)
      if (!quiet) ElMessage.error(friendlyError(result?.message))
      return { ok: false, error: String(result?.message) }
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (!quiet) {
      if (result.data?.ready) ElMessage.success(t.value.deviceHub.usb.installSuccess)
      else ElMessage.warning(t.value.deviceHub.usb.rebootRequired)
    }
    return { ok: true, data: result.data }
  }
  const installTransport = sharedOperation.wrap(installRaw)

  const cleanupRaw = async (options = {}) => {
    if (operationBlocked() || status.externally_managed) return { ok: false, skipped: true }
    const removing = !needsCleanup.value
    const auto = options?.auto === true
    const quiet = options?.quiet === true
    if (auto && disposed) return { ok: false, skipped: true }
    cleaning.value = true
    // The person confirmed this cleanup before the restart (or in the fix); continuing it asks nothing new.
    if (!auto && options?.confirmed !== true) {
      try {
        await ElMessageBox.confirm(
          t.value.deviceHub.usb.cleanupConfirm,
          removing ? t.value.deviceHub.components.uninstallTransport : t.value.deviceHub.usb.cleanupResidual,
          { confirmButtonText: removing ? t.value.systemTools.uninstall : t.value.deviceHub.usb.continue, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning', danger: removing },
        )
      } catch { cleaning.value = false; return { ok: false, skipped: true } }
    }
    if (statusRefreshPromise) await statusRefreshPromise
    if (auto && disposed) { cleaning.value = false; return { ok: false, skipped: true } }
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.cleanupTransport()
    } catch (error) {
      if (retryBusyResume(error, auto)) return { ok: false, skipped: true }
      reportError(error)
      await refreshStatus({ force: true })
      if (!quiet) ElMessage.error(cleanupError(error))
      return { ok: false, error: String(error) }
    } finally {
      cleaning.value = false
    }
    if (!result?.success) {
      if (retryBusyResume(result?.message, auto)) return { ok: false, skipped: true }
      reportError(result?.message)
      // The failed cleanup may still have removed some devnodes; refresh so the
      // residual flag reflects the actual state instead of the stale snapshot.
      await refreshStatus({ force: true })
      if (!quiet) ElMessage.error(cleanupError(result?.message))
      return { ok: false, error: String(result?.message) }
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (!quiet) {
      if (result.data?.cleanup_pending_restart) ElMessage.warning(t.value.deviceHub.usb.restartPending)
      else if (result.data?.vhci_residual) ElMessage.error(cleanupError('USBIP-CLEAN-002'))
      else ElMessage.success(removing ? t.value.deviceHub.components.transportRemoved : t.value.deviceHub.usb.cleanupSuccess)
    }
    return { ok: true, data: result.data }
  }
  const cleanupResidual = sharedOperation.wrap(cleanupRaw)

  /** Removes the older controllers beside the working one; the live controller stays and nothing is reinstalled. */
  const removeStale = sharedOperation.wrap(async (options = {}) => {
    if (transportBusy.value || !staleResidual.value) return
    const auto = options?.auto === true
    if (auto && disposed) return
    cleaning.value = true
    if (statusRefreshPromise) await statusRefreshPromise
    if (auto && disposed) { cleaning.value = false; return }
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.removeStaleNodes()
    } catch (error) {
      if (retryBusyResume(error, auto)) return
      reportError(error)
      await refreshStatus({ force: true })
      return ElMessage.error(cleanupError(error))
    } finally {
      cleaning.value = false
    }
    if (!result?.success) {
      if (retryBusyResume(result?.message, auto)) return
      reportError(result?.message)
      await refreshStatus({ force: true })
      return ElMessage.error(cleanupError(result?.message))
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (result.data?.cleanup_pending_restart) ElMessage.warning(t.value.deviceHub.usb.restartPending)
    else if (result.data?.vhci_residual) ElMessage.error(cleanupError('USBIP-CLEAN-002'))
    else ElMessage.success(t.value.deviceHub.usb.staleSuccess)
  })

  /** Writes the diagnostics file (device tree, services, driver packages, Windows version, last cleanup answer) and shows it. */
  async function exportDiagnostics() {
    if (exporting.value) return
    exporting.value = true
    try {
      const result = await usbip.exportDiagnostics()
      if (!result?.success) return ElMessage.error(friendlyError(result?.message))
      ElMessage.success(t.value.deviceHub.usb.diagnosticsSaved.replace('{path}', String(result.data || '')))
    } catch (error) {
      ElMessage.error(friendlyError(error))
    } finally {
      exporting.value = false
    }
  }

  /** The person's own request to replace an external full usbip-win2; the host decides again and rolls back on failure. */
  const replaceRaw = async (options = {}) => {
    if (operationBlocked() || !canReplaceLegacy.value || attachedDevices.value.length) return { ok: false, skipped: true }
    const quiet = options?.quiet === true
    replacing.value = true
    if (options?.confirmed !== true) {
      try {
        await ElMessageBox.confirm(
          t.value.deviceHub.usb.replaceConfirm,
          t.value.deviceHub.usb.replaceLegacy,
          { confirmButtonText: t.value.deviceHub.usb.replaceLegacy, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning', danger: true },
        )
      } catch { replacing.value = false; return { ok: false, skipped: true } }
    }
    if (statusRefreshPromise) await statusRefreshPromise
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.replaceLegacyTransport()
    } catch (error) {
      reportError(error)
      await refreshStatus({ force: true })
      if (!status.last_install_error && error) status.last_install_error = String(error)
      if (!quiet) ElMessage.error(friendlyError(error))
      return { ok: false, error: String(error) }
    } finally {
      replacing.value = false
    }
    if (!result?.success) {
      reportError(result?.message)
      // A failed replacement may have changed the system (rolled back to "not installed"): show what is really there.
      await refreshStatus({ force: true })
      if (!status.last_install_error && result?.message) status.last_install_error = String(result.message)
      if (!quiet) ElMessage.error(friendlyError(result?.message))
      return { ok: false, error: String(result?.message) }
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (!quiet) {
      if (result.data?.ready) ElMessage.success(t.value.deviceHub.usb.replaceSuccess)
      else ElMessage.warning(t.value.deviceHub.usb.rebootRequired)
    }
    return { ok: true, data: result.data }
  }
  const replaceLegacy = sharedOperation.wrap(replaceRaw)

  /** An error toast of the panel; the version mismatch carries the fix on the toast itself. */
  function notifyError(error) {
    const message = friendlyError(error)
    if (!usbipFixOffered(error) || !fixable()) return ElMessage.error(message)
    return ElMessage.error({ message, actionText: t.value.deviceHub.usb.fixAction, duration: 15000, onClick: () => { void fixTransport() } })
  }

  /** Windows restarts 30 seconds after the person said so; `shutdown /a` still cancels it. */
  async function restartWindows() {
    const usb = t.value.deviceHub.usb
    try {
      await ElMessageBox.confirm(usb.fixRestartConfirm, usb.fixRestartNow, { confirmButtonText: usb.fixRestartNow, cancelButtonText: usb.cancel, type: 'warning' })
    } catch { return }
    const result = await usbip.restartWindows()
    if (!result?.success) ElMessage.error(friendlyError(result?.message))
  }

  /** The raw answer of every step of the last fix, and what the transport says about itself. */
  function showFixDiagnostics() {
    return ElMessageBox.alert(fixRaw.value || status.detail || '', t.value.deviceHub.usb.diagnosticDetails)
  }

  /**
   * The one-click fix of a tool or driver that differs from the pinned version: reads the status, asks once, ends the stream through the
   * host, then runs the cleanup and install (or the confirmed replacement) of this page, and says how it went in the message at the bottom.
   */
  const fixTransport = sharedOperation.wrap(async () => {
    if (operationBlocked() || fixing.value) return
    fixing.value = true
    const usb = () => t.value.deviceHub.usb
    const version = compatibility.usbip.version
    let message = null
    const show = (tone, options) => { message = ElMessage[tone]({ id: FIX_MESSAGE_ID, ...options }) }
    const progress = (state) => {
      if (state.phase === 'checking') show('info', { message: `${usb().fixStepCheck}…`, duration: 0 })
      else if (state.phase === 'running') show('info', { message: fixProgressText(state, usb(), version), duration: 0 })
    }
    let finished
    try {
      sharedOperation.markChanged()
      finished = await runUsbipFix({
        refresh: async () => {
          usbipStatusChannel.invalidate()
          await refreshStatus({ force: true, afterCurrent: true })
          return { ...status, attached_devices: [...(status.attached_devices || [])] }
        },
        sessions: async () => {
          try {
            const sessions = await sunshine.askActiveSessions()
            return Array.isArray(sessions) ? sessions : null
          } catch { return null }
        },
        confirm: ({ plan }) => ElMessageBox.confirm(
          fixConfirmText(plan, usb(), version),
          usb().fixTitle,
          { confirmButtonText: usb().fixStart, cancelButtonText: usb().cancel, type: 'warning', danger: Boolean(plan.replacesExternal) },
        ).then(() => true, () => false),
        endStream: async () => {
          try { await sunshine.stopAllSessions(); return { ok: true } } catch (error) { return { ok: false, error: String(error) } }
        },
        cleanup: () => cleanupRaw({ confirmed: true, quiet: true }),
        install: () => installRaw({ confirmed: true, quiet: true }),
        replace: () => replaceRaw({ confirmed: true, quiet: true }),
        sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
        now: () => Date.now(),
      }, progress)
    } catch (error) {
      message?.close()
      throw error
    } finally {
      fixing.value = false
    }
    await refreshStatus({ force: true, afterCurrent: true })
    fixRaw.value = usbipFixRawText(finished, status)
    const result = describeFixResult(finished, usb(), (raw) => (finished.failedStep === 'cleanup' ? cleanupError(raw) : friendlyError(raw)), version)
    if (finished.outcome === 'failed') reportError(finished.error)
    // declined: the progress sentence that waited behind the question goes away
    if (!result.text) return message?.close()
    const action = result.action === 'restart'
      ? { actionText: usb().fixRestartNow, onClick: () => { void restartWindows() } }
      : result.action === 'diagnostics' ? { actionText: usb().diagnosticDetails, onClick: () => { void showFixDiagnostics() } } : {}
    show(result.tone, { message: result.text, duration: result.tone === 'success' || result.tone === 'info' ? 8000 : 20000, ...action })
    if (result.action === 'restart') {
      ElMessage.warning({ title: result.text, duration: 0, standing: 'usbip-fix-restart', revision: finished.outcome, ...action })
    }
  })

  sharedOperation.onOtherChange(() => refreshStatus({ afterCurrent: true, force: true }))
  // A pending restart continuation may have been skipped while another
  // component's confirmation was open. Cancellation does not mark a change.
  watch(sharedOperation.busyElsewhere, (busy, wasBusy) => {
    if (wasBusy && !busy && !resumeStarted && (status.cleanup_resume || status.install_resume)) {
      void refreshStatus({ force: true })
    }
  })
  // Entering the page reads the devices again: the shared answer may be minutes old, and the native side answers this from cheap probes.
  onMounted(() => refreshStatus({ force: true }))
  // KeepAlive 重新显示时用缓存立即校准，过期则静默重探。
  onActivated(() => refreshStatus({ force: true }))
  return { status, statusLoading, statusLoaded, statusProbeFailed, installing, cleaning, replacing, exporting, fixing, fixOffered, fixRaw, attachedDevices, needsCleanup, staleResidual, canReplaceLegacy, transportBusy, statusClass, statusTitle, friendlyError, notifyError, refreshStatus, installTransport, cleanupResidual, removeStale, exportDiagnostics, replaceLegacy, fixTransport, restartWindows, showFixDiagnostics }
}
