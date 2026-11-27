<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { Button, Dialog, InputText, Menu } from 'primevue'
import type { DialogPassThroughOptions } from 'primevue/dialog'
import type { MenuItem } from 'primevue/menuitem'
import { useAndroidAccess } from '~/composables/android_access'
import { isManagedDeviceName } from '~/composables/backend'
import pkg from '~/../package.json'

const {
  ready, accountResolved, authorized, username: account, networks, selectedNetworkId, connection, error, busy, captcha: captchaImage,
  devices, devicesLoading, devicesError,
  refreshCaptcha, signIn, register, selectNetwork, connect, disconnect, refreshNetworks, refreshDevices, retryStartup,
  loadDeviceName, renameDevice,
} = useAndroidAccess()

const mode = ref<'login' | 'register'>('login')
const username = ref('')
const password = ref('')
const passwordVisible = ref(false)
const captcha = ref('')
const captchaLoading = ref(false)
const aboutVisible = ref(false)
const menuVisible = ref(false)
const menuButton = ref<HTMLButtonElement>()
const renameVisible = ref(false)
const renameDraft = ref('')
const previousName = ref('')
const renameLoading = ref(false)
const renameSaving = ref(false)
const renameError = ref('')
let renameDialogGeneration = 0
const deviceQuery = ref('')
const deviceDialogVisible = ref(false)
const activeDeviceId = ref<string | null>(null)
const clipboardMessage = ref('')
const dialogParts: DialogPassThroughOptions = {
  root: { class: 'android-client-dialog' },
  mask: { class: 'android-client-dialog-mask' },
}
const dialogCloseButton = { 'aria-label': '关闭', severity: 'secondary', text: true, rounded: true }
const menu = ref<InstanceType<typeof Menu>>()
const menuItems: MenuItem[] = [
  { label: '本机名称', icon: 'pi pi-pencil', disabled: () => !authorized.value, command: () => { void openRenameDialog() } },
  { label: '关于比邻', icon: 'pi pi-info-circle', command: () => { aboutVisible.value = true } },
]

const connected = computed(() => connection.value === 'connected')
const changingConnection = computed(() => connection.value === 'connecting' || connection.value === 'disconnecting')
const currentNetwork = computed(() => networks.value.find(network => network.network_id === selectedNetworkId.value)
  ?? (networks.value.length === 1 ? networks.value[0] : undefined))
const connectionTitle = computed(() => {
  if (connection.value === 'connecting') return '正在连接'
  if (connection.value === 'disconnecting') return '正在断开'
  if (connected.value) return '已连接'
  return !authorized.value || networks.value.length ? '未连接' : '暂无可用网络'
})
const canSubmit = computed(() => username.value.trim() && password.value && !busy.value
  && (mode.value === 'login' || (captchaImage.value && captcha.value.trim() && !captchaLoading.value)))
const filteredDevices = computed(() => {
  const query = deviceQuery.value.trim().toLowerCase()
  return devices.value.filter(device => deviceName(device.hostname).toLowerCase().includes(query)
    || device.virtual_ipv4?.includes(query))
})
const activeDevice = computed(() => devices.value.find(device => device.device_id === activeDeviceId.value))
const liveDeviceStatus = computed(() => connected.value && !devicesError.value)
const proposedName = computed(() => renameDraft.value.trim())
const nameValidationError = computed(() => proposedName.value && !isManagedDeviceName(proposedName.value)
  ? '设备名称最多 32 个字符，不能包含控制字符。' : '')
const canRenameDevice = computed(() => Boolean(proposedName.value && proposedName.value !== previousName.value
  && !nameValidationError.value && !renameLoading.value && !renameSaving.value))

function showMenu(event: MouseEvent) {
  menu.value?.toggle(event)
}

async function openRenameDialog() {
  if (!authorized.value) return
  menuButton.value?.focus({ preventScroll: true })
  const generation = ++renameDialogGeneration
  previousName.value = ''
  renameDraft.value = ''
  renameError.value = ''
  renameLoading.value = true
  renameVisible.value = true
  try {
    const name = await loadDeviceName()
    if (generation !== renameDialogGeneration) return
    previousName.value = name
    renameDraft.value = name
  } catch (cause) {
    if (generation === renameDialogGeneration) {
      renameError.value = typeof cause === 'string' ? cause : '暂时无法读取设备名称。'
    }
  } finally {
    if (generation === renameDialogGeneration) renameLoading.value = false
  }
}

