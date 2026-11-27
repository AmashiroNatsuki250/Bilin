<script setup lang="ts">

import { type } from '@tauri-apps/plugin-os'

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { open } from '@tauri-apps/plugin-shell'
import { exit } from '@tauri-apps/plugin-process'
import { I18nUtils, RemoteManagement, Utils } from "bilin-frontend-lib"
import { useTray } from '~/composables/tray'
import { useDesktopAccount } from '~/composables/desktop_account'
import { focusDesktopMainWindow, useDesktopTrayBridge, type DesktopTrayPeer } from '~/composables/desktop_tray'
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  setMobileVpnTileActionHandler,
  setMobileVpnErrorHandler,
  syncMobileVpnService,
} from '~/composables/mobile_vpn'
import { executeVpnTileAction } from '~/composables/mobile_vpn_tile'
import { bootstrapAndroidManagement } from '~/composables/android_management'
import { GUIRemoteClient } from '~/modules/api'

import { ConfirmDialog, Select, ToggleSwitch, useToast, useConfirm } from 'primevue'
import { loadMode, saveMode, WebClientConfig, type Mode } from '~/composables/mode'
import {
  saveLastNetworkInstanceId,
  loadLastNetworkInstanceId,
  loadManagedSubnetProxyPreference,
  loadManagedTunnelPreference,
  loadVpnTileStoppedInstanceIds,
  saveManagedSubnetProxyPreference,
  saveManagedTunnelPreference,
  saveVpnTileStoppedInstanceIds,
} from '~/composables/config'
import ModeSwitcher from '~/components/ModeSwitcher.vue'
import { getBilinVersion, getServiceStatus, getManagementStatus, getControllerEnrollmentStatus, getManagedDeviceName, isManagedDeviceName, renameCurrentDevice, openControllerDeviceLogin, setManagedSubnetProxyEnabled, setManagedTunnelEnabled, type ControllerEnrollmentStatus, type DeviceLoginLaunch } from '~/composables/backend'
import { deviceLoginStorageKey, loadDeviceLoginAttempt, saveDeviceLoginAttempt, updateDeviceLoginAttempt, type DeviceLoginAttempt } from '~/composables/device_login'
import { managementWarningDetails, type OperationOutcome } from '~/composables/management_status'

const { t, locale } = useI18n()
const confirm = useConfirm()
const modeDialogVisible = ref(false)
const currentMode = ref<Mode>({ mode: 'normal' })
const editingMode = ref<Mode>({ mode: 'normal' })
const isModeSaving = ref(false)
const manualDisconnect = ref(false)
const managementReady = ref(type() !== 'android')

const configServerDialogVisible = ref(false)
const configServerConnected = ref(false)
const controllerEnrollment = ref<ControllerEnrollmentStatus | null>(null)
const deviceNameDialogVisible = ref(false)
const deviceNameDraft = ref('')
const deviceNameLoading = ref(false)
const deviceNameSaving = ref(false)
const deviceNameError = ref('')
const deviceLoginAttempt = ref<DeviceLoginAttempt | null>(null)
const deviceLoginOpening = ref(false)
const deviceLoginLaunch = ref<DeviceLoginLaunch | null>(null)
const deviceLoginError = ref('')
let activeDeviceLoginKey = ''
const managedController = ref<string | null>(null)
const controllerModeReady = ref(false)
const managedClient = computed(() => managedController.value !== null || controllerEnrollment.value !== null)
const nativeDesktopLayout = type() === 'windows' || type() === 'macos'
const desktopAccountInitialized = ref(false)
const desktopStartupFailed = ref(false)
const desktopAccountOrigin = computed(() => managedController.value ?? controllerEnrollment.value?.controller_origin ?? null)
const {
  managed: managedDesktopAccount, verified: desktopAccountVerified, showDesktopStartup, showDesktopAccount,
  canOpenDesktopAccount, desktopAccountOpening, desktopAccountError, openDesktopAccount,
  desktopNetworkSetupBusy, desktopNetworkSetupError, desktopNetworkSyncing, openDesktopNetworkSetup,
} = useDesktopAccount(nativeDesktopLayout, desktopAccountOrigin, desktopAccountInitialized, controllerEnrollment)
const desktopWorkspaceReady = computed(() => !managedDesktopAccount.value || desktopAccountVerified.value)
const assignedNetworks = computed(() => controllerEnrollment.value?.networks ?? [])
const managedTunnelBusy = ref(false)
let managedTunnelChange = 0
const desktopConnection = computed(() => {
  if (!nativeDesktopLayout || !managedClient.value) return
  const status = controllerEnrollment.value
  return {
    enabled: status?.desktop_tunnel_enabled ?? true,
    busy: managedTunnelBusy.value || isModeSaving.value,
    available: desktopWorkspaceReady.value && status?.desktop_tunnel_enabled !== undefined
      && status.bound && !status.revoked && !status.account_suspended
      && Boolean(status.device_login_revision && assignedNetworks.value.length),
  }
})
const desktopSubnetProxy = type() === 'windows' || type() === 'linux'
const subnetProxyEnabled = ref(desktopSubnetProxy && loadManagedSubnetProxyPreference())
const subnetProxyReady = ref(false)
const subnetProxySaving = ref(false)

const deviceLoginReady = computed(() => {
  const status = controllerEnrollment.value
  return Boolean(status?.device_login_supported && status.device_login_revision
    && status.registered && !status.bound && !status.revoked && !status.account_suspended)
})
const deviceLoginPending = computed(() => {
  const status = controllerEnrollment.value
  const attempt = deviceLoginAttempt.value
  return Boolean(status?.device_login_supported && !status.revoked && !status.account_suspended && attempt && !attempt.bound)
})
const deviceLoginProgress = computed(() => {
  if (deviceLoginOpening.value) return '正在获取登录链接并尝试打开浏览器…'
  if (deviceLoginLaunch.value?.browser_opened === false) return '系统未能打开浏览器，请复制登录链接后继续。'
  return '等待在浏览器完成登录'
})

