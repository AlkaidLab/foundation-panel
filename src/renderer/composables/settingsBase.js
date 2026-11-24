// The words the panel's own lists need of the product's settings tables (the categories of 主机设置, its pages, 个性化, the recent list), in English and Chinese.
// The product's data (Foundation Sunshine's settingsData, read at build time) holds them in 20 languages; foundation-ui answers a label's key itself when no table
// has it, which is how the list of 主机设置 of v1.0.19 showed `tabs.general`, `tabs.files` ... when that data was not in the build. These are the base the tables
// fall back to: the product's own text wins, a language that has no text for a key reads English, and a key that no table has never reaches the screen.
import { MODEL_LABEL_KEYS } from 'foundation-ui/settings/model.js'
import { getAliases, getLabels, getSearchIndex, provideSettingsData } from 'foundation-ui/settings/provider.js'
import { noteMissingText } from '../shared/missingText.js'

export const BASE_LABELS = Object.freeze({
  en: Object.freeze({
    'tabs.general': 'General',
    'tabs.files': 'Config Files',
    'tabs.input': 'Input',
    'tabs.network': 'Network',
    'shell.appearance': 'Appearance',
    'tabs.display': 'Display & video',
    'tabs.audio': 'Audio',
    'navbar.security': 'Security & sign-in',
    'shell.display_output': 'Displays',
    'shell.display_modes': 'Resolution & refresh rate',
    'shell.display_hdr': 'HDR',
    'config.hdr_enhanced': 'Image enhancement',
    'shell.display_encoders': 'Encoding & bitrate',
    'shell.virtual_display': 'Foundation Virtual Display',
    'shell.display_advanced': 'Advanced',
    'shell.audio_output': 'Audio output',
    'shell.microphone': 'Microphone',
    'shell.gamepad': 'Gamepad',
    'config.input_keyboard_title': 'Keyboard',
    'config.input_mouse_title': 'Mouse and touch',
    'shell.usb_forwarding': 'USB forwarding',
    'config.input_gamepad_title': 'Gamepad',
    'navbar.configuration': 'Configuration',
    'navbar.dualsense': 'DualSense',
    'shell.recent': 'Recently used',
    'shell.clear': 'Clear',
  }),
  zh: Object.freeze({
    'tabs.general': '常规',
    'tabs.files': '配置文件',
    'tabs.input': '输入',
    'tabs.network': '网络',
    'shell.appearance': '个性化',
    'tabs.display': '显示与视频',
    'tabs.audio': '音频',
    'navbar.security': '安全与登录',
    'shell.display_output': '显示器',
    'shell.display_modes': '分辨率与刷新率',
    'shell.display_hdr': 'HDR',
    'config.hdr_enhanced': '画质增强',
    'shell.display_encoders': '编码与码率',
    'shell.virtual_display': '基地虚拟显示器',
    'shell.display_advanced': '高级',
    'shell.audio_output': '音频输出',
    'shell.microphone': '麦克风',
    'shell.gamepad': '手柄',
    'config.input_keyboard_title': '键盘',
    'config.input_mouse_title': '鼠标与触控',
    'shell.usb_forwarding': 'USB 转发',
    'config.input_gamepad_title': '手柄',
    'navbar.configuration': '配置',
    'navbar.dualsense': 'DualSense 调校',
    'shell.recent': '最近使用',
    'shell.clear': '清除',
  }),
})

/** The tables with the base underneath: for English and Chinese a key the product lacks reads the base; every other table is the product's (it falls back to English). */
export function withBaseLabels(labels) {
  const out = { ...labels }
  for (const code of ['en', 'zh']) out[code] = { ...BASE_LABELS[code], ...labels?.[code] }
  return out
}

/** Puts the base under the tables the product supplied (idempotent); writes the keys of the model the product's English table lacks to the log. Call after the product's data is supplied. */
export function ensureBaseLabels() {
  const labels = getLabels()
  for (const key of MODEL_LABEL_KEYS) if (!labels?.en?.[key]) noteMissingText('settings-data', key, 'en')
  const complete = ['en', 'zh'].every((code) => Object.keys(BASE_LABELS[code]).every((key) => labels?.[code]?.[key]))
  if (complete) return false
  provideSettingsData({ searchIndex: getSearchIndex(), labels: withBaseLabels(labels), aliases: getAliases() })
  return true
}
