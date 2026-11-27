<script setup lang="ts">
defineProps<{
    connected: boolean;
    busy?: boolean;
    disabled?: boolean;
    label?: string;
}>();

defineEmits<{ toggle: [] }>();
</script>

<template>
    <button type="button" class="desktop-connection-switch" role="switch"
        :aria-label="label ?? '网络连接'" :aria-checked="connected" :aria-busy="busy || undefined"
        :disabled="disabled || busy" @click="$emit('toggle')">
        <span class="desktop-switch-track" aria-hidden="true"><span /></span>
    </button>
</template>

<style scoped>
.desktop-connection-switch {
    flex-shrink: 0;
    display: inline-grid;
    width: 44px;
    height: 34px;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: inherit;
    cursor: pointer;
}
.desktop-connection-switch:disabled { opacity: .62; cursor: default; }
.desktop-connection-switch:focus-visible { outline: 2px solid currentColor; outline-offset: 1px; }
.desktop-switch-track {
    display: flex;
    align-items: center;
    box-sizing: border-box;
    width: 36px;
    height: 20px;
    padding: 3px;
    border-radius: 20px;
    background: #5d625e;
    transition: background-color 140ms ease;
}
.desktop-switch-track span {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px #0003;
    transition: transform 140ms ease;
}
.desktop-connection-switch[aria-checked='true'] .desktop-switch-track { background: #18b756; }
.desktop-connection-switch[aria-checked='true'] .desktop-switch-track span { transform: translateX(16px); }
@media (prefers-reduced-motion: reduce) {
    .desktop-switch-track, .desktop-switch-track span { transition: none; }
}
</style>