async function editDeviceName() {
  if (deviceNameLoading.value || deviceNameSaving.value) return
  deviceNameDraft.value = controllerEnrollment.value?.device_name ?? ''
  deviceNameError.value = ''
  deviceNameDialogVisible.value = true
  deviceNameLoading.value = true
  try {
    deviceNameDraft.value = await getManagedDeviceName()
  } catch (error) {
    deviceNameError.value = String(error)
  } finally {
    deviceNameLoading.value = false
  }
}

async function saveDeviceName() {
  if (deviceNameLoading.value || deviceNameSaving.value || !isManagedDeviceName(deviceNameDraft.value)) return
  deviceNameSaving.value = true
  deviceNameError.value = ''
  try {
    const saved = await renameCurrentDevice(deviceNameDraft.value.trim())
    if (controllerEnrollment.value) controllerEnrollment.value = { ...controllerEnrollment.value, device_name: saved }
    deviceNameDialogVisible.value = false
  } catch (error) {
    deviceNameError.value = String(error)
  } finally {
    deviceNameSaving.value = false
  }
}

function clearDeviceLoginLink() {
  deviceLoginLaunch.value = null
  deviceLoginError.value = ''
}

function recordDeviceLoginAttempt(attempt: DeviceLoginAttempt | null) {
  deviceLoginAttempt.value = attempt
  saveDeviceLoginAttempt(activeDeviceLoginKey, attempt)
}

watch(controllerEnrollment, status => {
  if (!status) {
    activeDeviceLoginKey = ''
    deviceLoginAttempt.value = null
    clearDeviceLoginLink()
    return
  }
  const key = deviceLoginStorageKey(status)
  if (key !== activeDeviceLoginKey) {
    activeDeviceLoginKey = key
    deviceLoginAttempt.value = loadDeviceLoginAttempt(key)
    clearDeviceLoginLink()
  }
  const updated = updateDeviceLoginAttempt(deviceLoginAttempt.value, status)
  if (updated !== deviceLoginAttempt.value) {
    recordDeviceLoginAttempt(updated)
  }
  if (!updated || updated.bound || status.revoked || status.account_suspended) clearDeviceLoginLink()
})

async function openDeviceLogin() {
  const status = controllerEnrollment.value
  const revision = status?.device_login_revision
  if (!status || !revision || !deviceLoginReady.value || deviceLoginOpening.value) return
  const key = deviceLoginStorageKey(status)
  if (key !== activeDeviceLoginKey) return
  if (!deviceLoginAttempt.value) recordDeviceLoginAttempt({ revision, bound: false })
  const pending = deviceLoginAttempt.value
  deviceLoginOpening.value = true
  clearDeviceLoginLink()
  try {
    const issued = await openControllerDeviceLogin()
    if (key !== activeDeviceLoginKey || pending !== deviceLoginAttempt.value || !pending || !deviceLoginPending.value) return
    if (issued.revision > pending.revision) {
      recordDeviceLoginAttempt({ revision: issued.revision, bound: false })
    }
    deviceLoginLaunch.value = issued
  } catch (error) {
    if (key === activeDeviceLoginKey && pending === deviceLoginAttempt.value && deviceLoginPending.value) {
      deviceLoginError.value = String(error)
    }
  } finally {
    deviceLoginOpening.value = false
  }
}

async function updateSubnetProxy(enabled: boolean) {
  const previous = subnetProxyEnabled.value
  if (!subnetProxyReady.value || subnetProxySaving.value || isModeSaving.value || previous === enabled) return
  subnetProxySaving.value = true
  subnetProxyEnabled.value = enabled
  try {
    saveManagedSubnetProxyPreference(enabled)
    await setManagedSubnetProxyEnabled(enabled)
  } catch (error) {
    subnetProxyEnabled.value = previous
    try {
      saveManagedSubnetProxyPreference(previous)
    } catch (rollbackError) {
      console.error('Failed to restore subnet proxy preference', rollbackError)
    }
    toast.add({ severity: 'error', summary: '无法保存子网代理设置', life: 4000 })
    console.error('Failed to update subnet proxy', error)
  } finally {
    subnetProxySaving.value = false
  }
}

const controllerConnectionStatus = computed(() => {
  if (controllerEnrollment.value?.account_suspended) return 'controller.account_suspended'
  if (controllerEnrollment.value?.revoked) return 'controller.device_revoked'
  if (controllerEnrollment.value?.connected) return 'controller.config_connected'
  if (!controllerEnrollment.value?.registered) return 'controller.registering'
  if (!controllerEnrollment.value.bound) return 'controller.registration_waiting_binding'
  if (controllerEnrollment.value.desktop_tunnel_enabled === false) return 'controller.local_tunnel_paused'
  return 'controller.config_pending'
})
type ClientPage = 'overview' | 'traffic' | 'connections' | 'device' | 'settings'

const activePage = ref<ClientPage>('overview')
const pageBody = ref<HTMLElement>()
watch(activePage, () => { if (pageBody.value) pageBody.value.scrollTop = 0 })
const navigation: { page: ClientPage; icon: string }[] = [
  { page: 'overview', icon: 'pi pi-th-large' },
  { page: 'connections', icon: 'pi pi-share-alt' },
  { page: 'device', icon: 'pi pi-desktop' },
  { page: 'settings', icon: 'pi pi-cog' },
]
const networkPage = computed(() => activePage.value === 'overview' || activePage.value === 'connections' || activePage.value === 'traffic')
const desktopView = computed(() => {
  if (activePage.value === 'connections') return 'connections'
  if (nativeDesktopLayout && activePage.value !== 'traffic') return 'devices'
  return 'overview'
})
const canEditController = computed(() => !managedClient.value && currentMode.value.mode !== 'remote')
const controllerAddress = computed(() => managedController.value || (
  currentMode.value.mode !== 'remote' ? currentMode.value.config_server_url : undefined
))

