<script setup lang="ts">
import { getBilinVersion } from '~/composables/backend'
import pkg from '~/../package.json'

const { t } = useI18n()

const etVersion = ref('')

onMounted(async () => {
  etVersion.value = await getBilinVersion().catch(() => '')
})
</script>

<template>
  <dl class="client-setting-list">
    <div><dt>{{ t('about.version') }}</dt><dd><code>{{ pkg.version }}</code></dd></div>
    <div><dt>{{ t('status.version') }}</dt><dd><code>{{ etVersion || '—' }}</code></dd></div>
  </dl>
</template>
