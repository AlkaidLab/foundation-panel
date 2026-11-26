// A text the window could not find is never drawn as its key. The place that asked draws the text of the base language (English, then
// Chinese) or nothing, and this module writes one line to the panel's own log (`[renderer] i18n <where> 0 ms <key> [<language>]`), once
// per key and language, so a gap that reached a person is a line with the key, not a screenshot of a key. Only the key and the language
// code are written: no text of the person, no path, no name. The line stays on the machine, like every line of that log.

const MAX_LINES = 50
const SAFE = /^[A-Za-z0-9_.:\-[\]]{1,120}$/
const seen = new Set()
let sink = null

const defaultSink = async (kind, name, detail) => {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('frontend_slow', { kind, name, ms: 0, detail })
  } catch {
    // outside the app (a browser preview, a test): the console has it
    console.warn(`[i18n] ${name} ${detail}`)
  }
}

/** Replaces where the lines go (a test; a window that reports through its own channel). `null` restores the log of the panel. */
export function setMissingTextSink(fn) { sink = typeof fn === 'function' ? fn : null }
/** Forgets what was written, for a test. */
export function resetMissingText() { seen.clear() }

/** Writes the line for one missing text; true when it was new. `where` names the place (`settings-label`, `dualsense`, `welcome`), `key` is the key that had no text. */
export function noteMissingText(where, key, locale = '') {
  const safeKey = SAFE.test(String(key ?? '')) ? String(key) : '(unreadable key)'
  const language = /^[A-Za-z_-]{2,10}$/.test(String(locale ?? '')) ? String(locale) : ''
  const id = `${where}|${safeKey}|${language}`
  if (seen.has(id) || seen.size >= MAX_LINES) return false
  seen.add(id)
  const detail = language ? `${safeKey} [${language}]` : safeKey
  void (sink || defaultSink)('i18n', `missing-text ${where}`, detail)
  return true
}
