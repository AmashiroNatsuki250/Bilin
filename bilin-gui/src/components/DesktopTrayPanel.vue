<script setup lang="ts">
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { DesktopConnectionSwitch } from 'bilin-frontend-lib'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import {
  DESKTOP_TRAY_STATE,
  type DesktopTrayEvent,
  type DesktopTrayPeer,
  type DesktopTrayState,
  sendDesktopTrayEvent,
} from '~/composables/desktop_tray'

const state = ref<DesktopTrayState>({ enabled: false, connected: false, peers: [] })
const query = ref('')
const settings = ref<HTMLDetailsElement>()
const filteredPeers = computed(() => {
  const search = query.value.trim().toLocaleLowerCase()
  return state.value.peers.filter(peer => `${peer.name} ${peer.address ?? ''}`.toLocaleLowerCase().includes(search))
})
const onlineCount = computed(() => state.value.peers.filter(peer => peer.online).length)
const connectionStatus = computed(() => {
  if (state.value.busy) return '处理中'
  if (state.value.connected) return '已连接'
  if (state.value.connectionEnabled && state.value.canToggle) return '正在连接'
  return '未连接'
})

let stopListening: UnlistenFn | undefined
let disposed = false

function iconForPeer(peer: DesktopTrayPeer) {
  if (peer.kind === 'mobile')
    return 'pi-mobile'
  if (peer.kind === 'server')
    return 'pi-server'
  return 'pi-desktop'
}

function send(action: DesktopTrayEvent) {
  if (settings.value)
    settings.value.open = false
  void sendDesktopTrayEvent(action).catch(console.warn)
}

onMounted(async () => {
  try {
    const stop = await listen<DesktopTrayState>(DESKTOP_TRAY_STATE, ({ payload }) => {
      state.value = payload
      if (!payload.enabled)
        query.value = ''
    })
    if (disposed) {
      stop()
      return
    }
    stopListening = stop
    send({ type: 'ready' })
  }
  catch (error) {
    console.warn('Could not listen for desktop tray status:', error)
  }
})

onUnmounted(() => {
  disposed = true
  stopListening?.()
})
</script>

<template>
  <main class="tray-panel" aria-label="比邻快捷面板" @keydown.esc.prevent.stop="send({ type: 'dismiss' })">
    <header class="tray-header">
      <span class="tray-brand"><i class="pi pi-sitemap" aria-hidden="true" /></span>
      <h1>比邻</h1>
      <details v-if="state.enabled" ref="settings" class="tray-settings">
        <summary aria-label="设置">
          <i class="pi pi-cog" aria-hidden="true" />
        </summary>
        <div class="tray-settings-menu">
          <button type="button" @click="send({ type: 'settings' })">
            应用设置
          </button>
          <button type="button" @click="send({ type: 'quit' })">
            退出比邻
          </button>
        </div>
      </details>
    </header>

    <div v-if="state.enabled" class="tray-body">
      <section class="tray-network" aria-label="当前网络">
        <div class="tray-network-heading">
          <span><i class="pi pi-sitemap" aria-hidden="true" /> 当前网络</span>
          <span class="tray-connection" :class="{ 'is-online': state.connected }" aria-live="polite">
            {{ connectionStatus }}
          </span>
        </div>
        <div class="tray-network-line">
          <div>
            <h2 :title="state.networkName">
              {{ state.networkName || '未选择网络' }}
            </h2>
            <code v-if="state.address">{{ state.address }}</code>
          </div>
          <DesktopConnectionSwitch :connected="state.connectionEnabled ?? state.connected" :busy="state.busy" :disabled="!state.canToggle"
            @toggle="send({ type: 'toggle-connection' })" />
        </div>
      </section>

      <label class="tray-search">
        <i class="pi pi-search" aria-hidden="true" />
        <input v-model="query" type="search" aria-label="搜索设备" placeholder="搜索设备">
      </label>

      <div class="tray-peer-heading">
        <h2>设备</h2>
        <span>{{ onlineCount }} 台在线</span>
      </div>
      <ul v-if="filteredPeers.length" class="tray-peer-list">
        <li v-for="peer in filteredPeers" :key="peer.id">
          <button type="button" class="tray-peer" @click="send({ type: 'open-peer', peerId: peer.id })">
            <span class="tray-peer-icon"><i class="pi" :class="[iconForPeer(peer)]" aria-hidden="true" /></span>
            <span class="tray-peer-text">
              <strong :title="peer.name">{{ peer.name }}</strong>
              <span class="tray-peer-address" :class="{ 'is-online': peer.online }">
                <span class="tray-sr-only">{{ peer.online ? '在线' : '离线' }} </span>
                <code>{{ peer.address || '—' }}</code>
              </span>
            </span>
            <i class="pi pi-angle-right tray-peer-chevron" aria-hidden="true" />
          </button>
        </li>
      </ul>
      <p v-else class="tray-empty" role="status">
        {{ query.trim() ? '没有匹配的设备' : '暂无设备' }}
      </p>
    </div>

    <footer v-if="state.enabled" class="tray-footer">
      <button type="button" @click="send({ type: 'open-main' })">
        <i class="pi pi-window-maximize" aria-hidden="true" /> 打开主窗口
      </button>
      <button type="button" :disabled="!state.webConsoleAvailable" @click="send({ type: 'web-console' })">
        Web 控制台 <i class="pi pi-external-link" aria-hidden="true" />
      </button>
    </footer>
    <nav v-else class="tray-compact-actions" aria-label="快捷操作">
      <button type="button" @click="send({ type: 'open-main' })">
        <i class="pi pi-window-maximize" aria-hidden="true" /> 打开主窗口
      </button>
      <button type="button" @click="send({ type: 'quit' })">
        <i class="pi pi-sign-out" aria-hidden="true" /> 退出程序
      </button>
    </nav>
  </main>