async function saveDeviceName() {
  if (!canRenameDevice.value) return
  const generation = renameDialogGeneration
  renameSaving.value = true
  renameError.value = ''
  try {
    const name = await renameDevice(proposedName.value)
    if (generation === renameDialogGeneration) {
      previousName.value = name
      renameDraft.value = name
      renameVisible.value = false
    }
  } catch (cause) {
    if (generation === renameDialogGeneration) {
      renameError.value = typeof cause === 'string' ? cause : '无法保存设备名称，请重试。'
    }
  } finally {
    renameSaving.value = false
  }
}

async function reloadCaptcha() {
  if (captchaLoading.value) return
  captchaLoading.value = true
  captcha.value = ''
  try {
    await refreshCaptcha()
  } finally {
    captchaLoading.value = false
  }
}

function changeMode() {
  mode.value = mode.value === 'login' ? 'register' : 'login'
  error.value = ''
  password.value = ''
  passwordVisible.value = false
  if (mode.value === 'register') void reloadCaptcha()
}

async function submit() {
  if (!canSubmit.value) return
  const name = username.value.trim()
  const secret = password.value
  password.value = ''
  passwordVisible.value = false
  if (mode.value === 'login') {
    await signIn(name, secret)
  } else {
    await register(name, secret, captcha.value.trim())
    captcha.value = ''
  }
}

async function onNetworkChange(event: Event) {
  await selectNetwork((event.target as HTMLSelectElement).value)
}

async function toggleConnection() {
  if (busy.value || changingConnection.value) return
  if (connected.value) await disconnect()
  else if (currentNetwork.value) await connect()
}

function retryAccount() {
  return ready.value ? refreshNetworks() : retryStartup()
}

function deviceName(hostname: string) {
  return hostname.trim() || '未命名设备'
}

function openDevice(event: MouseEvent, id: string) {
  (event.currentTarget as HTMLButtonElement).focus({ preventScroll: true })
  activeDeviceId.value = id
  clipboardMessage.value = ''
  deviceDialogVisible.value = true
}

async function copyDeviceAddress() {
  const device = activeDevice.value
  if (!device?.virtual_ipv4) return
  clipboardMessage.value = ''
  let message: string
  try {
    await writeText(device.virtual_ipv4)
    message = '地址已复制。'
  } catch {
    message = '无法复制地址，请重试。'
  }
  if (deviceDialogVisible.value && activeDevice.value?.device_id === device.device_id) clipboardMessage.value = message
}

watch(selectedNetworkId, () => {
  deviceQuery.value = ''
  deviceDialogVisible.value = false
})

watch(activeDevice, device => {
  if (!device) deviceDialogVisible.value = false
})

watch(account, (next, previous) => {
  if (previous && !next) {
    mode.value = 'login'
    aboutVisible.value = false
    renameVisible.value = false
    renameDialogGeneration += 1
    deviceQuery.value = ''
    deviceDialogVisible.value = false
    menu.value?.hide()
  }
})
</script>

