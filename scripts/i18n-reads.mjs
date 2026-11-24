// The part of scripts/check-i18n-keys.mjs that reads source text: which keys of the desktop table a file reads, and whether English has them.
// Separate from the script so a test can hand it a few lines of source.

const isObject = (v) => !!v && typeof v === 'object' && !Array.isArray(v)

export const STRING_OR_ARRAY_MEMBERS = new Set(['length', 'map', 'filter', 'join', 'replace', 'replaceAll', 'split', 'trim', 'slice', 'forEach', 'find', 'some', 'every', 'includes', 'toString', 'concat', 'at', 'entries', 'keys', 'values', 'startsWith', 'endsWith', 'indexOf', 'toLowerCase', 'toUpperCase', 'flatMap', 'reduce', 'padStart', 'padEnd', 'localeCompare'])

/** The node of the English table a chain of members reaches: 'ok', or the first member English lacks. */
export function resolve(table, members) {
  let node = table
  for (const member of members) {
    if (Array.isArray(node)) {
      // an index the array does not have (`steps.3` of a list of three) is a text English lacks
      if (/^\d+$/.test(member)) { if (!(Number(member) in node)) return member; node = node[member]; continue }
      return STRING_OR_ARRAY_MEMBERS.has(member) ? 'ok' : member
    }
    if (isObject(node)) {
      if (!Object.hasOwn(node, member)) return member
      node = node[member]
      continue
    }
    return 'ok' // a text: what follows is a String member
  }
  return 'ok'
}

/** A read of a top-level section of the table through `t` (`t.m3e`, `t.value.deviceHub`): the sign of a file that is handed the table without calling useI18n itself. */
export const tableParameterPattern = (topKeys) => new RegExp(`(?<![\\w.$])t(?:\\??\\.value)?\\??\\.(?:${topKeys.map((k) => k.replace(/[^\w]/g, '')).join('|')})\\b`)

/**
 * The reads of the desktop table in one file: `{ index, path }` for every chain such as `t.value.deviceHub.usb.restartPending`, `text.sidebar.title` (a name bound to a part of
 * the table) or `c.input.gamepad` in a module that is handed the table as `t` (`pageItems(t, rail)`). `handedTable`: the pattern of tableParameterPattern, or null where a
 * file may not be read that way.
 */
export function findReads(src, { handedTable = null } = {}) {
  const m = src.match(/const\s*\{([^}]*)\}\s*=\s*useI18n\(\)/)
  let tName = ''
  if (m) {
    const alias = /(?:^|,)\s*t(?:\s*:\s*(\w+))?\s*(?=,|$)/.exec(m[1].replace(/\s+/g, ' ').trim())
    if (!alias) return []
    tName = alias[1] || 't'
  } else if (handedTable && handedTable.test(src)) {
    tName = 't'
  } else return []
  // names bound to a part of the table: `const text = computed(() => t.value.pinEntry)`
  const roots = new Map([[tName, []]])
  for (let pass = 0; pass < 3; pass++) {
    const bind = /(?:const|let)\s+(\w+)\s*=\s*(?:computed\(\s*\(\)\s*=>\s*)?(\w+)(?:\.value)?((?:\??\.\w+)+)\s*\)?\s*(?:$|;|\n)/g
    for (const b of src.matchAll(bind)) {
      const base = roots.get(b[2])
      if (!base) continue
      const members = b[3].split(/\??\./).filter(Boolean).filter((x) => x !== 'value')
      if (!roots.has(b[1])) roots.set(b[1], [...base, ...members])
    }
  }
  const out = []
  for (const [name, prefix] of roots) {
    const use = new RegExp(`(?<![\\w.$])${name}(?:\\??\\.value)?((?:\\??\\.[A-Za-z_]\\w*)+)`, 'g')
    for (const u of src.matchAll(use)) {
      const members = u[1].split(/\??\./).filter(Boolean)
      // the chain stops where the table stops: members of the bound name that are not table members (value) are skipped
      if (members[0] === 'value') members.shift()
      if (prefix.length === 0 && members.length === 0) continue
      // a callback parameter of the same name (`t => t.id`) is not the table
      const lineText = src.slice(src.lastIndexOf('\n', u.index) + 1, src.indexOf('\n', u.index))
      if (new RegExp(`(?:\\(|,\\s*|\\b)${name}\\s*(?:,[^)]*)?\\)?\\s*=>`).test(lineText)) continue
      out.push({ index: u.index, path: [...prefix, ...members] })
    }
  }
  return out
}