const showAutostartHint = ref(false)

async function openModeDialog() {
  editingMode.value = JSON.parse(JSON.stringify(currentMode.value))
  showAutostartHint.value = false
  modeDialogVisible.value = true
}

async function openAutostartDialog() {
  editingMode.value = JSON.parse(JSON.stringify(currentMode.value))
  editingMode.value.mode = 'service'
  showAutostartHint.value = true
  modeDialogVisible.value = true
}

async function onModeSave() {
  if (isModeSaving.value) {
    return;
  }
  isModeSaving.value = true
  try {
    await initWithMode(editingMode.value);
    desktopStartupFailed.value = false
    desktopAccountInitialized.value = true
    modeDialogVisible.value = false
  }
  catch (e: any) {
    desktopStartupFailed.value = true
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error switching mode", e, currentMode.value, editingMode.value)
    await initWithMode(currentMode.value);
  }
  finally {
    isModeSaving.value = false
  }
}

async function onUninstallService() {
  confirm.require({
    group: 'client',
    message: t('mode.uninstall_service_confirm'),
    header: t('mode.uninstall_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true
    },
    acceptProps: {
      label: t('mode.uninstall_service'),
      severity: 'danger'
    },
    accept: async () => {
      isModeSaving.value = true
      try {
        await initWithMode({ ...currentMode.value, mode: 'normal' });
        await initService(undefined)
        toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.uninstall_service_success'), life: 3000 })
        modeDialogVisible.value = false
      } catch (e: any) {
        toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
        console.error("Error uninstalling service", e)
      } finally {
        isModeSaving.value = false
      }
    },
  });
}

function stripModeMetadata(mode: Mode) {
  if (mode.mode !== 'service') {
    return mode
  }

  const serviceConfig = { ...mode }
  delete serviceConfig.installed_core_version
  return serviceConfig
}

function modeConfigChanged(next: Mode) {
  return JSON.stringify(stripModeMetadata(next)) !== JSON.stringify(stripModeMetadata(currentMode.value))
}

async function onStopService() {
  isModeSaving.value = true
  manualDisconnect.value = true
  try {
    await setServiceStatus(false)
    toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.stop_service_success'), life: 3000 })
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error stopping service", e)
  }
  finally {
    isModeSaving.value = false
  }
}

async function initWithMode(mode: Mode) {
  const running_inst_ids = (await remoteClient.value.list_network_instance_ids().catch(() => undefined))?.running_inst_ids ?? []

  if (currentMode.value.mode === 'service' && mode.mode !== 'service') {
    let serviceStatus = await getServiceStatus()
    if (serviceStatus === "Running") {
      manualDisconnect.value = true
      await setServiceStatus(false)
      serviceStatus = await getServiceStatus()
      for (let i = 0; i < 10; i++) { // macOS takes a while to stop the service
        if (serviceStatus === "Stopped") {
          break;
        }
        await new Promise(resolve => setTimeout(resolve, 100))
        serviceStatus = await getServiceStatus()
      }
    }
    if (serviceStatus === "Stopped") {
      await initService(undefined)
    }
  }

  let url: string | undefined = undefined
  let retrys = 1
  switch (mode.mode) {
    case 'remote':
      if (!mode.remote_rpc_address) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.remote_rpc_address_empty'), life: 10000 })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      url = mode.remote_rpc_address
      break;
    case 'service': {
      if (!mode.config_dir || !mode.file_log_dir || !mode.file_log_level || !mode.rpc_portal) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.service_config_empty'), life: 10000 })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      let serviceStatus = await getServiceStatus()
      const coreVersion = await getBilinVersion()
      if (serviceStatus === "NotInstalled" || modeConfigChanged(mode) || mode.installed_core_version !== coreVersion) {
        mode.config_server_url = mode.config_server_url || undefined
        await initService({
          config_dir: mode.config_dir,
          file_log_dir: mode.file_log_dir,
          file_log_level: mode.file_log_level,
          rpc_portal: mode.rpc_portal,
          config_server: mode.config_server_url,
        })
        mode.installed_core_version = coreVersion
        serviceStatus = await getServiceStatus()
      }
      if (serviceStatus === "Stopped") {
        await setServiceStatus(true)
      }
      url = "tcp://" + mode.rpc_portal.replace("0.0.0.0", "127.0.0.1")
      retrys = 5
      break;
    }
    case 'normal':
      url = mode.rpc_portal;
      break;
  }
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(mode.mode === 'normal', url)
      break;
    } catch (e) {
      if (i === retrys - 1) {
        const errMsg = e instanceof Error ? e.message : String(e)
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: t('mode.rpc_connection_failed', { error: errMsg }),
          life: 1000,
        })
        throw e;
      }
      console.error("Error connecting rpc client, retrying...", e)
      await new Promise(resolve => setTimeout(resolve, 1000))
    }
  }
  if (!managedController.value) {
    await sendConfigs(running_inst_ids.map(Utils.UuidToStr))
  }
  if (mode.mode === 'normal') {
    mode.config_server_url = mode.config_server_url || undefined
    const subnetProxy = desktopSubnetProxy ? subnetProxyEnabled.value : undefined
    await initWebClient(mode.config_server_url?.trim() || undefined, subnetProxy,
      nativeDesktopLayout ? loadManagedTunnelPreference() : undefined)
    controllerEnrollment.value = await getControllerEnrollmentStatus()
  }
  currentMode.value = mode
  try {
    await saveMode(mode)
  } catch (error) {
    if (type() !== 'android') {
      throw error
    }
    // The mode is already applied; a preference write must not abort VPN setup.
    reportManagementWarning({ runtime_applied: true, persistence_warning: String(error) })
  }
  clientRunning.value = await isClientRunning()
}