<template>
  <div class="android-client" :class="{ 'android-client--workspace': account }">
    <main class="android-frame">
      <header class="android-toolbar">
        <div class="android-brand">
          <span class="android-brand-mark"><i class="pi pi-share-alt" aria-hidden="true" /></span>
          <strong>比邻</strong>
        </div>
        <button v-if="account" ref="menuButton" class="android-menu-button" type="button" aria-label="更多"
          aria-haspopup="menu" :aria-expanded="menuVisible" aria-controls="android-client-menu" @click="showMenu">
          <i class="pi pi-ellipsis-v" aria-hidden="true" />
        </button>
        <Menu id="android-client-menu" ref="menu" :model="menuItems" popup
          @show="menuVisible = true" @hide="menuVisible = false">
          <template #start>
            <div class="android-menu-profile">
              <strong>{{ account }}</strong>
              <small v-if="currentNetwork">{{ currentNetwork.display_name }}</small>
            </div>
          </template>
        </Menu>
      </header>

      <section v-if="!account && (!ready || !accountResolved)" class="android-load" aria-live="polite">
        <p v-if="error" role="alert">{{ error }}</p>
        <p v-else>{{ ready ? '正在确认设备状态…' : '正在启动…' }}</p>
        <Button v-if="error" label="重试" outlined :loading="busy" @click="retryAccount" />
      </section>

      <section v-else-if="!account" class="android-flow">
        <header class="android-hero">
          <i :class="mode === 'login' ? 'pi pi-shield' : 'pi pi-user-plus'" aria-hidden="true" />
          <h1>{{ mode === 'login' ? '登录比邻' : '创建账号' }}</h1>
          <p v-if="mode === 'login'">使用控制台账号登录</p>
        </header>

        <form class="android-auth-form" @submit.prevent="submit">
          <div class="android-field">
            <label for="android-username">用户名</label>
            <InputText id="android-username" v-model="username" autocomplete="username" autocapitalize="none"
              :spellcheck="false" :disabled="busy" required />
          </div>
          <div class="android-field">
            <label for="android-password">密码</label>
            <div class="android-password">
              <InputText id="android-password" v-model="password" :type="passwordVisible ? 'text' : 'password'"
                :autocomplete="mode === 'login' ? 'current-password' : 'new-password'" :disabled="busy" required />
              <button type="button" :aria-label="passwordVisible ? '隐藏密码' : '显示密码'" :disabled="busy"
                @click="passwordVisible = !passwordVisible">
                <i :class="passwordVisible ? 'pi pi-eye-slash' : 'pi pi-eye'" aria-hidden="true" />
              </button>
            </div>
          </div>
          <div v-if="mode === 'register'" class="android-field">
            <label for="android-captcha">验证码</label>
            <div class="android-captcha">
              <InputText id="android-captcha" v-model="captcha" autocapitalize="off" autocomplete="off"
                :spellcheck="false" :disabled="busy" required />
              <button type="button" aria-label="更换验证码" :disabled="captchaLoading || busy" @click="reloadCaptcha">
                <img v-if="captchaImage" :src="captchaImage" alt="验证码" />
                <i v-else :class="captchaLoading ? 'pi pi-spin pi-spinner' : 'pi pi-refresh'" aria-hidden="true" />
              </button>
            </div>
          </div>
          <p v-if="error" class="android-message" role="alert">{{ error }}</p>
          <Button type="submit" class="android-primary" :label="mode === 'login' ? '登录' : '注册'"
            :disabled="!canSubmit" :loading="busy" />
          <button type="button" class="android-mode-button" :disabled="busy" @click="changeMode">
            {{ mode === 'login' ? '注册账号' : '返回登录' }}
          </button>
        </form>
      </section>

      <section v-else class="android-workspace">
        <header class="android-hero android-connection-hero">
          <div class="android-connection-row">
            <span class="android-connection-dots" :class="{ 'is-moving': connected }" aria-hidden="true" />
            <button type="button" class="android-connect" role="switch" aria-label="组网连接"
              :aria-checked="connected || connection === 'disconnecting'"
              :disabled="!authorized || busy || changingConnection || (!connected && !currentNetwork)" @click="toggleConnection">
              <span class="android-switch-track" aria-hidden="true"><span /></span>
            </button>
            <h1 class="android-connection-status" :data-state="connection" aria-live="polite">
              <span aria-hidden="true" />{{ connectionTitle }}
            </h1>
          </div>
          <div class="android-current-network">
            <span id="android-network-label">当前网络</span>
            <div v-if="networks.length > 1" class="android-network-picker">
              <select :value="selectedNetworkId ?? ''" aria-labelledby="android-network-label"
                :disabled="!authorized || busy || changingConnection" @change="onNetworkChange">
                <option disabled value="">选择网络</option>
                <option v-for="network in networks" :key="network.network_id" :value="network.network_id">{{ network.display_name }}</option>
              </select>
              <i class="pi pi-chevron-down" aria-hidden="true" />
            </div>
            <strong v-else>{{ currentNetwork?.display_name || '暂未分配' }}</strong>
          </div>
        </header>

        <div v-if="error" class="android-device-error">
          <p class="android-message" role="alert">{{ error }}</p>
          <button v-if="!authorized" type="button" :disabled="busy" @click="retryAccount">重试</button>
        </div>
        <section v-if="currentNetwork" class="android-devices" aria-labelledby="android-devices-heading">
          <label class="android-device-search">
            <i class="pi pi-search" aria-hidden="true" />
            <input v-model="deviceQuery" type="search" aria-label="搜索设备" placeholder="搜索设备"
              autocomplete="off" autocapitalize="off" :spellcheck="false" />
          </label>
          <h2 id="android-devices-heading">设备</h2>
          <div v-if="devicesError" class="android-device-error">
            <p class="android-device-message" role="alert">{{ devicesError }}</p>
            <button type="button" :disabled="devicesLoading" @click="refreshDevices">重试</button>
          </div>
          <p v-if="!devices.length && devicesLoading" class="android-device-message" role="status">正在读取设备…</p>
          <p v-else-if="authorized && !filteredDevices.length && !devicesError" class="android-device-message" role="status">
            {{ deviceQuery.trim() ? '没有匹配的设备' : '暂无设备' }}
          </p>
          <ul v-if="filteredDevices.length" class="android-device-list" :class="{ 'is-disconnected': !connected }">
            <li v-for="device in filteredDevices" :key="device.device_id">
              <button type="button" class="android-device-row" @click="openDevice($event, device.device_id)">
                <span class="android-device-text">
                  <span class="android-device-name">
                    <span class="android-device-dot" :class="{ 'is-online': liveDeviceStatus && device.online }" aria-hidden="true" />
                    <strong>{{ deviceName(device.hostname) }}</strong>
                  </span>
                  <span v-if="device.virtual_ipv4" class="android-device-address">{{ device.virtual_ipv4 }}</span>
                </span>
                <span v-if="liveDeviceStatus" class="android-visually-hidden">{{ device.online ? '在线' : '离线' }}</span>
                <i class="pi pi-chevron-right" aria-hidden="true" />
              </button>
            </li>
          </ul>
        </section>
        <div v-else-if="authorized && !networks.length" class="android-empty-networks">
          <p class="android-message">在 Web 控制台创建或选择网络，将本机加入。</p>
          <Button label="刷新网络" icon="pi pi-refresh" outlined :loading="busy" @click="refreshNetworks" />
        </div>
      </section>
    </main>

    <Dialog v-model:visible="aboutVisible" header="关于比邻" :pt="dialogParts" :close-button-props="dialogCloseButton"
      :draggable="false" modal>
      <dl class="android-dialog-fields">
        <div><dt>应用版本</dt><dd>{{ pkg.version }}</dd></div>
      </dl>
    </Dialog>
    <Dialog v-model:visible="renameVisible" header="本机名称" :pt="dialogParts" :close-button-props="dialogCloseButton"
      :draggable="false" :closable="!renameSaving" :close-on-escape="!renameSaving" modal class="android-rename-device-dialog">
      <form class="android-rename-form" @submit.prevent="saveDeviceName">
        <div class="android-field">
          <label for="android-device-name">设备名称</label>
          <InputText id="android-device-name" v-model="renameDraft" autocomplete="off" autocapitalize="off"
            :spellcheck="false" :disabled="renameLoading || renameSaving" :invalid="Boolean(nameValidationError)"
            aria-describedby="android-rename-feedback" required @update:model-value="renameError = ''" />
        </div>
        <div id="android-rename-feedback">
          <p v-if="renameLoading" class="android-message" role="status">正在读取设备名称…</p>
          <p v-else-if="nameValidationError || renameError" class="android-message" role="alert">{{ nameValidationError || renameError }}</p>
        </div>
        <Button type="submit" class="android-primary" label="保存" :disabled="!canRenameDevice" :loading="renameSaving" />
      </form>
    </Dialog>
    <Dialog v-model:visible="deviceDialogVisible" :header="activeDevice ? deviceName(activeDevice.hostname) : '设备'"
      :pt="dialogParts" :close-button-props="dialogCloseButton" :draggable="false" modal class="android-device-dialog">
      <dl v-if="activeDevice" class="android-dialog-fields">
        <div>
          <dt>虚拟 IPv4</dt>
          <dd><code v-if="activeDevice.virtual_ipv4">{{ activeDevice.virtual_ipv4 }}</code><template v-else>未分配</template></dd>
        </div>
        <div v-if="liveDeviceStatus"><dt>状态</dt><dd>{{ activeDevice.online ? '在线' : '离线' }}</dd></div>
      </dl>
      <p v-if="clipboardMessage" class="android-message android-copy-message" role="status">{{ clipboardMessage }}</p>
      <template v-if="activeDevice?.virtual_ipv4" #footer>
        <Button class="android-dialog-copy" label="复制地址" icon="pi pi-copy" outlined @click="copyDeviceAddress" />
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.android-client {
  min-height: 100%;
  height: 100dvh;
  overflow-y: auto;
  background: #f4f4f4;
  color: #181818;
}

