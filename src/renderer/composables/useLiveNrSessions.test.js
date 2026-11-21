import assert from 'node:assert/strict'
import test from 'node:test'
import { ref } from 'vue'
import { useLiveNrSessions } from './useLiveNrSessions.js'

for (const stale of ['success', 'failure']) {
  test(`a late poll ${stale} cannot undo the refresh after a global NR save`, async () => {
    const reads = [], selections = []
    const { nr, pollNr } = useLiveNrSessions(ref(true), (command, args) => {
      if (command === 'nr_overlay_settings') return Promise.resolve({ target: null })
      if (command === 'nr_overlay_select_session') { selections.push(args.id); return Promise.resolve() }
      assert.equal(command, 'nr_live_status')
      const pending = Promise.withResolvers()
      reads.push(pending)
      return pending.promise
    })
    const oldPoll = pollNr(), afterSave = pollNr()
    const session = { id: 7, hdr_mode: 'sdr' }
    reads[1].resolve({ nr_enabled: true, has_streams: true, pipelines: [session] })
    await afterSave
    if (stale === 'success') reads[0].resolve({ nr_enabled: false, has_streams: false, pipelines: [] })
    else reads[0].reject(new Error('old connection timed out'))
    await oldPoll
    assert.deepEqual({ ...nr }, { enabled: true, hasStreams: true, list: [session], online: true, selected: 7 })
    assert.deepEqual(selections, [7])
    const next = pollNr()
    reads[2].resolve({ nr_enabled: false, has_streams: false, pipelines: [] })
    await next
    assert.equal(nr.enabled, false)
    assert.equal(nr.selected, null)
  })
}
