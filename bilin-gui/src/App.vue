<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window'
import { type } from '@tauri-apps/plugin-os'
import { defineAsyncComponent } from 'vue'
import pkg from '~/../package.json'

const { t, locale } = useI18n()
const isAndroid = type() === 'android'
const currentWindow = getCurrentWindow()
const isDesktopAccessWindow = currentWindow.label === 'desktop-access'
const isDesktopAccessReturn = isDesktopAccessWindow && new URLSearchParams(window.location.search).has('desktop-access-result')
const isDesktopTrayWindow = currentWindow.label === 'desktop-tray'
const AndroidClient = defineAsyncComponent(() => import('~/components/AndroidClient.vue'))
const DesktopAccessReturn = defineAsyncComponent(() => import('~/components/DesktopAccessReturn.vue'))
const DesktopTrayPanel = defineAsyncComponent(() => import('~/components/DesktopTrayPanel.vue'))

watchEffect(() => {
  document.documentElement.lang = locale.value === 'cn' ? 'zh-CN' : 'en'
})

onBeforeMount(async () => {
  if (!isDesktopTrayWindow && !isDesktopAccessWindow)
    await currentWindow.setTitle(`${t('brand_name')} · v${pkg.version}`)
})
</script>

<template>
  <Toast position="bottom-right" />
  <DesktopAccessReturn v-if="isDesktopAccessReturn" />
  <DesktopTrayPanel v-else-if="isDesktopTrayWindow" />
  <AndroidClient v-else-if="isAndroid" />
  <RouterView v-else-if="!isDesktopAccessWindow" />
</template>