.android-frame {
  display: grid;
  align-content: start;
  gap: 19px;
  width: min(100%, 480px);
  margin-inline: auto;
  padding: max(15px, env(safe-area-inset-top)) 18px max(24px, env(safe-area-inset-bottom));
}

.android-toolbar, .android-brand {
  display: flex;
  align-items: center;
}

.android-toolbar { justify-content: space-between; min-height: 44px; }
.android-brand { gap: 10px; }
.android-brand strong { font-size: 20px; font-weight: 720; letter-spacing: .04em; }

.android-brand-mark, .android-menu-button {
  display: grid;
  place-items: center;
  flex: 0 0 auto;
  border-radius: 12px;
}

.android-brand-mark { width: 39px; height: 39px; background: #202020; color: #fff; font-size: 19px; }
.android-menu-button { width: 44px; height: 44px; border: 1px solid #dedede; background: #fff; font-size: 17px; }

.android-flow { display: grid; gap: 14px; }
.android-client--workspace { background: #fdfdfd; }
.android-client--workspace .android-frame { gap: 16px; padding-inline: 12px; }
.android-client--workspace .android-toolbar { padding-inline-start: 7px; }
.android-client--workspace .android-menu-button { border: 0; background: transparent; }
.android-workspace { display: grid; gap: 16px; min-width: 0; }
.android-hero {
  position: relative;
  isolation: isolate;
  display: grid;
  justify-items: start;
  align-content: center;
  gap: 8px;
  overflow: hidden;
  min-height: 156px;
  border: 1px solid #2d2d2d;
  border-radius: 20px;
  padding: 24px;
  background: #202020;
  color: #fff;
}

.android-hero::before {
  content: "";
  position: absolute;
  inset: 0;
  z-index: -1;
  background: radial-gradient(circle, #666 0.8px, transparent 1.25px) 0 0 / 13px 13px;
  mask-image: linear-gradient(90deg, transparent 14%, #000 100%);
  opacity: .67;
  pointer-events: none;
}

.android-hero > i { margin-bottom: 5px; color: #d6d6d6; font-size: 20px; }
.android-hero h1 { font-size: clamp(24px, 7vw, 32px); font-weight: 720; line-height: 1.18; margin: 0; }
.android-hero p { font-size: 13px; color: #e0e0e0; margin: 0; overflow-wrap: anywhere; }
.android-connection-hero {
  --connection-inset-top: 10px;
  --connection-inset-inline: 16px;
  display: block;
  min-height: auto;
  padding: var(--connection-inset-top) var(--connection-inset-inline) 7px;
  border-color: #28292b;
  border-radius: 15px;
  background: linear-gradient(125deg, #313234, #252628);
}
.android-connection-hero::before { content: none; }

.android-connection-row {
  position: relative;
  isolation: isolate;
  display: flex;
  align-items: center;
  gap: 18px;
  width: 100%;
  min-height: 54px;
  padding-bottom: 7px;
}

.android-connection-dots {
  --dot-spacing: 11px;
  position: absolute;
  inset: calc(-1 * var(--connection-inset-top)) calc(-1 * var(--connection-inset-inline)) 0;
  z-index: -1;
  overflow: hidden;
  pointer-events: none;
  mask-image: linear-gradient(90deg, transparent 45%, #000 100%);
}

.android-connection-dots::before {
  content: "";
  position: absolute;
  inset: 0 calc(-1 * var(--dot-spacing)) 0 0;
  background: radial-gradient(circle, #666 .8px, transparent 1.25px) 0 0 / var(--dot-spacing) var(--dot-spacing);
  opacity: .6;
}

@media (prefers-reduced-motion: no-preference) {
  .android-connection-dots.is-moving::before { animation: android-dots-drift 2s linear infinite; }
}

@keyframes android-dots-drift {
  to { transform: translateX(calc(-1 * var(--dot-spacing))); }
}

.android-hero .android-connection-status {
  display: flex;
  align-items: center;
  gap: 10px;
  color: #f6f6f6;
  font-size: 18px;
  font-weight: 650;
}
.android-connection-status > span, .android-device-dot {
  width: 10px;
  height: 10px;
  flex: 0 0 10px;
  border-radius: 50%;
  background: #a6a8aa;
}
.android-connection-status[data-state="connected"] > span { background: #23d36f; }
.android-connection-status[data-state="disconnected"] > span { background: #ed9088; }

.android-connect {
  display: grid;
  place-items: center;
  width: 56px;
  min-height: 44px;
  flex: 0 0 56px;
  padding: 0;
  border-radius: 10px;
  cursor: pointer;
}
.android-switch-track {
  display: flex;
  align-items: center;
  width: 54px;
  height: 28px;
  padding: 3px;
  border-radius: 999px;
  background: #55585a;
  transition: background .16s ease;
}
.android-switch-track > span {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  background: #fafafa;
  box-shadow: 0 1px 3px #1114;
  transition: transform .16s ease;
}
.android-connect[aria-checked="true"] .android-switch-track { background: #0bab50; }
.android-connect[aria-checked="true"] .android-switch-track > span { transform: translateX(26px); }
.android-connect:focus-visible, .android-network-picker select:focus-visible { outline: 2px solid #fff; outline-offset: 2px; }
.android-connect:disabled { cursor: default; opacity: .72; }

.android-current-network {
  display: flex;
  align-items: center;
  gap: 18px;
  width: 100%;
  min-height: 49px;
  border-top: 1px solid #ffffff30;
}
.android-current-network > span { flex: 0 0 auto; color: #cecece; font-size: 14px; }
.android-current-network > strong, .android-network-picker select {
  min-width: 0;
  overflow: hidden;
  color: #f5f5f5;
  font-size: 17px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.android-network-picker { position: relative; flex: 1 1 auto; min-width: 0; }
.android-network-picker select {
  display: block;
  width: 100%;
  min-height: 44px;
  padding-inline-end: 25px;
  appearance: none;
  border: 0;
  border-radius: 5px;
  background: transparent;
  cursor: pointer;
}
.android-network-picker select:disabled { color: #d3d3d3; cursor: default; }
.android-network-picker option { background: #fff; color: #202020; }
.android-network-picker > i {
  position: absolute;
  inset-inline-end: 0;
  top: 50%;
  transform: translateY(-50%);
  font-size: 14px;
  pointer-events: none;
}

.android-device-search {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 44px;
  padding-inline: 14px;
  border-radius: 14px;
  background: #eceeef;
  color: #474a4e;
}
.android-device-search:focus-within { outline: 2px solid #6b6b6b; outline-offset: 2px; }
.android-device-search > i { flex: 0 0 auto; font-size: 16px; }
.android-device-search input {
  width: 100%;
  min-width: 0;
  min-height: 42px;
  border: 0;
  outline: 0;
  background: transparent;
  font-size: 16px;
}
.android-device-search input::placeholder { color: #595d61; opacity: 1; }
.android-devices h2 { margin: 15px 3px 4px; font-size: 19px; font-weight: 720; }
.android-device-message { margin: 10px 3px; color: #5e6265; }
.android-device-error { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.android-device-error button { min-height: 44px; padding-inline: 8px; text-decoration: underline; }
.android-device-list { padding: 0; margin: 0 2px; list-style: none; }
.android-device-list li { border-bottom: 1px solid #dedfe0; }
.android-device-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 13px;
  align-items: center;
  gap: 12px;
  width: 100%;
  min-height: 66px;
  padding: 9px 5px;
  text-align: start;
}
.android-device-row:active { background: #f0f1f2; }
.android-device-dot.is-online { background: #17c96a; }
.android-device-text { display: grid; gap: 1px; min-width: 0; overflow-wrap: anywhere; }
.android-device-name { display: flex; align-items: center; gap: 8px; min-width: 0; }
.android-device-name > strong {
  min-width: 0;
  overflow: hidden;
  font-size: 17px;
  font-weight: 640;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.android-device-address { color: #55595b; font-size: 14px; }
.android-device-row > i { color: #5d6163; font-size: 13px; }
.android-device-list.is-disconnected .android-device-text { color: #6d7072; }
.android-device-list.is-disconnected .android-device-address { color: #707376; }
.android-visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
}
.android-empty-networks { display: grid; gap: 10px; }

.android-auth-form, .android-load {
  display: grid;
  gap: 14px;
  min-width: 0;
  border: 1px solid #e1e1e1;
  border-radius: 16px;
  padding: 18px;
  background: #fff;
}

.android-field { display: grid; gap: 6px; }
.android-field label { font-size: 13px; font-weight: 650; }
.android-field :deep(.p-inputtext) {
  width: 100%;
  min-width: 0;
  min-height: 46px;
  border-radius: 9px;
  font-size: 16px;
}

.android-password { position: relative; }
.android-password :deep(.p-inputtext) { padding-inline-end: 46px; }
.android-password button {
  position: absolute;
  inset: 1px 1px 1px auto;
  width: 44px;
  border-radius: 8px;
  color: #4b4b4b;
}

.android-captcha { display: grid; grid-template-columns: minmax(0, 1fr) 106px; gap: 8px; }
.android-captcha button { min-height: 46px; border: 1px solid #d5d5d5; border-radius: 9px; overflow: hidden; }
.android-captcha img { display: block; width: 100%; height: 44px; object-fit: contain; }

.android-primary {
  min-height: 48px;
  width: 100%;
  border-radius: 10px;
  background: #202020;
  border-color: #202020;
  color: #fff;
  font-weight: 630;
}
.android-primary:enabled:hover { background: #353535; border-color: #353535; }
.android-mode-button { min-height: 44px; border-radius: 8px; color: #383838; }
.android-mode-button:hover { background: #f4f4f4; }
.android-mode-button:disabled, .android-captcha button:disabled { opacity: .55; }
.android-message { margin: 0; color: #464646; font-size: 13px; overflow-wrap: anywhere; }
.android-menu-profile { display: grid; gap: 3px; max-width: 250px; padding: 10px 12px; border-bottom: 1px solid #e9e9e9; overflow-wrap: anywhere; }
.android-menu-profile small { color: #5e5e5e; }

:global(.android-client-dialog-mask.p-dialog-mask) {
  box-sizing: border-box;
  padding: max(12px, env(safe-area-inset-top)) max(14px, env(safe-area-inset-right))
    max(12px, env(safe-area-inset-bottom)) max(14px, env(safe-area-inset-left));
}
:global(.android-client-dialog.p-dialog) {
  box-sizing: border-box;
  width: min(100%, 380px);
  min-width: 0;
  max-width: none;
  max-height: 100%;
  overflow-y: auto;
  overscroll-behavior: contain;
  border-radius: 16px;
  background: #fff;
  color: #202020;
}
:global(.android-client-dialog .p-dialog-header) {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  gap: 12px;
  padding: 14px 16px 8px;
}
:global(.android-client-dialog .p-dialog-title) {
  min-width: 0;
  margin-block: 8px 4px;
  overflow-wrap: anywhere;
  font-size: 1.25rem;
  font-weight: 690;
  line-height: 1.35;
}
:global(.android-client-dialog .p-dialog-close-button.p-button) {
  width: 44px;
  min-width: 44px;
  height: 44px;
  padding: 0;
  border: 0;
  border-radius: 50%;
  background: transparent;
  color: #525252;
  box-shadow: none;
}
:global(.android-client-dialog .p-dialog-close-button.p-button:is(:hover, :active)) { background: #ededed; }
:global(.android-client-dialog .p-dialog-close-button.p-button:focus-visible) {
  outline: 2px solid #777;
  outline-offset: 1px;
}
:global(.android-client-dialog .p-dialog-close-button svg) { width: 16px; height: 16px; }
:global(.android-client-dialog .p-dialog-content) { flex-shrink: 0; min-height: 0; padding: 0 18px 16px; overflow: visible; }
:global(.android-client-dialog .p-dialog-footer) { padding: 0 18px 16px; }
:global(.android-client-dialog .android-dialog-copy.p-button) { width: 100%; min-height: 44px; }

.android-dialog-fields { display: grid; margin: 0; }
.android-dialog-fields > div {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  justify-content: space-between;
  gap: 4px 14px;
  padding-block: 10px;
}
.android-dialog-fields > div + div { border-top: 1px solid #ececec; }
.android-dialog-fields dt, .android-dialog-fields dd { min-width: 0; margin: 0; overflow-wrap: anywhere; }
.android-dialog-fields dt { color: #5b5b5b; }
.android-dialog-fields dd { font-weight: 550; }
.android-dialog-fields code { user-select: text; }
.android-copy-message { margin-top: 8px; }
.android-rename-form { display: grid; gap: 12px; }
.android-rename-form > div:empty { display: none; }

@media (min-width: 540px) {
  .android-frame { padding-top: 28px; }
}
</style>
