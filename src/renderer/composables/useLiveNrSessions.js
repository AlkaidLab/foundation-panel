import { reactive, ref } from 'vue'
import { nrPipelines } from './nrOverlayState.js'

export function useLiveNrSessions(ready, invoke) {
  const nr = reactive({ list: [], selected: null, enabled: null, online: true, hasStreams: false })
  const nrError = ref('')
  let pollRevision = 0, selectionRevision = 0, savedTarget, targetRestored = false

  async function selectNr(id, current = () => true) {
    const revision = ++selectionRevision
    try {
      await invoke('nr_overlay_select_session', { id })
      if (revision !== selectionRevision || !current()) return
      nr.selected = id
      nrError.value = ''
    } catch (error) {
      if (revision === selectionRevision && current()) nrError.value = String(error)
    }
  }

  async function pollNr() {
    const revision = ++pollRevision
    const current = () => revision === pollRevision && ready.value
    if (!ready.value) return
    try {
      // A timer and a refresh after a save can overlap, including on first load.
      savedTarget ??= invoke('nr_overlay_settings').then(s => s?.target ?? null, () => null)
      const [status, target] = await Promise.all([invoke('nr_live_status'), savedTarget])
      if (!current()) return
      if (!targetRestored) {
        targetRestored = true
        if (!selectionRevision && target != null) nr.selected = target
      }
      nr.enabled = status.nr_enabled
      nr.hasStreams = status.has_streams === true
      nr.list = nrPipelines(status.pipelines)
      nr.online = true
      if (!nr.list.some(p => p.id === nr.selected)) nr.selected = null
      if (nr.selected === null && nr.list.length === 1) await selectNr(nr.list[0].id, current)
    } catch {
      if (!current()) return
      nr.online = false
      nr.list = []
    }
  }

  function dispose() {
    ++pollRevision
    ++selectionRevision
  }
  return { nr, nrError, selectNr, pollNr, dispose }
}
