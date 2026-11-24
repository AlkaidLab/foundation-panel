import assert from 'node:assert/strict'
import fs from 'node:fs'
import test from 'node:test'
import vm from 'node:vm'

// The standalone offline page (src-tauri/error-404.html) is the document the panel's proxy answers with when a page request of the frame fails. Its script is run here on a fake clock and a fake
// network: what it asks, when it calls the host back and what it says while it waits. OFFLINE_PAGE_PATH points the same tests at another copy of the page (the one before a change).
const file = process.env.OFFLINE_PAGE_PATH || new URL('../../../src-tauri/error-404.html', import.meta.url)
const html = fs.readFileSync(file, 'utf8')
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1]

/** One load of the page. `answer(url, atMs)` returns { status, delay } for a request of the page, or 'hang'. `store` is the page's sessionStorage (it survives a reload, like the frame's). */
function loadPage({ path = '/downloads', search = '', answer, store = {}, startAt = 0 }) {
  let now = startAt
  let seq = 0
  const timers = new Map()
  const events = []
  const elements = {}
  const el = (id) => (elements[id] ||= { id, textContent: '', disabled: false, style: {}, classList: { set: new Set(), add(c) { this.set.add(c) }, remove(c) { this.set.delete(c) }, has(c) { return this.set.has(c) } }, listeners: {}, addEventListener(type, fn) { this.listeners[type] = fn } })
  const requests = []
  const location = { origin: 'http://127.0.0.1:48081', pathname: path, search, reloads: 0, reload() { this.reloads++; events.push({ at: now, event: 'reload' }) } }
  const context = {
    document: { getElementById: el },
    location,
    sessionStorage: { getItem: (k) => (k in store ? store[k] : null), setItem: (k, v) => { store[k] = String(v) } },
    Date: { now: () => now },
    AbortController,
    AbortSignal: { timeout: (ms) => { const c = new AbortController(); context.setTimeout(() => c.abort(), ms); return c.signal } },
    setTimeout: (fn, ms = 0) => { const id = ++seq; timers.set(id, { at: now + ms, fn, id }); return id },
    clearTimeout: (id) => { timers.delete(id) },
    setInterval: (fn, ms) => { const id = ++seq; timers.set(id, { at: now + ms, fn, every: ms, id }); return id },
    clearInterval: (id) => { timers.delete(id) },
    fetch: (url, options = {}) => new Promise((resolve, reject) => {
      const request = { url, at: now, cancelled: false }
      requests.push(request)
      const verdict = answer(String(url), now)
      options.signal?.addEventListener('abort', () => reject(new Error('aborted')))
      if (verdict === 'hang') return
      context.setTimeout(() => resolve({ status: verdict.status, body: { cancel: () => { request.cancelled = true; return Promise.resolve() } } }), verdict.delay || 0)
    }),
    console,
  }
  vm.createContext(context)
  vm.runInContext(script, context)
  const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve() }
  return {
    requests, location, events, el, store,
    head: () => el('wait-head').textContent,
    async advance(ms) {
      const end = now + ms
      await settle()
      for (;;) {
        const next = [...timers.values()].filter((t) => t.at <= end).sort((a, b) => a.at - b.at || a.id - b.id)[0]
        if (!next) break
        now = next.at
        if (next.every) next.at += next.every; else timers.delete(next.id)
        next.fn()
        await settle()
      }
      now = end
      await settle()
    },
    now: () => now,
  }
}

test('the page asks for its own address, not another one', async () => {
  const page = loadPage({ path: '/downloads', search: '?a=1', answer: () => ({ status: 200 }) })
  await page.advance(100)
  assert.equal(page.requests[0].url, '/downloads?a=1')
  assert.ok(page.requests.every((r) => r.url === '/downloads?a=1'), 'every probe is for the page that failed')
})

test('the host recovering for the page\'s address ends in one reload, with the probe body released', async () => {
  let up = false
  const page = loadPage({ answer: () => (up ? { status: 200 } : { status: 503 }) })
  await page.advance(3000)
  assert.equal(page.location.reloads, 0)
  up = true
  await page.advance(2000)
  assert.equal(page.location.reloads, 1)
  assert.equal(page.head(), 'Sunshine 已恢复，正在返回…')
  assert.ok(page.requests.some((r) => r.cancelled), 'the probe body is cancelled, it is not left holding a connection')
})

test('a probe that is out is waited for: a host that answers in 3 s is seen as up in the first ten seconds', async () => {
  const page = loadPage({ answer: () => ({ status: 200, delay: 3000 }) })
  await page.advance(4500)
  assert.equal(page.location.reloads, 1, 'a slow answer is an answer')
})

test('a gateway timeout is not an answer', async () => {
  const page = loadPage({ answer: () => ({ status: 504 }) })
  await page.advance(12000)
  assert.equal(page.location.reloads, 0)
})

test('a page that is answered for its probe and fails again after every reload does not reload in a tight loop', async () => {
  // the host answers the probe and then fails the document: the page is made again by each reload, with the same session storage
  const store = {}
  const delays = []
  let clock = 0
  for (let i = 0; i < 5; i++) {
    const page = loadPage({ store, startAt: clock, answer: () => ({ status: 200 }) })
    const loaded = page.now()
    while (!page.events.length && page.now() - loaded < 20000) await page.advance(100)
    assert.ok(page.events.length, 'the page returned')
    delays.push(page.events[0].at - loaded)
    clock = page.events[0].at + 800 // the next document is there 0.8 s after the reload
  }
  assert.ok(delays[0] < delays[1] && delays[1] < delays[2], `the wait before returning grows: ${delays}`)
  assert.ok(delays[4] >= 5000, `and stops growing at 5 s: ${delays}`)
})

test('a return that does not replace the page stops claiming a return and counts its rounds again', async () => {
  // the probe is answered, the reload of the document hangs (nothing replaces this page)
  const page = loadPage({ answer: () => ({ status: 200 }) })
  await page.advance(1000)
  assert.equal(page.head(), 'Sunshine 已恢复，正在返回…')
  await page.advance(21000)
  assert.notEqual(page.head(), 'Sunshine 已恢复，正在返回…')
  assert.match(page.head(), /次探测失败/)
  assert.equal(page.el('retry').disabled, false, 'the button works again')
  assert.equal(page.el('retry-label').textContent, '再试一次')
})

test('while the host is down it probes once a second for ten seconds, then every 5 s, then every 30 s', async () => {
  const page = loadPage({ answer: () => ({ status: 503 }) })
  await page.advance(10000)
  assert.ok(page.requests.length >= 5 && page.requests.length <= 11, `${page.requests.length} probes in the first ten seconds`)
  const before = page.requests.length
  await page.advance(5200)
  assert.equal(page.requests.length, before + 1)
})
