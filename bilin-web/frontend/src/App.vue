<script setup lang="ts">
import { Toast } from 'primevue'
</script>

<template>
  <Toast position="bottom-right" />
  <RouterView :key="String($route.name)" />
</template>
