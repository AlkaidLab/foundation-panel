import { test } from 'node:test'
import assert from 'node:assert/strict'
import { findReads, resolve, tableParameterPattern } from './i18n-reads.mjs'

const en = { m3e: { nav: { input: 'Input', audio: 'Audio' }, input: { gamepad: 'Gamepad' } }, dualSense: { status: { install: { steps: ['a', 'b', 'c'] } } } }

test('an index an array does not have is a text English lacks; a valid index and the array members pass', () => {
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', '2']), 'ok')
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', '3']), '3')
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', 'length']), 'ok')
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', 'map']), 'ok')
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', 'nope']), 'nope')
  assert.equal(resolve(en, ['dualSense', 'status', 'install', 'steps', '2', 'length']), 'ok')
})

test('a module that is handed the table as `t` is read like a window that calls useI18n', () => {
  const handed = tableParameterPattern(Object.keys(en))
  const src = [
    'export function pageItems(t, rail) {',
    '  const c = t.m3e',
    '  return [{ label: c.input.gamepad }, { label: c.input.nope }, { label: t.m3e.nav.audio }]',
    '}',
  ].join('\n')
  const reads = findReads(src, { handedTable: handed }).map((r) => r.path.join('.'))
  assert.ok(reads.includes('m3e.input.gamepad'))
  assert.ok(reads.includes('m3e.nav.audio'))
  const missing = reads.filter((p) => resolve(en, p.split('.')) !== 'ok')
  assert.deepEqual(missing, ['m3e.input.nope'])
})

test('a file that is not handed a table, or is not allowed to be read that way, is not read', () => {
  const src = 'const c = t.m3e\nconst x = c.input.nope'
  assert.deepEqual(findReads(src, {}), [])
  assert.deepEqual(findReads('const y = other.m3e', { handedTable: tableParameterPattern(['m3e']) }), [])
})

test('a callback parameter named t is not the table', () => {
  const src = 'const { t } = useI18n()\nrows.map((t) => t.id)\nconst a = t.value.m3e.nav.input'
  assert.deepEqual(findReads(src).map((r) => r.path.join('.')), ['m3e.nav.input'])
})
