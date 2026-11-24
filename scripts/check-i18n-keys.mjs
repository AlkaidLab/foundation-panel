#!/usr/bin/env node
// Checks the text tables of the panel so that no window can show a raw key.
//
//   1. Base language: the keys the code reads (desktop `t.value.a.b`, the welcome page `t('key')`, the DualSense `tx('a.b')`)
//      must exist in English, the table every other language falls back to.
//   2. Chinese is the second base: it holds exactly the keys of English.
//   3. Every other language holds the keys of English, except the ones listed in scripts/i18n-untranslated.json (they show the
//      English text on purpose until translated). A new key that a language lacks, or a listed key that is complete in every
//      language, fails the check, so the list only shrinks.
//   4. The stylus plug-in table (native-plugins/stylus/i18n.json): every key the Rust code asks for exists in English.
//
// Run: node scripts/check-i18n-keys.mjs   (exit 1 on a failure; `--update-untranslated` rewrites the list)
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const DESKTOP = path.join(root, 'src/renderer/desktop/i18n')
const WELCOME = path.join(root, 'src/i18n/locales')
const STYLUS = path.join(root, 'native-plugins/stylus')
const LIST = path.join(root, 'scripts/i18n-untranslated.json')
const update = process.argv.includes('--update-untranslated')

const failures = []
const fail = (msg) => failures.push(msg)

const isObject = (v) => !!v && typeof v === 'object' && !Array.isArray(v)
function leaves(node, trail = [], out = new Map()) {
  if (Array.isArray(node)) node.forEach((item, i) => leaves(item, [...trail, i], out))
  else if (isObject(node)) for (const key of Object.keys(node)) leaves(node[key], [...trail, key], out)
  else out.set(trail.join('.'), node)
  return out
}
const present = (v) => v !== undefined && v !== null && v !== ''

async function loadTables(dir, pick) {
  const codes = fs.readdirSync(dir).filter((f) => /^[a-z]{2}(_[A-Z]{2})?\.js$/.test(f)).map((f) => f.slice(0, -3)).sort()
  const tables = {}
  for (const code of codes) {
    const mod = await import(pathToFileURL(path.join(dir, `${code}.js`)).href)
    tables[code] = pick(mod, code)
  }
  return tables
}

// --- completeness of the language tables -------------------------------------------------------------------------------
const desktop = await loadTables(DESKTOP, (mod, code) => mod[code])
const welcome = await loadTables(WELCOME, (mod) => mod.default)
const desktopLeaves = Object.fromEntries(Object.entries(desktop).map(([c, t]) => [c, leaves(t)]))
const welcomeLeaves = Object.fromEntries(Object.entries(welcome).map(([c, t]) => [c, leaves(t)]))

function checkBases(label, byCode) {
  const en = byCode.en
  const zh = byCode.zh
  for (const key of en.keys()) if (!present(en.get(key))) fail(`${label}: English has an empty text for "${key}"`)
  for (const key of en.keys()) if (!present(zh.get(key))) fail(`${label}: Chinese lacks "${key}" that English has`)
  for (const key of zh.keys()) if (!en.has(key)) fail(`${label}: English lacks "${key}" that Chinese has`)
}
checkBases('desktop', desktopLeaves)
checkBases('welcome', welcomeLeaves)

// welcome tables are small and complete: no language may lack a key
for (const [code, table] of Object.entries(welcomeLeaves)) {
  for (const key of welcomeLeaves.en.keys()) if (!present(table.get(key))) fail(`welcome: ${code} lacks "${key}"`)
}

// desktop: the languages other than English and Chinese fall back to English per key; a gap has to be listed
const listed = new Set(fs.existsSync(LIST) ? JSON.parse(fs.readFileSync(LIST, 'utf8')) : [])
const gaps = new Map()
for (const [code, table] of Object.entries(desktopLeaves)) {
  if (code === 'en' || code === 'zh') continue
  for (const key of desktopLeaves.en.keys()) {
    if (present(table.get(key))) continue
    // zh_TW reads zh before English, and zh is complete, so a gap of zh_TW is never shown as English
    if (code === 'zh_TW') continue
    if (!gaps.has(key)) gaps.set(key, [])
    gaps.get(key).push(code)
  }
}
if (update) {
  fs.writeFileSync(LIST, `${JSON.stringify([...gaps.keys()].sort(), null, 2)}\n`)
  console.log(`wrote ${LIST} (${gaps.size} keys)`)
} else {
  for (const [key, codes] of gaps) {
    if (!listed.has(key)) fail(`desktop: "${key}" is missing in ${codes.join(', ')}; translate it or list it in scripts/i18n-untranslated.json`)
  }
  for (const key of listed) {
    if (!desktopLeaves.en.has(key)) fail(`desktop: scripts/i18n-untranslated.json lists "${key}", which English no longer has`)
    else if (!gaps.has(key)) fail(`desktop: "${key}" is translated everywhere; remove it from scripts/i18n-untranslated.json`)
  }
}

// --- keys the code reads ------------------------------------------------------------------------------------------------
const walk = (dir, out = []) => {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === 'node_modules' || entry.name === 'dist') continue
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) walk(full, out)
    else if (/\.(vue|js|mjs)$/.test(entry.name) && !/\.test\.(js|mjs)$/.test(entry.name)) out.push(full)
  }
  return out
}
const lineOf = (src, index) => src.slice(0, index).split('\n').length
const rel = (file) => path.relative(root, file).split(path.sep).join('/')
const STRING_OR_ARRAY_MEMBERS = new Set(['length', 'map', 'filter', 'join', 'replace', 'replaceAll', 'split', 'trim', 'slice', 'forEach', 'find', 'some', 'every', 'includes', 'toString', 'concat', 'at', 'entries', 'keys', 'values', 'startsWith', 'endsWith', 'indexOf', 'toLowerCase', 'toUpperCase', 'flatMap', 'reduce', 'padStart', 'padEnd', 'localeCompare'])

