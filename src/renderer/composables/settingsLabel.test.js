import test from 'node:test'
import assert from 'node:assert/strict'
import { provideSettingsData } from 'foundation-ui/settings/provider.js'
import { resetMissingText, setMissingTextSink } from '../shared/missingText.js'
import { settingsLabel } from './settingsLabel.js'

test('a label the tables have is the table text, in the language, then English', () => {
  provideSettingsData({ searchIndex: [], labels: { en: { 'shell.appearance': 'Appearance', 'tabs.display': 'Display' }, de: { 'shell.appearance': 'Darstellung' } } })
  assert.equal(settingsLabel('de', 'shell.appearance', 'x'), 'Darstellung')
  assert.equal(settingsLabel('de', 'tabs.display', 'x'), 'Display')
})

test('a label no table has is the fallback text, never the key, and the gap is written once', () => {
  const lines = []
  resetMissingText()
  setMissingTextSink((kind, name, detail) => lines.push(detail))
  provideSettingsData({ searchIndex: [], labels: { en: {} } })
  assert.equal(settingsLabel('en', 'shell.appearance', 'Appearance'), 'Appearance')
  assert.equal(settingsLabel('en', 'shell.appearance', 'Appearance'), 'Appearance')
  assert.equal(settingsLabel('en', 'tabs.audio'), '')
  assert.deepEqual(lines, ['shell.appearance [en]', 'tabs.audio [en]'])
  setMissingTextSink(null)
})

test('an empty text of the product is no label: the fallback is drawn, in the language and in English', () => {
  resetMissingText()
  setMissingTextSink(() => {})
  provideSettingsData({ searchIndex: [], labels: { en: { 'tabs.general': '' }, de: { 'tabs.general': '' } } })
  assert.equal(settingsLabel('de', 'tabs.general', 'General'), 'General')
  assert.equal(settingsLabel('en', 'tabs.general', 'General'), 'General')
  setMissingTextSink(null)
})
