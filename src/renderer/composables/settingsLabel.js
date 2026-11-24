import { labelOf } from 'foundation-ui/settings/index.js'
import { noteMissingText } from '../shared/missingText.js'

/**
 * The label of a settings key of the product's tables (`shell.appearance`, `tabs.display`). foundation-ui answers the key itself when no table
 * has it (a build without the product's settings data, a key a newer host added): the panel draws `fallback` instead (a text of its own
 * language tables, or nothing) and writes the gap to its log.
 */
export function settingsLabel(locale, key, fallback = '') {
  const text = labelOf(locale, key)
  if (text && text !== key) return text
  noteMissingText('settings-label', key, locale)
  return fallback
}
