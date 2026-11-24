import test from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

// scripts/check-i18n-keys.mjs: every key the code reads exists in English, Chinese holds the keys of English, and a language that lacks
// a key is listed in scripts/i18n-untranslated.json (it shows the English text on purpose). Run on its own: node scripts/check-i18n-keys.mjs
test('the text tables have every key the code reads, and no language has a gap that is not listed', () => {
  const script = fileURLToPath(new URL('../../../../scripts/check-i18n-keys.mjs', import.meta.url))
  const result = spawnSync(process.execPath, [script], { encoding: 'utf8' })
  assert.equal(result.status, 0, `${result.stdout}${result.stderr}`)
})