/** The node of the English table a chain of members reaches: 'ok', or the first member English lacks. */
function resolve(table, members) {
  let node = table
  for (const member of members) {
    if (Array.isArray(node)) {
      if (/^\d+$/.test(member) || STRING_OR_ARRAY_MEMBERS.has(member)) { node = /^\d+$/.test(member) ? node[member] : undefined; if (node === undefined) return 'ok'; continue }
      return member
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

// the table a window reads for English: the file plus the USB/IP texts the entry adds (usbipOwnership.js)
const { withUsbipOwnership } = await import(pathToFileURL(path.join(DESKTOP, 'usbipOwnership.js')).href)
const en = withUsbipOwnership(desktop.en, 'en')
const sources = walk(path.join(root, 'src'))
let reads = 0
let checked = 0
for (const file of sources) {
  if (file.startsWith(DESKTOP)) continue
  const src = fs.readFileSync(file, 'utf8')
  // the name the window gives the table of the desktop i18n
  const m = src.match(/const\s*\{([^}]*)\}\s*=\s*useI18n\(\)/)
  if (!m) continue
  const alias = /(?:^|,)\s*t(?:\s*:\s*(\w+))?\s*(?=,|$)/.exec(m[1].replace(/\s+/g, ' ').trim())
  if (!alias) continue
  const tName = alias[1] || 't'
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
  for (const [name, prefix] of roots) {
    const use = new RegExp(`(?<![\\w.$])${name}(?:\\??\\.value)?((?:\\??\\.[A-Za-z_]\\w*)+)`, 'g')
    for (const u of src.matchAll(use)) {
      const members = u[1].split(/\??\./).filter(Boolean)
      // the chain stops where the table stops: members of the bound name that are not table members (value) are skipped
      if (members[0] === 'value') members.shift()
      if (prefix.length === 0 && members.length === 0) continue
      const missing = resolve(en, [...prefix, ...members])
      // a callback parameter of the same name (`t => t.id`) is not the table
      const lineText = src.slice(src.lastIndexOf('\n', u.index) + 1, src.indexOf('\n', u.index))
      if (new RegExp(`(?:\\(|,\\s*|\\b)${name}\\s*(?:,[^)]*)?\\)?\\s*=>`).test(lineText)) continue
      checked += 1
      if (missing !== 'ok') {
        reads += 1
        fail(`desktop: ${rel(file)}:${lineOf(src, u.index)} reads "${[...prefix, ...members].join('.')}" but English has no "${missing}"`)
      }
    }
  }
}

// the DualSense sentences are asked by a dotted path: tx('status.states.ready') / tr('presets.title')
const dualSense = en.dualSense
for (const file of sources) {
  if (file.startsWith(DESKTOP)) continue
  const src = fs.readFileSync(file, 'utf8')
  if (!/useDualSenseText\(/.test(src)) continue
  for (const call of src.matchAll(/(?<![\w.])(tx|tr)\(\s*(['"`])([^'"`$]+)\2/g)) {
    const base = call[1] === 'tx' ? dualSense : dualSense.tuning
    const missing = resolve(base, call[3].split('.'))
    if (missing !== 'ok') fail(`desktop: ${rel(file)}:${lineOf(src, call.index)} asks ${call[1]}('${call[3]}') but English has no "${missing}"`)
  }
}

// the welcome page and the frame ask for flat keys: t('key') of src/i18n
for (const file of sources) {
  if (file.startsWith(WELCOME) || file.startsWith(DESKTOP)) continue
  const src = fs.readFileSync(file, 'utf8')
  if (!/from\s+['"][./]*i18n\/index\.js['"]/.test(src) && !/useWelcomeI18n/.test(src)) continue
  if (/useI18n\(\)/.test(src) && !/useWelcomeI18n/.test(src)) continue
  for (const call of src.matchAll(/(?<![\w.$])t\(\s*(['"])([A-Za-z0-9_]+)\1/g)) {
    if (!welcomeLeaves.en.has(call[2])) fail(`welcome: ${rel(file)}:${lineOf(src, call.index)} asks t('${call[2]}') but English has no such key`)
  }
}

// --- the stylus plug-in --------------------------------------------------------------------------------------------------
try {
  const stylus = JSON.parse(fs.readFileSync(path.join(STYLUS, 'i18n.json'), 'utf8'))
  const rs = fs.readFileSync(path.join(STYLUS, 'src/lib.rs'), 'utf8')
  for (const call of rs.matchAll(/\btr\("([a-z0-9_.]+)"\)/g)) {
    if (!Object.hasOwn(stylus.en, call[1])) fail(`stylus: native-plugins/stylus/src/lib.rs:${lineOf(rs, call.index)} asks tr("${call[1]}") but English has no such key`)
  }
  for (const code of Object.keys(stylus)) for (const key of Object.keys(stylus[code])) if (!Object.hasOwn(stylus.en, key)) fail(`stylus: ${code} has "${key}" that English lacks`)
} catch (error) {
  fail(`stylus: could not be checked (${error.message})`)
}

if (failures.length) {
  console.error(`i18n key check failed (${failures.length}):`)
  for (const line of failures) console.error(`  - ${line}`)
  process.exit(1)
}
console.log(`i18n key check passed: ${desktopLeaves.en.size} desktop keys in ${Object.keys(desktop).length} languages, ${welcomeLeaves.en.size} welcome keys, ${checked} reads of the code checked`)