onMounted(async () => {
  const cleanupFns: Array<() => void> = []

  if (type() === 'android') {
    setMobileVpnErrorHandler(error => toast.add({ severity: 'error', summary: t('error'), detail: error, life: 10000 }))
    cleanupFns.push(() => setMobileVpnErrorHandler())
    try {
      await bootstrapAndroidManagement()
      managementReady.value = true
    } catch (error) {
      toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 10000 })
      return
    }
    try {
      await initMobileVpnService()
    } catch (e: any) {
      console.error("bilin init vpn service failed", e)
    }
  }

  cleanupFns.push(await listenGlobalEvents())
  cleanupFns.push(await listen<OperationOutcome>('management_warning', event => reportManagementWarning(event.payload)))
  try {
    managedController.value = await invoke<string | null>('managed_controller_url')
    if (managedController.value) {
      await I18nUtils.loadLanguageAsync('cn')
      currentMode.value = { mode: 'normal', config_server_url: managedController.value }
    } else {
      currentMode.value = loadMode()
    }
    controllerModeReady.value = true
    await initWithMode(currentMode.value);
    desktopAccountInitialized.value = true
  } catch (error) {
    desktopStartupFailed.value = true
    toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 10000 })
    return
  } finally {
    subnetProxyReady.value = true
  }

  if (type() === 'android') {
    setMobileVpnTileActionHandler(handleMobileVpnTileAction)
    cleanupFns.push(() => setMobileVpnTileActionHandler())
    try {
      await consumePendingMobileVpnTileAction()
      await syncMobileVpnService()
    } catch (e: any) {
      console.error("bilin sync vpn service failed", e)
    }
  }

  onUnmounted(() => {
    cleanupFns.forEach(unlisten => unlisten())
  })
});

useTray(true)
let toast = useToast();
let lastManagementWarning = ''
function reportManagementWarning(outcome: OperationOutcome) {
  const detail = managementWarningDetails(outcome, key => t(key))
  if (detail && detail !== lastManagementWarning) {
    toast.add({ severity: 'warn', summary: t('management_warning'), detail, life: 10000 })
  }
  lastManagementWarning = detail
}

const remoteClient = computed(() => new GUIRemoteClient());
const instanceId = ref<string | undefined>(undefined);
const clientRunning = ref(false);
const requestedPeerId = ref<string | null>(null)
const desktopManagement = ref<InstanceType<typeof RemoteManagement>>()
const devicePanel = shallowRef<import('bilin-frontend-lib').NetworkTypes.DesktopPanelState>()

function handleDevicePanelState(state: import('bilin-frontend-lib').NetworkTypes.DesktopPanelState) {
  devicePanel.value = state
}

async function toggleManagedConnection() {
  const status = controllerEnrollment.value
  const connection = desktopConnection.value
  if (!status || !connection?.available || connection.busy) return
  const enabled = !connection.enabled
  managedTunnelBusy.value = true
  managedTunnelChange++
  try {
    await setManagedTunnelEnabled(enabled, status)
    try {
      saveManagedTunnelPreference(enabled)
    } catch (error) {
      toast.add({ severity: 'warn', summary: '无法保存连接设置', life: 4000 })
      console.debug('Unable to save desktop tunnel preference', error)
    }
  } catch (error) {
    toast.add({ severity: 'error', summary: enabled ? '连接失败，请重试' : '断开失败，请重试', life: 4000 })
    console.debug('Unable to change desktop tunnel state', error)
  } finally {
    try {
      const latest = await getControllerEnrollmentStatus()
      if (latest) controllerEnrollment.value = latest
      await desktopManagement.value?.refreshDesktopNetwork()
    } catch (error) {
      console.debug('Unable to refresh desktop tunnel state', error)
    } finally {
      managedTunnelBusy.value = false
    }
  }
}

function peerKind(icon: string): DesktopTrayPeer['kind'] {
  if (icon.includes('mobile')) return 'mobile'
  if (icon.includes('server')) return 'server'
  return 'desktop'
}

useDesktopTrayBridge(() => {
  const panel = devicePanel.value
  const enabled = !showDesktopAccount.value && !showDesktopStartup.value
  const available = enabled && desktopWorkspaceReady.value && controllerModeReady.value && clientRunning.value
  return {
    enabled,
    networkName: available ? panel?.networkName : undefined,
    address: available ? panel?.address : undefined,
    connected: available && !!panel?.connected,
    connectionEnabled: available && desktopConnection.value?.available ? desktopConnection.value.enabled : undefined,
    busy: !available || isModeSaving.value || managedTunnelBusy.value,
    canToggle: available && (desktopConnection.value?.available ?? !!panel?.canToggle),
    webConsoleAvailable: !!desktopAccountOrigin.value,
    peers: available ? (panel?.peers ?? []).map(peer => ({
      id: peer.id, name: peer.name, address: peer.address, online: peer.online, kind: peerKind(peer.icon),
    })) : [],
  }
}, async (action) => {
  switch (action.type) {
    case 'toggle-connection': {
      if (desktopConnection.value) {
        await toggleManagedConnection()
        break
      }
      const panel = devicePanel.value
      if (!desktopWorkspaceReady.value || !panel?.canToggle) return
      if (panel.connected) await desktopManagement.value?.disconnectCurrentNetwork()
      else await desktopManagement.value?.connectCurrentNetwork()
      break
    }
    case 'web-console':
      if (desktopAccountOrigin.value) await open(desktopAccountOrigin.value)
      break
    case 'quit':
      await exitClient()
      break
    case 'open-main':
      activePage.value = 'overview'
      await focusDesktopMainWindow()
      break
    case 'settings':
      activePage.value = 'settings'
      await focusDesktopMainWindow()
      break
    case 'open-peer':
      activePage.value = 'overview'
      requestedPeerId.value = null
      await nextTick()
      requestedPeerId.value = action.peerId
      await focusDesktopMainWindow()
      break
  }
})

