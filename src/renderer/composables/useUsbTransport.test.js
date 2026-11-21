import assert from 'node:assert/strict'
import test from 'node:test'
import 'vue'

globalThis.localStorage ??= { getItem: () => null, setItem: () => {}, removeItem: () => {} }
globalThis.document ??= { documentElement: {} }
globalThis.window ??= {
  addEventListener() {},
  removeEventListener() {},
  dispatchEvent() {},
  matchMedia: () => ({ addEventListener() {}, matches: false }),
  __TAURI_INTERNALS__: { transformCallback: () => 1, invoke: async () => ({}) },
}

const { emitUsbipErrorReport, formatUsbipFriendlyError, usbipErrorReportCode } = await import('./useUsbTransport.js')

test('usbipErrorReportCode extracts structured codes and subcodes without user text', () => {
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-016: Windows driver policy blocked (exit code 1) [!!! Error 0xe0000247 in C:\\Users\\Alice\\file]'),
    'usbip-setup-016.0xe0000247',
  )
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-010: the USB/IP installer failed with exit code 1 [pnputil failed]'),
    'usbip-setup-010.exit-1',
  )
  assert.equal(
    usbipErrorReportCode('USBIP-SETUP-007: transport installation failed: USBIP-EXEC-006: hash mismatch'),
    'usbip-exec-006',
  )
  assert.equal(usbipErrorReportCode('unexpected failure with C:\\Users\\secret'), 'usbip-unknown')
})

test('formatUsbipFriendlyError unwraps USBIP-SETUP-007 to the underlying specific error code', () => {
  const usbTexts = {
    unknownError: 'fallback',
    errors: {
      'USBIP-SETUP-007': 'generic install failed',
      'USBIP-SETUP-015': 'reboot pending',
      'USBIP-EXEC-006': 'payload mismatch',
    },
  }
  assert.equal(
    formatUsbipFriendlyError('USBIP-SETUP-007: transport installation failed: USBIP-EXEC-006: missing', usbTexts),
    'payload mismatch',
  )
  assert.equal(
    formatUsbipFriendlyError('USBIP-SETUP-015: a Windows restart is pending', usbTexts),
    'reboot pending',
  )
  assert.equal(formatUsbipFriendlyError('USBIP-SETUP-007: raw error', usbTexts), 'generic install failed')
  assert.equal(formatUsbipFriendlyError('other', usbTexts), 'fallback')
})

test('emitUsbipErrorReport sends only component, code and severity', () => {
  const sent = []
  const code = emitUsbipErrorReport(
    'USBIP-SETUP-015: pending restart (exit code 3010) [C:\\Users\\bob\\secret]',
    (event) => sent.push(event),
  )
  assert.equal(code, 'usbip-setup-015.exit-3010')
  assert.deepEqual(sent, [{ component: 'usbip', code: 'usbip-setup-015.exit-3010', severity: 'error' }])
})
