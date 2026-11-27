<script setup lang="ts">
import { emitTo } from '@tauri-apps/api/event'
import { onMounted, ref } from 'vue'
import { DESKTOP_ACCESS_EVENT, readDesktopAccessResult } from '~/composables/desktop_account'

const error = ref('')

onMounted(async () => {
  const result = readDesktopAccessResult(new URLSearchParams(window.location.search))
  if (!result) {
    error.value = '登录结果无效，请从比邻重新打开登录窗口。'
    return
  }
  try {
    await emitTo('main', DESKTOP_ACCESS_EVENT, result)
  } catch {
    error.value = '无法返回比邻，请重新打开登录窗口。'
  }
})
</script>

<template>
  <main class="desktop-access-return" aria-live="polite">
    <p v-if="error" role="alert">{{ error }}</p>
    <p v-else>正在确认本机绑定状态…</p>
  </main>
</template>

<style scoped>
.desktop-access-return { display: grid; min-height: 100dvh; place-items: center; padding: 24px; color: #252525; background: #f6f7f5; }
</style>
