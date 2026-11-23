// The full-screen PIN entry (src-tauri/src/pin_window.rs): what it says, what the host's answer means, and how a controller moves over the keypad. No DOM and no
// framework, so the tests run it as it is.
export const PIN_LENGTH = 4
/** After a pairing the window stays this long, so the person sees that it worked. */
export const CLOSE_AFTER_SUCCESS_MS = 1600
/** After the host says that pairing is over the page says so, then closes itself (the host closes it at 3 s if the page has not). */
export const CLOSE_AFTER_ENDED_MS = 1800
/** The reasons pin_window.rs sends with `pin-pairing-ended` (EndReason::as_str). */
export const ENDED_REASONS = Object.freeze(['finished', 'no-request', 'host-lost'])

export const MESSAGES = {
  zh: {
    title: 'PIN 配对',
    named: '“{name}”请求配对',
    unnamed: '有设备请求配对',
    hint: '输入客户端上显示的 4 位 PIN',
    cancel: '取消',
    preparing: '正在连接主机…',
    pairing: '正在配对…',
    done: '配对成功',
    wrong: 'PIN 不对，请再试一次',
    unreachable: '连不上主机，请确认 Sunshine 正在运行',
    failed: '配对没有成功，请再试一次',
    'ended-finished': '这个配对请求已经结束',
    'ended-no-request': '现在没有等待配对的请求',
    'ended-host-lost': '主机没有响应',
    backspace: '删除',
  },
  en: {
    title: 'PIN pairing',
    named: '“{name}” wants to pair',
    unnamed: 'A device wants to pair',
    hint: 'Enter the 4-digit PIN shown on the client',
    cancel: 'Cancel',
    preparing: 'Connecting to the host…',
    pairing: 'Pairing…',
    done: 'Paired',
    wrong: 'That PIN is not right. Try again.',
    unreachable: 'Cannot reach the host. Make sure Sunshine is running.',
    failed: 'Pairing did not work. Try again.',
    'ended-finished': 'This pairing request is over',
    'ended-no-request': 'No pairing request is waiting',
    'ended-host-lost': 'The host is not responding',
    backspace: 'Delete',
  },
}

/** The heading: the client's name when the host reports it. */
export function headingFor(text, clientName) {
  const name = String(clientName || '').trim().slice(0, 64)
  return name ? text.named.replace('{name}', name) : text.unnamed
}

/** What the host's answer to `POST /api/pin` is: 'ok' when it paired, 'wrong' when it did not take the PIN. */
export function classifyAnswer(data) {
  return String(data && data.status).toLowerCase() === 'true' ? 'ok' : 'wrong'
}

/** A failed request is the host not being there (the page cannot say more than that). */
export const classifyFailure = () => 'unreachable'

/** The message key of an ended pairing; an unknown reason reads as the general one. */
export function endedKey(reason) {
  return `ended-${ENDED_REASONS.includes(reason) ? reason : 'finished'}`
}

/** The rows of MPinPad's keypad (foundation-ui vue/pinpad.js PIN_ROWS), kept here so that this file runs in node without the UI library; PinApp checks the pad it mounted against it. */
export const PIN_ROWS = Object.freeze([['1', '2', '3'], ['4', '5', '6'], ['7', '8', '9'], ['backspace', '0', '']])

/** The cells of the keypad that can be pressed, as [row, col]: the empty cell of the last row is not one. */
export const PAD_CELLS = Object.freeze(PIN_ROWS.flatMap((row, r) => row.map((key, c) => (key ? [r, c] : null)).filter(Boolean)))

/** The cell a direction of the pad leads to from [row, col]: the next pressable cell that way, the same one at the edge. */
export function movePad([row, col], direction) {
  const step = { up: [-1, 0], down: [1, 0], left: [0, -1], right: [0, 1] }[direction]
  if (!step) return [row, col]
  let [r, c] = [row + step[0], col + step[1]]
  // the empty cell is skipped along the row; up and down stay in their column
  while (r >= 0 && r < PIN_ROWS.length && c >= 0 && c < PIN_ROWS[0].length) {
    if (PIN_ROWS[r][c]) return [r, c]
    r += step[0]; c += step[1]
  }
  return [row, col]
}

/** The key a cell of the pad is: a digit, or 'backspace'. */
export const keyAt = ([row, col]) => PIN_ROWS[row][col]

// the standard layout of a controller (Gamepad API, "Standard Gamepad"): A, B and the d-pad
const BUTTON_A = 0, BUTTON_B = 1, DPAD = { 12: 'up', 13: 'down', 14: 'left', 15: 'right' }

/**
 * What a controller did between two readings: the pressed buttons of the standard layout, as a boolean array each. Only a press (not held) counts, so a held d-pad
 * moves one cell. Returns a list of 'up' | 'down' | 'left' | 'right' | 'press' | 'cancel'.
 */
export function padEdges(before, now) {
  const out = []
  const edge = (i) => !!now[i] && !before[i]
  for (const [index, direction] of Object.entries(DPAD)) if (edge(Number(index))) out.push(direction)
  if (edge(BUTTON_A)) out.push('press')
  if (edge(BUTTON_B)) out.push('cancel')
  return out
}

/** The left stick as a d-pad: past the dead zone, the stronger axis. null inside it. */
export function stickDirection(x, y, deadZone = 0.55) {
  if (Math.hypot(x, y) < deadZone) return null
  return Math.abs(x) > Math.abs(y) ? (x > 0 ? 'right' : 'left') : (y > 0 ? 'down' : 'up')
}
