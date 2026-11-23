import assert from 'node:assert/strict'
import { test } from 'node:test'
import { DESKTOP_LOCALES } from '../desktop/i18n/merge.js'
import { en } from '../desktop/i18n/en.js'
import { zh } from '../desktop/i18n/zh.js'

const tables = { en, zh }
for (const code of DESKTOP_LOCALES) {
  if (!tables[code]) tables[code] = (await import(`../desktop/i18n/${code}.js`))[code]
}

test('the PIN card tells the person that the keyboard works, in every language of the panel', () => {
  assert.equal(Object.keys(tables).length, DESKTOP_LOCALES.length)
  for (const code of DESKTOP_LOCALES) {
    const hint = tables[code].pinEntry?.keyboardHint
    assert.ok(typeof hint === 'string' && hint.trim().length > 8, `${code} has the hint`)
    // the key that cancels is named in every language; the PIN is confirmed by the fourth digit, so no Enter step is promised
    assert.match(hint, /Esc|Échap/, code) // the French keyboard names the key Échap
    assert.doesNotMatch(hint, /Enter/, code)
    assert.ok(hint.length <= 60, `${code}: one short line`)
  }
})

test('the hint is shown unless the screen has nothing but touch', async () => {
  const { readFileSync } = await import('node:fs')
  const page = readFileSync(new URL('./PinApp.vue', import.meta.url), 'utf8')
  assert.match(page, /touchOnly = ref\(false\)/)
  assert.match(page, /coarsePointer\?\.matches && !finePointer\?\.matches/)
  assert.match(page, /v-if="keyboardHint && !touchOnly"/)
})
