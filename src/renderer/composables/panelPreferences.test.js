import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { KEEP_ON_PREFERENCES_RESET, PRIVACY_KEYS, resetPanelPreferences } from './panelPreferences.js'

const storage = (entries) => {
  const map = new Map(Object.entries(entries))
  return { get length() { return map.size }, key: (i) => [...map.keys()][i] ?? null, getItem: (k) => (map.has(k) ? map.get(k) : null), setItem: (k, v) => map.set(k, String(v)), removeItem: (k) => map.delete(k), keys: () => [...map.keys()] }
}
const read = (path) => readFileSync(new URL(path, import.meta.url), 'utf8')

test('the native files are reset first, then the window storage, except the developer mode, the privacy choices and the AI connection; then the window loads again', async () => {
  const order = []
  const local = storage({ 'sunshine-theme': 'dark', language: 'en', 'foundation-ui.dash2.layout': '[]', 'foundation-ui.dev': '1', 'foundation-ui.dev.flags': '{}', 'foundation-ui.telemetry.consent': 'off', 'foundation-ui.telemetry.errors': 'off', 'foundation-ui.telemetry.notice': '1', 'sunshine-ai-config': '{"key":"x"}' })
  const session = storage({ 'foundation.guide.dismissed': '1' })
  const cleared = await resetPanelPreferences({ invoke: async (name) => { order.push(name) }, local, session, reload: () => order.push('reload') })
  assert.deepEqual(order, ['reset_panel_preferences', 'reload'])
  assert.deepEqual(local.keys().sort(), ['foundation-ui.dev', 'foundation-ui.dev.flags', 'foundation-ui.telemetry.consent', 'foundation-ui.telemetry.errors', 'foundation-ui.telemetry.notice', 'sunshine-ai-config'])
  assert.equal(cleared, 3)
  assert.deepEqual(session.keys(), [])
})

test('when the native side refuses, nothing is cleared and the window does not reload', async () => {
  const local = storage({ 'sunshine-theme': 'dark' })
  let reloaded = false
  await assert.rejects(resetPanelPreferences({ invoke: async () => { throw new Error('disk') }, local, session: storage({}), reload: () => { reloaded = true } }), /disk/)
  assert.deepEqual(local.keys(), ['sunshine-theme'])
  assert.equal(reloaded, false)
})

test('the kept keys are the developer mode, the privacy choices and the AI connection, and the privacy keys are the ones foundation-ui writes', () => {
  assert.ok(KEEP_ON_PREFERENCES_RESET.includes('sunshine-ai-config'))
  for (const key of PRIVACY_KEYS) assert.ok(KEEP_ON_PREFERENCES_RESET.includes(key))
  assert.ok(KEEP_ON_PREFERENCES_RESET.includes('foundation-ui.dev'))
})

test('the panel shows the page in the system rail, finds it from the search, the diagnostics page and the frame, and the native command is registered', () => {
  const router = read('./useRouter.js')
  assert.match(router, /RESET: 'reset'/)
  assert.match(router, /\[ROUTES\.RESET\]: \{ rail: 'system' \}/)
  assert.match(read('./panelNav.js'), /id: ROUTES\.RESET, icon: 'restart_alt'/)
  const shell = read('../components/SidebarMenu.vue')
  assert.match(shell, /<ResetSettingsPage v-else-if="entry\.id === ROUTES\.RESET"/)
  assert.match(shell, /resetSettings: \(\) => jumpTo\(ROUTES\.RESET\)/)
  assert.match(shell, /data\.page === 'reset'\) router\.navigate\(ROUTES\.RESET\)/)
  assert.match(shell, /@open-reset="jumpTo\(ROUTES\.RESET\)"/)
  assert.match(shell, /ROUTES\.DOCTOR, ROUTES\.RESET,/, 'the preferences can be reset while the host is away')
  assert.match(read('../components/m3e/DiagnosticsPage.vue'), /resetSettings: \(\) => emit\('open-reset'\)/)
  assert.match(read('../../../src-tauri/src/main.rs'), /tray::reset_panel_preferences/)
  const page = read('../components/m3e/ResetSettingsPage.vue')
  assert.match(page, /createResetClient/)
  assert.match(page, /:panel="panel"/)
})
