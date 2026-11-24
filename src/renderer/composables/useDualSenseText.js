import { computed } from 'vue'
import { useI18n } from '../desktop/i18n/index.js'
import { formatText } from './dualsenseText.js'
import { noteMissingText } from '../shared/missingText.js'

const lookup = (root, path) => String(path).split('.').reduce((node, part) => node?.[part], root)

export function useDualSenseText() {
  const { t, locale } = useI18n()
  const text = computed(() => t.value.dualSense)
  // A path no table has (a state a newer host reports) is a line of the log and an empty sentence, never the path itself on screen.
  const sentence = (root, path, params) => {
    const found = lookup(root, path)
    if (typeof found === 'string' && found !== '') return formatText(found, params)
    noteMissingText('dualsense', path, locale.value)
    return ''
  }
  /** Sentence of the status card and error maps: `status.states.ready`. */
  const tx = (path, params) => sentence(text.value, path, params)
  /** Sentence of the tuning page: `presets.title` under `dualSense.tuning`. */
  const tr = (path, params) => sentence(text.value.tuning, path, params)
  return { text, tx, tr, locale }
}
