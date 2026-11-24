import test from 'node:test'
import assert from 'node:assert/strict'
import { noteMissingText, resetMissingText, setMissingTextSink } from './missingText.js'

test('a missing text is written once per key and language, with the key and the language only', () => {
  const lines = []
  resetMissingText()
  setMissingTextSink((kind, name, detail) => lines.push([kind, name, detail]))
  assert.equal(noteMissingText('settings-label', 'tabs.display', 'de'), true)
  assert.equal(noteMissingText('settings-label', 'tabs.display', 'de'), false)
  assert.equal(noteMissingText('settings-label', 'tabs.display', 'fr'), true)
  assert.deepEqual(lines, [
    ['i18n', 'missing-text settings-label', 'tabs.display [de]'],
    ['i18n', 'missing-text settings-label', 'tabs.display [fr]'],
  ])
  setMissingTextSink(null)
})

test('a key that is not a key (a sentence, a path of the person) is not written', () => {
  const lines = []
  resetMissingText()
  setMissingTextSink((kind, name, detail) => lines.push(detail))
  noteMissingText('dualsense', 'C:\\Users\\someone\\Documents', 'en')
  noteMissingText('dualsense', 'Please sign in as someone', 'en')
  assert.deepEqual(lines, ['(unreadable key) [en]'])
  setMissingTextSink(null)
})

test('the number of lines of one window is limited', () => {
  let count = 0
  resetMissingText()
  setMissingTextSink(() => { count += 1 })
  for (let i = 0; i < 200; i++) noteMissingText('welcome', `key_${i}`, 'en')
  assert.equal(count, 50)
  setMissingTextSink(null)
  resetMissingText()
})
