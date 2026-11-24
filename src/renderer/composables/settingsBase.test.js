import test from 'node:test'
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { MODEL_LABEL_KEYS } from 'foundation-ui/settings/model.js'
import { labelOf, provideSettingsData } from 'foundation-ui/settings/provider.js'
import { BASE_LABELS, ensureBaseLabels } from './settingsBase.js'
import { resetMissingText, setMissingTextSink } from '../shared/missingText.js'

test('the base holds the words of every key foundation-ui needs, in English and Chinese', () => {
  for (const code of ['en', 'zh']) {
    for (const key of [...MODEL_LABEL_KEYS, 'shell.clear']) assert.ok(BASE_LABELS[code][key], `${code} lacks ${key}`)
  }
})

test('a build without the product data draws the base words, not the keys of the list of 主机设置', () => {
  const lines = []
  resetMissingText()
  setMissingTextSink((kind, name, detail) => lines.push(detail))
  provideSettingsData({ searchIndex: [], labels: { en: {} } })
  assert.equal(labelOf('en', 'tabs.general'), 'tabs.general')
  assert.equal(ensureBaseLabels(), true)
  assert.equal(labelOf('en', 'tabs.general'), 'General')
  assert.equal(labelOf('zh', 'tabs.general'), '常规')
  assert.equal(labelOf('de', 'tabs.general'), 'General')
  assert.equal(ensureBaseLabels(), false)
  assert.ok(lines.includes('tabs.general [en]'), 'the gap is a line of the log')
  setMissingTextSink(null)
})

test('the product text wins over the base, and a language of the product keeps its own words', () => {
  provideSettingsData({ searchIndex: [], labels: { en: { 'tabs.general': 'Basics' }, de: { 'tabs.general': 'Allgemein' } } })
  ensureBaseLabels()
  assert.equal(labelOf('en', 'tabs.general'), 'Basics')
  assert.equal(labelOf('de', 'tabs.general'), 'Allgemein')
  assert.equal(labelOf('en', 'tabs.files'), 'Config Files')
})

// With the product's data next to the build (SETTINGS_DATA_DIR: just test, the CI jobs), its English table must hold every key foundation-ui needs
const dir = process.env.SETTINGS_DATA_DIR
test('the pinned settings data of the product has every key foundation-ui needs in English', { skip: !(dir && existsSync(join(dir, 'labels.js'))) }, async () => {
  const mod = await import(pathToFileURL(join(dir, 'labels.js')).href)
  const labels = mod.LABELS || mod.default
  const missing = MODEL_LABEL_KEYS.filter((key) => !labels.en?.[key])
  assert.deepEqual(missing, [])
})