async function handleMobileVpnTileAction(action: 'start' | 'stop') {
  try {
    const result = await executeVpnTileAction(action, remoteClient.value, {
      lastInstanceId: loadLastNetworkInstanceId(),
      stoppedInstanceIds: loadVpnTileStoppedInstanceIds(),
      saveStoppedInstanceIds: saveVpnTileStoppedInstanceIds,
      syncVpnService: syncMobileVpnService,
    })

    if (!result.instanceId) {
      toast.add({
        severity: 'warn',
        summary: t('vpn_tile_no_network'),
        detail: t('vpn_tile_no_network_description'),
        life: 5000,
      })
      return
    }

    instanceId.value = result.instanceId
    await saveLastNetworkInstanceId(result.instanceId)
    toast.add({
      severity: action === 'start' ? 'success' : 'secondary',
      summary: t(action === 'start' ? 'vpn_tile_started' : 'vpn_tile_stopped'),
      life: 3000,
    })
  }
  catch (error) {
    console.error('VPN tile action failed', action, error)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: t('vpn_tile_action_failed', { error: String(error) }),
      life: 8000,
    })
  }
}

watch(instanceId, async (newVal) => {
  if (newVal) {
    try {
      await saveLastNetworkInstanceId(newVal);
    } catch (error) {
      toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 8000 })
    }
  }
});

watch(clientRunning, async (newVal, oldVal) => {
  if (!managementReady.value) return
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false
      return
    }
    await reconnectClient()
  } else if (newVal && !oldVal) {
    const lastInstanceId = loadLastNetworkInstanceId();
    if (lastInstanceId) {
      instanceId.value = lastInstanceId;
    }
  }
})

onMounted(async () => {
  if (managementReady.value) {
    clientRunning.value = await isClientRunning().catch(() => false)
  }
  const timer = setInterval(async () => {
    if (!managementReady.value) return
    try {
      clientRunning.value = await isClientRunning()
      if (clientRunning.value) {
        // Recover warnings even if a lifecycle event was missed during UI attachment.
        const status = await getManagementStatus().catch(() => undefined)
        if (status) reportManagementWarning(status.last_outcome)
      }
    } catch (e) {
      clientRunning.value = false
      console.error("Error checking client running status", e)
    }
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
async function reconnectClient() {
  editingMode.value = JSON.parse(JSON.stringify(currentMode.value));
  await onModeSave()
}

onMounted(async () => {
  window.setTimeout(async () => {
    await setTrayMenu({ show: t('tray.show'), quit: t('tray.exit') })
  }, 1000)
})

const logLevel = ref('off')
const logLevels = computed(() => ['off', 'warn', 'info', 'debug', 'trace'].map(value => ({
  value,
  label: t(`logging_level_${value}`),
})))

async function updateLogLevel(next: string) {
  try {
    await setLoggingLevel(next)
    logLevel.value = next
  } catch (error) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 4000 })
  }
}

async function openLogDirectory() {
  await open(await invoke<string>('get_log_dir_path'))
}

async function copyValue(value: string) {
  try {
    await writeText(value)
    toast.add({ severity: 'success', summary: t('desktop.copied'), life: 1600 })
  } catch (error) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(error), life: 4000 })
  }
}

async function copyLogDirectory() {
  await copyValue(await invoke<string>('get_log_dir_path'))
}

async function changeLanguage() {
  await I18nUtils.loadLanguageAsync(locale.value === 'en' ? 'cn' : 'en')
  await setTrayMenu({ show: t('tray.show'), quit: t('tray.exit') })
}

async function exitClient() {
  await exit(1)
}

async function connectRpcClient(isNormalMode: boolean, url?: string) {
  await initRpcConnection(isNormalMode, url)
  console.log("bilin rpc connection established, isNormalMode: ", isNormalMode)
}

