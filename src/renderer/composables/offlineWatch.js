// The frame shows the offline page when the proxy could not answer a page request. The page asks the host again through the proxy (the way the failed request went); this watch asks it
// the other way, from the panel's own process, with the native readiness probe (`probe_sunshine_host`), and tells the page the moment the host answers so it returns at once.
// The panel's probe does not share a connection pool or a timeout with the page, and it reads the host's credential like every consumer of the host does (proxy_server.rs wait_for_host).
// It is not a retry of anything that failed: it runs only while the offline page is on screen, reports nothing but "the host answers", and the page decides how it returns (its own spacing
// of returns that end on the offline page again).

/** Between two native probes: quick at first, then every 5 s. The probe itself waits up to its own timeout for a host that is starting. */
export const WATCH_DELAYS_MS = Object.freeze([1000, 2000, 5000])
/** The watch ends after this long without an answer; the page goes on with its own rounds and tells the panel again at each of them. */
export const WATCH_LIMIT_MS = 60000

/** An answer of the host that counts as "up": any status below 500 (the web interface answers; a 5xx is a host that is still starting). */
export const hostAnswered = (status) => typeof status === 'number' && status < 500

/**
 * `probe()` resolves with the host's HTTP status or rejects. `hostUp()` is called once when it answers; the watch ends with it.
 * `notice()` starts a watch (a second one while one runs is ignored); `stop()` ends it.
 */
export function createOfflineWatch({ probe, hostUp, now = () => Date.now(), schedule = (fn, ms) => setTimeout(fn, ms), cancel = (id) => clearTimeout(id), delays = WATCH_DELAYS_MS, limitMs = WATCH_LIMIT_MS } = {}) {
  let running = false
  let timer = null
  let startedAt = 0
  let generation = 0
  const attempt = async (mine, step) => {
    timer = null
    let up = false
    try { up = hostAnswered(await probe()) } catch { up = false }
    if (mine !== generation) return
    if (up) {
      running = false
      hostUp()
      return
    }
    if (now() - startedAt >= limitMs) {
      running = false
      return
    }
    timer = schedule(() => attempt(mine, step + 1), delays[Math.min(step, delays.length - 1)])
  }
  return {
    notice() {
      if (running) return
      running = true
      startedAt = now()
      generation += 1
      attempt(generation, 0)
    },
    stop() {
      generation += 1
      running = false
      if (timer !== null) { cancel(timer); timer = null }
    },
    get running() { return running },
  }
}
