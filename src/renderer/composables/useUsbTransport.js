import { computed, onActivated, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from './notifyMessage.js'
import { usbip } from '../tauri-adapter.js'
import { useI18n } from '../desktop/i18n/index.js'
import { useDeviceComponentOperation } from './useDeviceComponentOperation.js'
import { usbipStatusChannel } from './deviceStatusStore.js'
import { usbipCanReplaceLegacy, usbipResidualIsStale, usbipTransportNeedsCleanup } from './deviceHubStatus.js'

/** Coordinates shared transport maintenance and refreshes actual state after failed operations. */
export function useUsbTransport() {
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
    last_cleanup_error: '',
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
  // The cleanup that waited for a restart is continued once per run of the panel, so a node that never goes cannot loop.
  let resumeStarted = false
  let statusRefreshPromise = null

  const attachedDevices = computed(() => status.attached_devices || [])
  const needsCleanup = computed(() => usbipTransportNeedsCleanup(status))
  const canReplaceLegacy = computed(() => usbipCanReplaceLegacy(status))
  const staleResidual = computed(() => usbipResidualIsStale(status))
  const transportBusy = computed(() => Boolean(sharedOperation.busyElsewhere.value || installing.value || cleaning.value || replacing.value || exporting.value))
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
    const message = String(error || '')
    const code = message.match(/^(USBIP-[A-Z]+-\d{3})/)?.[1]
    const translated = code && t.value.deviceHub.usb.errors?.[code]
    return translated || t.value.deviceHub.usb.unknownError
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
      if (result?.success) {
        statusProbeFailed.value = false
        applyStatus(result.data)
      } else {
        applyProbeFailure(result?.message || '')
      }
    } catch (error) {
      applyProbeFailure(String(error || ''))
    } finally {
      statusLoaded.value = true
      statusLoading.value = false
    }
    // Windows restarted since a cleanup waited for it and the leftover is still there: continue it (one prompt for administrator rights).
    if (status.cleanup_resume && !resumeStarted && !statusProbeFailed.value && !status.externally_managed) {
      resumeStarted = true
      ElMessage.info(t.value.deviceHub.usb.resumeCleanup)
      queueMicrotask(() => cleanupResidual({ auto: true }))
    }
  }

  async function refreshStatus(options = {}) {
    const afterCurrent = options?.afterCurrent === true
    const force = options?.force === true
    const activeRefresh = statusRefreshPromise
    if (activeRefresh) {
      await activeRefresh
      if (!afterCurrent) return
    }

    const nextRefresh = performStatusRefresh({ force })
    statusRefreshPromise = nextRefresh
    try {
      await nextRefresh
    } finally {
      if (statusRefreshPromise === nextRefresh) statusRefreshPromise = null
    }
  }

  const installTransport = sharedOperation.wrap(async () => {
    if (transportBusy.value || status.externally_managed) return
    installing.value = true
    try {
      await ElMessageBox.confirm(
        t.value.deviceHub.usb.installConfirm,
        t.value.deviceHub.usb.installTransport,
        { confirmButtonText: t.value.deviceHub.usb.continue, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning' },
      )
    } catch { installing.value = false; return }
    // Do not let an in-flight status refresh apply stale data over the result
    // of the operation that is about to run.
    if (statusRefreshPromise) await statusRefreshPromise
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.installTransport()
    } catch (error) {
      await refreshStatus({ force: true })
      return ElMessage.error(friendlyError(error))
    } finally {
      installing.value = false
    }
    if (!result?.success) {
      await refreshStatus({ force: true })
      return ElMessage.error(friendlyError(result?.message))
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (result.data?.ready) ElMessage.success(t.value.deviceHub.usb.installSuccess)
    else ElMessage.warning(t.value.deviceHub.usb.rebootRequired)
  })

  const cleanupResidual = sharedOperation.wrap(async (options = {}) => {
    if (transportBusy.value || status.externally_managed) return
    const removing = !needsCleanup.value
    const auto = options?.auto === true
    cleaning.value = true
    // The person confirmed this cleanup before the restart; continuing it asks nothing new.
    if (!auto) {
      try {
        await ElMessageBox.confirm(
          t.value.deviceHub.usb.cleanupConfirm,
          removing ? t.value.deviceHub.components.uninstallTransport : t.value.deviceHub.usb.cleanupResidual,
          { confirmButtonText: removing ? t.value.systemTools.uninstall : t.value.deviceHub.usb.continue, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning', danger: removing },
        )
      } catch { cleaning.value = false; return }
    }
    if (statusRefreshPromise) await statusRefreshPromise
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.cleanupTransport()
    } catch (error) {
      await refreshStatus({ force: true })
      return ElMessage.error(cleanupError(error))
    } finally {
      cleaning.value = false
    }
    if (!result?.success) {
      // The failed cleanup may still have removed some devnodes; refresh so the
      // residual flag reflects the actual state instead of the stale snapshot.
      await refreshStatus({ force: true })
      return ElMessage.error(cleanupError(result?.message))
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (result.data?.cleanup_pending_restart) ElMessage.warning(t.value.deviceHub.usb.restartPending)
    else if (result.data?.vhci_residual) ElMessage.error(cleanupError('USBIP-CLEAN-002'))
    else ElMessage.success(removing ? t.value.deviceHub.components.transportRemoved : t.value.deviceHub.usb.cleanupSuccess)
  })

  /** Removes the older controllers beside the working one; the live controller stays and nothing is reinstalled. */
  const removeStale = sharedOperation.wrap(async () => {
    if (transportBusy.value || !staleResidual.value) return
    cleaning.value = true
    if (statusRefreshPromise) await statusRefreshPromise
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.removeStaleNodes()
    } catch (error) {
      await refreshStatus({ force: true })
      return ElMessage.error(cleanupError(error))
    } finally {
      cleaning.value = false
    }
    if (!result?.success) {
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
  const replaceLegacy = sharedOperation.wrap(async () => {
    if (transportBusy.value || !canReplaceLegacy.value || attachedDevices.value.length) return
    replacing.value = true
    try {
      await ElMessageBox.confirm(
        t.value.deviceHub.usb.replaceConfirm,
        t.value.deviceHub.usb.replaceLegacy,
        { confirmButtonText: t.value.deviceHub.usb.replaceLegacy, cancelButtonText: t.value.deviceHub.usb.cancel, type: 'warning', danger: true },
      )
    } catch { replacing.value = false; return }
    if (statusRefreshPromise) await statusRefreshPromise
    sharedOperation.markChanged()
    usbipStatusChannel.invalidate()
    let result
    try {
      result = await usbip.replaceLegacyTransport()
    } catch (error) {
      await refreshStatus({ force: true })
      return ElMessage.error(friendlyError(error))
    } finally {
      replacing.value = false
    }
    if (!result?.success) {
      // A failed replacement may have changed the system (rolled back to "not installed"): show what is really there.
      await refreshStatus({ force: true })
      return ElMessage.error(friendlyError(result?.message))
    }
    applyStatus(result.data)
    usbipStatusChannel.prime(result)
    if (result.data?.ready) ElMessage.success(t.value.deviceHub.usb.replaceSuccess)
    else ElMessage.warning(t.value.deviceHub.usb.rebootRequired)
  })

  sharedOperation.onOtherChange(() => refreshStatus({ afterCurrent: true, force: true }))
  // Entering the page reads the devices again: the shared answer may be minutes old, and the native side answers this from cheap probes.
  onMounted(() => refreshStatus({ force: true }))
  // KeepAlive 重新显示时用缓存立即校准，过期则静默重探。
  onActivated(() => refreshStatus({ force: true }))
  return { status, statusLoading, statusLoaded, statusProbeFailed, installing, cleaning, replacing, exporting, attachedDevices, needsCleanup, staleResidual, canReplaceLegacy, transportBusy, statusClass, statusTitle, friendlyError, refreshStatus, installTransport, cleanupResidual, removeStale, exportDiagnostics, replaceLegacy }
}
