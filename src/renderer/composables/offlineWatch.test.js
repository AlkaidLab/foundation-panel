import assert from 'node:assert/strict'
import fs from 'node:fs'
import test from 'node:test'
import { createOfflineWatch, hostAnswered, WATCH_DELAYS_MS, WATCH_LIMIT_MS } from './offlineWatch.js'

// A fake clock: timers run in order as the clock is advanced, and a probe answers after the time the scenario gives it.
function harness({ answer }) {
  let at = 0
  let seq = 0
  const timers = new Map()
  const log = []
  const probe = () => new Promise((resolve, reject) => {
    const verdict = answer(at)
    log.push({ at, event: 'probe' })
    timers.set(++seq, { at: at + (verdict.delay || 0), fn: () => (verdict.status === undefined ? reject(new Error('down')) : resolve(verdict.status)), id: seq })
  })
  const watch = createOfflineWatch({
    probe,
    hostUp: () => log.push({ at, event: 'up' }),
    now: () => at,
    schedule: (fn, ms) => { const id = ++seq; timers.set(id, { at: at + ms, fn, id }); return id },
    cancel: (id) => timers.delete(id),
  })
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve() }
  return {
    watch, log,
    async advance(ms) {
      const end = at + ms
      await settle()
      for (;;) {
        const next = [...timers.values()].filter((t) => t.at <= end).sort((a, b) => a.at - b.at || a.id - b.id)[0]
        if (!next) break
        at = next.at
        timers.delete(next.id)
        next.fn()
        await settle()
      }
      at = end
      await settle()
    },
  }
}

test('a host that answers the first probe is told to the page at once, and the watch ends', async () => {
  const h = harness({ answer: () => ({ status: 200 }) })
  h.watch.notice()
  await h.advance(0)
  assert.deepEqual(h.log, [{ at: 0, event: 'probe' }, { at: 0, event: 'up' }])
  assert.equal(h.watch.running, false)
})

test('a host that is down is probed after 1, 2 and then every 5 seconds, and the page is told the moment it answers', async () => {
  const h = harness({ answer: (at) => (at >= 9000 ? { status: 200 } : {}) })
  h.watch.notice()
  await h.advance(20000)
  assert.deepEqual(h.log.filter((e) => e.event === 'probe').map((e) => e.at), [0, 1000, 3000, 8000, 13000])
  assert.deepEqual(h.log.at(-1), { at: 13000, event: 'up' })
})

test('a 5xx answer is a host that is still starting, not an answer', () => {
  assert.equal(hostAnswered(200), true)
  assert.equal(hostAnswered(401), true)
  assert.equal(hostAnswered(503), false)
  assert.equal(hostAnswered(undefined), false)
})

test('a host that answers late (a probe that waits for it) is waited for, one probe at a time', async () => {
  const h = harness({ answer: (at) => (at === 0 ? { status: 200, delay: 2400 } : { status: 200 }) })
  h.watch.notice()
  h.watch.notice()
  await h.advance(5000)
  assert.deepEqual(h.log, [{ at: 0, event: 'probe' }, { at: 2400, event: 'up' }])
})

test('a watch that finds nothing for a minute ends, and the next notice from the page starts a new one', async () => {
  const h = harness({ answer: () => ({}) })
  h.watch.notice()
  await h.advance(WATCH_LIMIT_MS + 6000)
  const probes = h.log.filter((e) => e.event === 'probe')
  assert.ok(probes.at(-1).at <= WATCH_LIMIT_MS + 5000, 'no probe after the limit')
  assert.equal(h.watch.running, false)
  const before = probes.length
  h.watch.notice()
  await h.advance(0)
  assert.equal(h.log.filter((e) => e.event === 'probe').length, before + 1)
  assert.equal(h.watch.running, true)
})

test('a stopped watch asks nothing more and never reports, even for a probe that was out', async () => {
  const h = harness({ answer: (at) => (at === 0 ? { status: 200, delay: 500 } : {}) })
  h.watch.notice()
  await h.advance(100)
  h.watch.stop()
  await h.advance(10000)
  assert.deepEqual(h.log, [{ at: 0, event: 'probe' }])
  assert.equal(h.watch.running, false)
})

test('the delays are quick first and settle at five seconds', () => {
  assert.deepEqual([...WATCH_DELAYS_MS], [1000, 2000, 5000])
})

// ---- the way it is wired: the page tells the panel it is up, the panel tells the page the host answers -------------------------------------------------------------------------
const frame = fs.readFileSync(new URL('../components/SunshineFrame.vue', import.meta.url), 'utf8')
const page = fs.readFileSync(new URL('../../../src-tauri/error-404.html', import.meta.url), 'utf8')

test('the panel starts the watch on a message from the frame it shows, probes natively, and tells that frame the host answers', () => {
  assert.match(frame, /import \{ createOfflineWatch \} from '\.\.\/composables\/offlineWatch\.js'/)
  assert.match(frame, /probe: \(\) => sunshine\.probeHost\(/, 'the probe is the native one')
  assert.match(frame, /'foundation-offline-page': \(_data, event\) => \{ if \(isTrustedSunshineEvent\(event\)\) offlineWatch\.notice\(\) \}/, 'only the frame that is shown may start it')
  assert.match(frame, /postMessage\(\{ type: 'foundation-offline-host-up' \}, origin\)/)
  assert.match(frame, /'webui-ready': \(\) => \{ offlineWatch\.stop\(\); finishLoading\(\) \}/, 'a real page ends the watch')
  assert.match(frame, /offlineWatch\.stop\(\)\s*\n\s*clearLoadedFrameReveal\(\)/, 'leaving the view ends it')
})

test('the offline page says that it is up and returns when the panel tells it the host answers', () => {
  assert.match(page, /type: 'foundation-offline-page'/)
  assert.match(page, /'foundation-offline-host-up'/)
  assert.match(page, /event\.source !== hostWindow\.parent/, 'only the panel above the page may tell it')
})
