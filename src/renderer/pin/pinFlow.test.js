import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import {
  ENDED_REASONS, MESSAGES, PAD_CELLS, PIN_LENGTH, PIN_ROWS, classifyAnswer, classifyFailure, endedKey, headingFor, keyAt, movePad, padEdges, stickDirection,
} from './pinFlow.js'

const here = dirname(fileURLToPath(import.meta.url))
const source = (...parts) => readFileSync(join(here, ...parts), 'utf8')

test('the heading names the client the host reports, and does not when it reports none', () => {
  assert.equal(headingFor(MESSAGES.en, 'Living Room TV'), '“Living Room TV” wants to pair')
  assert.equal(headingFor(MESSAGES.zh, '客厅电视'), '“客厅电视”请求配对')
  assert.equal(headingFor(MESSAGES.en, ''), 'A device wants to pair')
  assert.equal(headingFor(MESSAGES.en, '   '), 'A device wants to pair')
  assert.equal(headingFor(MESSAGES.en, undefined), 'A device wants to pair')
  assert.equal(headingFor(MESSAGES.en, 'x'.repeat(200)).length, '“” wants to pair'.length + 64, 'a long name is cut')
})

test('the host answers {"status":"true"} when it paired, anything else is a wrong PIN', () => {
  assert.equal(classifyAnswer({ status: 'true' }), 'ok')
  assert.equal(classifyAnswer({ status: true }), 'ok')
  assert.equal(classifyAnswer({ status: 'True' }), 'ok')
  assert.equal(classifyAnswer({ status: 'false' }), 'wrong')
  assert.equal(classifyAnswer({ status: false }), 'wrong')
  assert.equal(classifyAnswer({}), 'wrong')
  assert.equal(classifyAnswer(null), 'wrong')
  assert.equal(classifyFailure(), 'unreachable')
})

test('an ended pairing says why, and an unknown reason reads as the general one', () => {
  for (const reason of ENDED_REASONS) {
    assert.ok(MESSAGES.en[endedKey(reason)], reason)
    assert.ok(MESSAGES.zh[endedKey(reason)], reason)
  }
  assert.equal(endedKey('something-newer'), 'ended-finished')
  assert.equal(endedKey(undefined), 'ended-finished')
})

test('the reasons are the words the host sends', () => {
  const rust = source('..', '..', '..', 'src-tauri', 'src', 'pin_window.rs')
  const sent = [...rust.matchAll(/Self::\w+ => "([a-z-]+)"/g)].map((match) => match[1])
  assert.deepEqual([...sent].sort(), [...ENDED_REASONS].sort())
  const event = /PAIRING_ENDED_EVENT: &str = "([^"]+)"/.exec(rust)?.[1]
  assert.ok(event && source('PinApp.vue').includes(`'${event}'`), 'the page listens for the event the host emits')
  const label = /PIN_WINDOW_ID: &str = "([^"]+)"/.exec(rust)?.[1]
  assert.ok(label)
})

test('every message exists in both languages', () => {
  assert.deepEqual(Object.keys(MESSAGES.zh).sort(), Object.keys(MESSAGES.en).sort())
  for (const language of Object.values(MESSAGES)) for (const value of Object.values(language)) assert.ok(String(value).trim())
})

test('a controller moves over the keys that can be pressed and stops at the edges', () => {
  assert.equal(PIN_LENGTH, 4)
  assert.equal(PAD_CELLS.length, 11, 'ten digits and the backspace')
  assert.deepEqual(movePad([1, 1], 'up'), [0, 1])
  assert.equal(keyAt(movePad([1, 1], 'down')), '8')
  assert.equal(keyAt(movePad([2, 1], 'down')), '0')
  assert.deepEqual(movePad([0, 0], 'up'), [0, 0])
  assert.deepEqual(movePad([0, 0], 'left'), [0, 0])
  assert.deepEqual(movePad([0, 2], 'right'), [0, 2])
  // the empty cell next to the zero is no key: the cursor stays on the zero
  assert.deepEqual(movePad([3, 1], 'right'), [3, 1])
  assert.deepEqual(movePad([2, 2], 'down'), [2, 2], 'below the nine there is no key')
  assert.equal(keyAt(movePad([3, 1], 'left')), 'backspace')
  assert.deepEqual(movePad([1, 1], 'sideways'), [1, 1])
})

test('every pressable cell is reachable from the middle of the pad', () => {
  const seen = new Set(['1,1'])
  const queue = [[1, 1]]
  while (queue.length) {
    const cell = queue.shift()
    for (const direction of ['up', 'down', 'left', 'right']) {
      const next = movePad(cell, direction)
      if (!seen.has(String(next))) { seen.add(String(next)); queue.push(next) }
    }
  }
  assert.equal(seen.size, PAD_CELLS.length)
  assert.equal(PIN_ROWS.flat().filter(Boolean).length, PAD_CELLS.length)
})

test('only a new press of a button counts, so a held d-pad moves one key', () => {
  const idle = Array(17).fill(false)
  const press = (...indexes) => idle.map((_, i) => indexes.includes(i))
  assert.deepEqual(padEdges(idle, press(13)), ['down'])
  assert.deepEqual(padEdges(press(13), press(13)), [])
  assert.deepEqual(padEdges(press(13), idle), [])
  assert.deepEqual(padEdges(idle, press(0)), ['press'])
  assert.deepEqual(padEdges(idle, press(1)), ['cancel'])
  assert.deepEqual(padEdges([], press(12, 15)), ['up', 'right'])
  assert.deepEqual(padEdges(idle, press(3)), [], 'the face buttons that do nothing here stay quiet')
})

test('the left stick is a d-pad past its dead zone', () => {
  assert.equal(stickDirection(0, 0), null)
  assert.equal(stickDirection(0.3, 0.3), null)
  assert.equal(stickDirection(0.9, 0.1), 'right')
  assert.equal(stickDirection(-0.9, 0.2), 'left')
  assert.equal(stickDirection(0.1, 0.9), 'down')
  assert.equal(stickDirection(0.1, -0.9), 'up')
})

test('the window page is built with the panel: an entry of the build and a page the host opens', () => {
  assert.ok(source('..', '..', '..', 'vite.config.js').includes("pin: resolve(rendererSrcPath, 'pin/index.html')"))
  assert.ok(source('..', '..', '..', 'src-tauri', 'src', 'pin_window.rs').includes('"pin/index.html"'))
})