async function openConfigServerDialog() {
  editingMode.value = JSON.parse(JSON.stringify(currentMode.value))
  configServerDialogVisible.value = true
}
async function onConfigServerSave() {
  if (JSON.stringify(currentMode.value) === JSON.stringify(editingMode.value)) {
    configServerDialogVisible.value = false
    return;
  }
  if (editingMode.value.mode === 'service') {
    await new Promise<void>((resolve, reject) => {
      confirm.require({
        group: 'client',
        message: t('config-server.update_service_confirm'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
          label: t('web.common.cancel'),
          severity: 'secondary',
          outlined: true
        },
        acceptProps: {
          label: t('web.common.confirm'),
        },
        accept: async () => {
          resolve()
        },
        reject: () => {
          reject()
        }
      });
    })
  }
  await onModeSave();
  configServerDialogVisible.value = false
}
onMounted(() => {
  let refreshing = false
  const timer = setInterval(async () => {
    const mode = currentMode.value
    if (refreshing || mode.mode !== 'normal' || isModeSaving.value || managedTunnelBusy.value) return
    const tunnelChange = managedTunnelChange
    refreshing = true
    try {
      const status = await getControllerEnrollmentStatus()
      const connected = status?.connected ?? await isWebClientConnected().catch(() => false)
      if (currentMode.value !== mode || isModeSaving.value || tunnelChange !== managedTunnelChange) return
      controllerEnrollment.value = status
      configServerConnected.value = connected
    } catch (error) {
      console.debug('Unable to refresh device enrollment status', error)
    } finally {
      refreshing = false
    }
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
const configServerConnectionStatus = computed(() => {
  if (currentMode.value.mode !== 'normal') {
    return 'unknown'
  }
  if (!currentMode.value.config_server_url && !controllerEnrollment.value) {
    return 'disconnected'
  }
  return configServerConnected.value ? 'connected' : 'connecting'
})
const unmanagedConnectionLabel = computed(() => (
  t('config-server.' + configServerConnectionStatus.value).replace(/^[:：]\s*/, '') || '—'
))

</script>

<template>
  <main v-if="showDesktopStartup || showDesktopAccount" class="desktop-access-gate">
    <section class="desktop-access-card" aria-labelledby="desktop-access-title">
      <span class="desktop-access-mark"><i class="pi pi-sitemap" aria-hidden="true" /></span>
      <h1 id="desktop-access-title">{{ t('brand_name') }}</h1>
      <p v-if="showDesktopStartup" role="status">正在启动…</p>
      <p v-else>使用 Web 控制台账号登录，成功后自动绑定本机。</p>
      <p v-if="desktopAccountError" class="desktop-access-error" role="alert">{{ desktopAccountError }}</p>
      <Button v-if="desktopStartupFailed" label="重试" icon="pi pi-refresh" :loading="isModeSaving" @click="reconnectClient" />
      <Button v-else-if="showDesktopAccount" label="登录／注册" icon="pi pi-sign-in"
        :loading="desktopAccountOpening" :disabled="!canOpenDesktopAccount" @click="openDesktopAccount" />
    </section>
  </main>
  <div v-else class="client-shell" :class="{ 'client-shell-workspace': nativeDesktopLayout }">
    <aside v-if="!nativeDesktopLayout" class="client-rail">
      <div class="client-brand">
        <span class="client-mark"><i class="pi pi-sitemap" aria-hidden="true" /></span>
        <div><strong>{{ t('brand_name') }}</strong><small>{{ t('desktop.client') }}</small></div>
      </div>
      <nav class="client-navigation" :aria-label="t('desktop.nav')">
        <button v-for="item in navigation" :key="item.page" type="button"
          :class="{ active: activePage === item.page }" :aria-current="activePage === item.page ? 'page' : undefined"
          @click="activePage = item.page">
          <i :class="item.icon" aria-hidden="true" /><span>{{ t('desktop.' + item.page) }}</span>
        </button>
      </nav>
      <div class="client-rail-footer">
        <div v-if="managedClient || controllerAddress" class="client-rail-status">
          <i class="client-status-dot" :class="{ connected: managedClient ? controllerEnrollment?.connected : configServerConnected }"
            aria-hidden="true" />
          <div><span>{{ t('desktop.controller_link') }}</span>
            <small>{{ managedClient ? t(controllerConnectionStatus) : unmanagedConnectionLabel }}</small></div>
        </div>
        <button class="client-quit" type="button" @click="exitClient">
          <i class="pi pi-power-off" aria-hidden="true" /><span>{{ t('exit') }}</span>
        </button>
      </div>
    </aside>

    <main class="client-main">
      <header v-if="!nativeDesktopLayout || activePage !== 'overview'" class="client-heading">
        <h1>{{ t(activePage === 'traffic' ? 'desktop.overview' : 'desktop.' + activePage) }}</h1>
        <button v-if="nativeDesktopLayout" type="button" class="workspace-footer-button" @click="activePage = 'overview'">
          <i class="pi pi-arrow-left" aria-hidden="true" />{{ t('desktop.main_window') }}
        </button>
        <div v-if="!nativeDesktopLayout && activePage === 'overview' && managedClient && desktopSubnetProxy" class="client-subnet-proxy"
          title="自动共享本机局域网的 /24 网段">
          <label for="managed-subnet-proxy">子网代理</label>
          <ToggleSwitch input-id="managed-subnet-proxy" :model-value="subnetProxyEnabled"
            :disabled="!subnetProxyReady || subnetProxySaving || isModeSaving" @update:model-value="updateSubnetProxy" />
        </div>
      </header>
      <div ref="pageBody" class="client-body" :class="{ 'client-workspace-home': nativeDesktopLayout && activePage === 'overview' }">
        <div v-show="networkPage">
          <section v-if="!desktopWorkspaceReady" class="workspace-sync" role="status">
            <span class="workspace-sync-mark"><i class="pi pi-sitemap" aria-hidden="true" /></span>
            <div>
              <h2>当前网络</h2>
              <p>正在同步设备…</p>
            </div>
          </section>
          <RemoteManagement v-else-if="controllerModeReady && clientRunning" ref="desktopManagement" :api="remoteClient" :desktop-view="desktopView"
            :managed="managedClient" :assigned-networks="assignedNetworks" :desktop-connection="desktopConnection"
            :pause-auto-refresh="isModeSaving" :requested-peer-id="requestedPeerId" v-model:instance-id="instanceId"
            @copy-address="copyValue" @device-panel-state="handleDevicePanelState" @toggle-managed-connection="toggleManagedConnection">
            <template v-if="nativeDesktopLayout && desktopAccountVerified" #managed-empty>
              <div v-if="desktopNetworkSyncing" class="text-xl text-center font-medium" role="status">正在同步网络…</div>
              <template v-else>
                <div class="text-xl text-center font-medium mb-3">本机尚未加入网络</div>
                <Button label="创建网络" icon="pi pi-plus" :loading="desktopNetworkSetupBusy" @click="openDesktopNetworkSetup" />
                <p v-if="desktopNetworkSetupError" class="client-muted" role="alert">{{ desktopNetworkSetupError }}</p>
              </template>
            </template>
          </RemoteManagement>
          <section v-else-if="controllerModeReady" class="client-empty">
            <span class="client-section-icon"><i class="pi pi-cloud" aria-hidden="true" /></span>
            <h2>{{ t('client.not_running') }}</h2>
            <Button @click="reconnectClient" :loading="isModeSaving" :label="t('client.retry')" icon="pi pi-refresh" />
          </section>
        </div>

        <div v-if="activePage === 'device'" class="client-preferences">
          <section v-if="!desktopWorkspaceReady" class="workspace-sync" role="status">
            <span class="workspace-sync-mark"><i class="pi pi-desktop" aria-hidden="true" /></span>
            <div>
              <h2>{{ t('desktop.identity') }}</h2>
              <p>正在同步设备…</p>
            </div>
          </section>
          <section v-else-if="managedClient" class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-id-card" aria-hidden="true" /></span>
              <h2>{{ t('desktop.identity') }}</h2></header>
            <div v-if="controllerEnrollment?.device_code" class="client-secret">
              <div><span>{{ t('controller.device_id') }}</span><code>{{ controllerEnrollment.device_code }}</code></div>
              <Button outlined icon="pi pi-copy" :label="t('desktop.copy')" :aria-label="t('controller.copy_device_id')"
                @click="copyValue(controllerEnrollment.device_code)" />
            </div>
            <p v-else class="client-muted">{{ t('desktop.device_id_pending') }}</p>
            <p v-if="controllerEnrollment?.device_code_claim_supported && controllerEnrollment.device_code
                && controllerEnrollment.registered && !controllerEnrollment.bound" class="client-muted">
              {{ t('desktop.pairing_instructions') }}
            </p>
            <template v-else-if="!controllerEnrollment?.device_code_claim_supported && controllerEnrollment?.pairing_code">
              <div class="client-secret">
                <div><span>{{ t('controller.pairing_code') }}</span><code>{{ controllerEnrollment.pairing_code }}</code></div>
                <Button outlined icon="pi pi-copy" :label="t('desktop.copy')" :aria-label="t('controller.copy_pairing_code')"
                  @click="copyValue(controllerEnrollment.pairing_code!)" />
              </div>
              <p class="client-muted">{{ t('desktop.legacy_pairing_instructions') }}</p>
            </template>
            <template v-if="controllerEnrollment?.device_name != null">
              <dl class="client-setting-list">
                <div><dt>设备名称</dt><dd>{{ controllerEnrollment.device_name || '未命名设备' }}</dd></div>
              </dl>
              <div v-if="controllerEnrollment.registered" class="client-panel-actions">
                <Button label="修改设备名称" icon="pi pi-pencil" outlined @click="editDeviceName" />
              </div>
            </template>
            <dl v-if="controllerEnrollment?.bound" class="client-setting-list">
              <div><dt>账号绑定</dt><dd>{{ controllerEnrollment.bound_account || '已绑定' }}</dd></div>
            </dl>
            <div v-else-if="deviceLoginPending" class="client-device-login">
              <p v-if="deviceLoginError" class="client-muted" role="alert">{{ deviceLoginError }}</p>
              <p v-else class="client-muted" role="status">{{ deviceLoginProgress }}</p>
              <InputText v-if="deviceLoginLaunch && !deviceLoginLaunch.browser_opened" class="client-device-login-url"
                aria-label="登录链接" :model-value="deviceLoginLaunch.login_url" readonly />
              <div class="client-device-login-actions">
                <Button v-if="deviceLoginLaunch" label="复制登录链接" icon="pi pi-copy" @click="copyValue(deviceLoginLaunch.login_url)" />
                <Button label="继续登录" icon="pi pi-external-link" outlined :loading="deviceLoginOpening"
                  :disabled="!deviceLoginReady" @click="openDeviceLogin" />
              </div>
            </div>
            <div v-else-if="deviceLoginReady" class="client-panel-actions">
              <Button label="登录并绑定本机" icon="pi pi-external-link" @click="openDeviceLogin" />
            </div>
          </section>
          <section class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-globe" aria-hidden="true" /></span>
              <h2>{{ t('config-server.title') }}</h2></header>
            <dl class="client-setting-list">
              <div><dt>{{ t('desktop.connection_details') }}</dt>
                <dd>{{ managedClient ? t(controllerConnectionStatus) : unmanagedConnectionLabel }}</dd></div>
              <div><dt>{{ t('desktop.controller_address') }}</dt><dd><code>{{ controllerAddress || '—' }}</code></dd></div>
            </dl>
            <div v-if="managedController || canEditController" class="client-panel-actions">
              <Button v-if="managedController" @click="open(managedController)" icon="pi pi-external-link"
                :label="t('desktop.open_console')" />
              <Button v-if="canEditController" @click="openConfigServerDialog" icon="pi pi-pencil"
                :label="t('desktop.edit_address')" outlined />
            </div>
          </section>
        </div>

        <div v-if="activePage === 'settings'" class="client-preferences">
          <section v-if="nativeDesktopLayout" class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-sitemap" aria-hidden="true" /></span>
              <h2>{{ t('desktop.workspace_tools') }}</h2></header>
            <div v-if="desktopWorkspaceReady && managedClient && desktopSubnetProxy" class="client-setting-row"
              title="自动共享本机局域网的 /24 网段">
              <label for="workspace-subnet-proxy">子网代理</label>
              <ToggleSwitch input-id="workspace-subnet-proxy" :model-value="subnetProxyEnabled"
                :disabled="!subnetProxyReady || subnetProxySaving || isModeSaving" @update:model-value="updateSubnetProxy" />
            </div>
            <div class="client-panel-actions">
              <Button outlined :label="t('desktop.overview')" icon="pi pi-chart-line" @click="activePage = 'traffic'" />
              <Button outlined :label="t('desktop.connections')" icon="pi pi-share-alt" @click="activePage = 'connections'" />
              <Button outlined :label="t('desktop.device')" icon="pi pi-desktop" @click="activePage = 'device'" />
              <Button outlined :label="t('exit')" icon="pi pi-power-off" @click="exitClient" />
            </div>
          </section>
          <section v-if="controllerModeReady && !managedClient" class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-sliders-h" aria-hidden="true" /></span>
              <h2>{{ t('desktop.basic_settings') }}</h2></header>
            <div class="client-panel-actions">
              <Button outlined @click="changeLanguage" icon="pi pi-language" :label="t('exchange_language')" />
              <Button v-if="type() !== 'android'" outlined @click="openModeDialog" icon="pi pi-sync"
                :label="`${t('mode.switch_mode')} · ${t('mode.' + currentMode.mode)}`" />
              <Button v-if="type() !== 'android'" outlined @click="openAutostartDialog" icon="pi pi-clock"
                :label="t('mode.autostart')" />
            </div>
          </section>
          <section class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-file" aria-hidden="true" /></span>
              <h2>{{ t('desktop.diagnostics') }}</h2></header>
            <div class="client-setting-row">
              <label for="client-log-level">{{ t('desktop.log_level') }}</label>
              <Select input-id="client-log-level" :model-value="logLevel" :options="logLevels"
                option-label="label" option-value="value" @update:model-value="updateLogLevel" />
            </div>
            <div class="client-panel-actions">
              <Button v-if="type() !== 'android'" outlined @click="openLogDirectory" icon="pi pi-folder-open"
                :label="t('logging_open_dir')" />
              <Button outlined @click="copyLogDirectory" icon="pi pi-copy" :label="t('logging_copy_dir')" />
            </div>
          </section>
          <section class="client-panel">
            <header><span class="client-section-icon"><i class="pi pi-info-circle" aria-hidden="true" /></span>
              <h2>{{ t('desktop.application_info') }}</h2></header>
            <About />
          </section>
        </div>
      </div>
      <footer v-if="nativeDesktopLayout" class="workspace-footer">
        <div v-if="managedClient || controllerAddress" class="workspace-footer-status" :title="t('desktop.controller_link')">
          <i class="workspace-footer-dot" :class="{ connected: managedClient ? controllerEnrollment?.connected : configServerConnected }"
            aria-hidden="true" />
          <span>{{ managedClient ? t(controllerConnectionStatus) : unmanagedConnectionLabel }}</span>
        </div>
        <nav class="workspace-footer-actions" :aria-label="t('desktop.nav')">
          <button class="workspace-footer-button" type="button" :aria-current="activePage === 'settings' ? 'page' : undefined"
            @click="activePage = 'settings'">
            <i class="pi pi-cog" aria-hidden="true" />{{ t('desktop.window_settings') }}
          </button>
          <button v-if="managedController" class="workspace-footer-button" type="button" @click="open(managedController)">
            {{ t('desktop.web_console') }}<i class="pi pi-external-link" aria-hidden="true" />
          </button>
        </nav>
      </footer>
    </main>

    <ConfirmDialog group="client" />
    <Dialog v-model:visible="deviceNameDialogVisible" modal header="修改设备名称" class="client-dialog"
      :draggable="false" :closable="!deviceNameSaving" :close-on-escape="!deviceNameSaving">
      <form id="desktop-device-name-form" class="client-dialog-field" @submit.prevent="saveDeviceName">
        <label for="desktop-device-name">设备名称</label>
        <InputText id="desktop-device-name" v-model="deviceNameDraft" :disabled="deviceNameLoading || deviceNameSaving" />
        <p v-if="deviceNameLoading" class="client-muted" role="status">正在读取设备名称…</p>
        <p v-else-if="deviceNameError" class="client-muted" role="alert">{{ deviceNameError }}</p>
        <p v-else-if="!isManagedDeviceName(deviceNameDraft)" class="client-muted">请输入 1 至 32 个字符的设备名称。</p>
      </form>
      <template #footer>
        <Button :label="t('web.common.cancel')" outlined :disabled="deviceNameSaving" @click="deviceNameDialogVisible = false" />
        <Button type="submit" form="desktop-device-name-form" :label="t('web.common.save')" :loading="deviceNameSaving"
          :disabled="deviceNameLoading || !isManagedDeviceName(deviceNameDraft)" />
      </template>
    </Dialog>
    <Dialog v-model:visible="modeDialogVisible" modal :header="t('mode.switch_mode')" class="client-dialog">
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-4">
        {{ t('mode.autostart_hint') }}
      </Message>
      <ModeSwitcher v-model="editingMode" @uninstall-service="onUninstallService" @stop-service="onStopService" />
      <template #footer>
        <Button :label="t('web.common.cancel')" @click="modeDialogVisible = false" outlined />
        <Button :label="t('web.common.save')" @click="onModeSave" :loading="isModeSaving" />
      </template>
    </Dialog>
    <Dialog v-model:visible="configServerDialogVisible" modal :header="t('config-server.title')" class="client-dialog">
      <div class="client-dialog-field">
        <label for="config-server-address">{{ t('config-server.address') }}</label>
        <InputText id="config-server-address" v-model="(editingMode as WebClientConfig).config_server_url"
          :placeholder="t('config-server.address_placeholder')" />
        <p class="client-muted whitespace-pre-wrap">{{ t('config-server.description') }}</p>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" @click="configServerDialogVisible = false" outlined />
        <Button :label="t('web.common.save')" @click="onConfigServerSave" :loading="isModeSaving" />
      </template>
    </Dialog>
  </div>
</template>

<style scoped src="../desktop-workspace.css"></style>
