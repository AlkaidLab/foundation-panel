<template>
  <MSection v-if="platform === 'windows'" :title="text.wiredTitle" :description="text.wiredHint">
    <MSkeleton v-if="loading" :rows="1" role="status" :aria-label="text.loadingConfig" />
    <template v-else>
      <MGroup>
        <MSwitchItem :model-value="enabled" :disabled="saving" :pending="saving" icon="cable" :title="text.wiredEnable" @update:model-value="toggle" />
        <MItem icon="mobile" :title="text.iosPlatform" :value="text.iosStatus" />
        <MItem icon="android" :title="text.androidPlatform" :value="text.wipStatus" />
        <MItem icon="desktop_windows" :title="text.pcPlatform" :value="text.wipStatus" />
      </MGroup>
      <MNotice>
        <span>{{ text.iosNotice }}</span>
        <template #action><MButton size="sm" @click="openVoidLink">{{ text.getVoidLink }}</MButton></template>
      </MNotice>
    </template>
  </MSection>
</template>

<script setup>
import { MSection, MGroup, MItem, MSwitchItem, MNotice, MButton, MSkeleton, saveSettingKey } from 'foundation-ui/vue'
import { computed, onMounted, ref } from 'vue'
import { ElMessage } from '../../composables/notifyMessage.js'
import { openExternalUrl } from '../../tauri-adapter.js'
import { useI18n } from '../../desktop/i18n/index.js'

// The App Store page of the VoidLink client: the only client that speaks the USB wired streaming today.
const VOIDLINK_APP_STORE_URL = 'https://apps.apple.com/cn/app/voidlink/id6747717070'
const { t } = useI18n()
const text = computed(() => t.value.deviceHub.usb)
const loading = ref(true)
const saving = ref(false)
const platform = ref('')
const enabled = ref(false)
const baseline = ref(false)
// the panel's local proxy to the host API, the same path the other settings pages fetch through
const proxyUrl = 'http://localhost:48081'
const hostApi = {
  getJson: async (path) => (await fetch(`${proxyUrl}${path}`)).json(),
  postJson: async (path, body) => (await fetch(`${proxyUrl}${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })).json(),
}

function openVoidLink() {
  void openExternalUrl(VOIDLINK_APP_STORE_URL)
}

onMounted(async () => {
  try {
    const data = await hostApi.getJson('/api/config')
    platform.value = data.platform || ''
    enabled.value = data.usb_wired_streaming_enabled === true || data.usb_wired_streaming_enabled === 'enabled'
    baseline.value = enabled.value
  } catch {
    platform.value = ''
  } finally {
    loading.value = false
  }
})

async function toggle(on) {
  if (saving.value) return
  saving.value = true
  enabled.value = on
  try {
    const result = await saveSettingKey(hostApi, 'usb_wired_streaming_enabled', on ? 'enabled' : 'disabled')
    if (!result.ok) throw new Error(result.error || 'save failed')
    baseline.value = on
    ElMessage.success(text.value.savedToast)
  } catch {
    enabled.value = baseline.value
    ElMessage.error(text.value.wiredSaveFailed)
  } finally {
    saving.value = false
  }
}
</script>