</template>

<style scoped>
.tray-panel {
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  height: 100dvh;
  overflow: hidden;
  background: #fcfdfa;
  color: #202420;
  font-family: system-ui, 'Microsoft YaHei', 'PingFang SC', sans-serif;
  font-size: 13px;
}
.tray-panel :is(button, summary) {
  cursor: pointer;
}
.tray-panel button:disabled {
  opacity: 0.55;
  cursor: default;
}
.tray-panel :is(button, summary, input):focus-visible {
  outline: 2px solid #536253;
  outline-offset: 2px;
}
.tray-header {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 13px 15px;
  flex-shrink: 0;
}
.tray-brand {
  display: grid;
  place-items: center;
  width: 29px;
  height: 29px;
  color: white;
  background: #252724;
  border-radius: 8px;
}
.tray-header h1 {
  margin: 0;
  font-size: 18px;
  font-weight: 750;
  line-height: 1.2;
}
.tray-settings {
  position: relative;
  margin-left: auto;
  z-index: 1;
}
.tray-settings summary {
  display: grid;
  place-items: center;
  width: 30px;
  height: 30px;
  border: 1px solid #e1e4df;
  border-radius: 7px;
  background: white;
  color: #394339;
  list-style: none;
}
.tray-settings summary::-webkit-details-marker {
  display: none;
}
.tray-settings-menu {
  position: absolute;
  top: 34px;
  right: 0;
  min-width: 107px;
  padding: 4px;
  border: 1px solid #e1e4df;
  border-radius: 8px;
  background: white;
  box-shadow: 0 8px 22px #17251725;
}
.tray-settings-menu button {
  display: block;
  width: 100%;
  padding: 7px 8px;
  border-radius: 4px;
  white-space: nowrap;
  text-align: left;
}
.tray-settings-menu button:hover,
.tray-footer button:not(:disabled):hover,
.tray-compact-actions button:hover {
  background: #e9eee8;
}
.tray-compact-actions {
  display: grid;
  gap: 2px;
  border-top: 1px solid #e5e7e3;
  padding: 7px 9px;
}
.tray-compact-actions button {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 35px;
  padding: 6px 8px;
  border-radius: 6px;
  font-weight: 650;
  text-align: start;
}
.tray-body {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 0 13px;
}
.tray-network {
  position: relative;
  isolation: isolate;
  flex-shrink: 0;
  overflow: hidden;
  padding: 13px 14px;
  border-radius: 11px;
  background: linear-gradient(118deg, #292a28, #1e201f);
  color: #f5f7f3;
}
.tray-network::before {
  position: absolute;
  z-index: -1;
  inset: 0;
  content: '';
  background: radial-gradient(circle, #666d68 1px, transparent 1.35px) 1px 2px / 12px 12px;
  opacity: 0.48;
  -webkit-mask-image: linear-gradient(90deg, transparent 24%, black);
  mask-image: linear-gradient(90deg, transparent 24%, black);
}
.tray-network-heading,
.tray-network-line {
  display: flex;
  justify-content: space-between;
  gap: 13px;
  align-items: flex-end;
}
.tray-network-heading {
  align-items: center;
  margin-bottom: 11px;
  color: #d4dbd2;
  font-size: 11px;
}
.tray-network-heading .pi {
  margin-right: 4px;
}
.tray-connection,
.tray-peer-address {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  white-space: nowrap;
}
.tray-connection::before,
.tray-peer-address::before {
  width: 6px;
  height: 6px;
  flex-shrink: 0;
  border-radius: 50%;
  background: #a1a9a1;
  content: '';
}
.tray-connection.is-online::before {
  background: #35ce81;
}
.tray-network-line > div {
  min-width: 0;
}
.tray-network-line h2 {
  margin: 0;
  overflow: hidden;
  font-size: 20px;
  font-weight: 720;
  line-height: 1.3;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tray-network-line code {
  display: block;
  margin-top: 3px;
  color: #dce4dc;
  font-size: 11px;
}
.tray-search {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  gap: 8px;
  height: 34px;
  margin: 13px 0 7px;
  padding: 0 10px;
  border: 1px solid #e1e5df;
  border-radius: 8px;
  background: white;
  color: #596259;
}
.tray-search input {
  min-width: 0;
  width: 100%;
  border: 0;
  background: transparent;
  color: #252b25;
  font: inherit;
  font-size: 12px;
}
.tray-search input::placeholder {
  color: #596259;
}
.tray-peer-heading {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  padding: 6px 2px 5px;
  font-size: 11px;
}
.tray-peer-heading h2 {
  margin: 0;
  font-size: 12px;
  font-weight: 670;
}
.tray-peer-heading span,
.tray-empty {
  color: #606a60;
}
.tray-empty {
  padding: 15px 4px;
}
.tray-peer-list {
  margin: 0;
  padding: 0;
  list-style: none;
}
.tray-peer-list li + li {
  border-top: 1px solid #e5e7e3;
}
.tray-peer {
  display: flex;
  width: 100%;
  min-width: 0;
  height: 52px;
  gap: 9px;
  align-items: center;
  padding: 0 4px;
  text-align: left;
}
.tray-peer:hover {
  background: #f0f4ee;
}
.tray-peer-icon {
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  flex-shrink: 0;
  border: 1px solid #e2e6e0;
  background: #f2f5f0;
  border-radius: 8px;
  color: #424b42;
}
.tray-peer-text {
  display: flex;
  flex-direction: column;
  min-width: 0;
}
.tray-peer-text strong {
  overflow: hidden;
  font-weight: 730;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tray-peer-address {
  color: #606a60;
  font-size: 11px;
}
.tray-peer-address.is-online::before {
  background: #168658;
}
.tray-peer-chevron {
  margin-left: auto;
  color: #626d62;
}
.tray-footer {
  display: flex;
  flex-shrink: 0;
  justify-content: space-between;
  gap: 6px;
  padding: 8px 12px;
  border-top: 1px solid #e5e7e3;
  background: #f4f6f3;
}
.tray-footer button {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 6px 4px;
  border-radius: 4px;
  color: #384038;
  font-size: 12px;
  font-weight: 690;
  white-space: nowrap;
}
.tray-panel code {
  font-family: Consolas, Monaco, monospace;
}
.tray-sr-only {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
</style>
