import { invoke } from '@tauri-apps/api/core'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { bootstrapAndroidManagement } from './android_management'
import { getControllerEnrollmentStatus, getManagedDeviceName, initRpcConnection, initWebClient, renameCurrentDevice } from './backend'
import { listenGlobalEvents } from './event'
import {
  consumePendingMobileVpnTileAction,
  hasAndroidVpnAttachment,
  initMobileVpnService,
  setAndroidVpnNetwork,
  setMobileVpnErrorHandler,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from './mobile_vpn'

interface AccessNetwork {
  network_id: string
  display_name: string
}

interface AccessAccount {
  username: string
  networks: AccessNetwork[]
  device_login_revision: number
  selected_network_id: string | null
}

interface AccessDevice {
  device_id: string
  hostname: string
  virtual_ipv4: string | null
  online: boolean
}

interface AccessDeviceRoster {
  network_id: string
  devices: AccessDevice[]
}

type Connection = 'disconnected' | 'connecting' | 'connected' | 'disconnecting'

export function useAndroidAccess() {
  const ready = ref(false)
  const accountResolved = ref(false)
  const username = ref<string | null>(null)
  const networks = ref<AccessNetwork[]>([])
  const selectedNetworkId = ref<string | null>(null)
  const connection = ref<Connection>('disconnected')
  const error = ref('')
  const busy = ref(false)
  const captcha = ref('')
  const devices = ref<AccessDevice[]>([])
  const devicesLoading = ref(false)
  const devicesError = ref('')

  const sessionRevision = ref<number>()
  const authorized = computed(() => sessionRevision.value !== undefined)
  let rememberedRevision: number | undefined
  let sessionGeneration = 0
  let nextSessionUpdate = 0
  let validationStarted = 0
  let connectDeadline = 0
  let automaticStart = true
  let refreshPending = false
  let checking = false
  let unlisten: (() => void) | undefined
  let vpnListenersReady = false
  let timer: ReturnType<typeof setInterval> | undefined
  let devicesRequest = 0
  let devicesPending = false

  function hasNetwork(id: string | null) {
    return Boolean(id && networks.value.some(network => network.network_id === id))
  }

  function resetDevices() {
    devicesRequest += 1
    devicesPending = false
    devices.value = []
    devicesLoading.value = false
    devicesError.value = ''
  }

  watch(selectedNetworkId, resetDevices, { flush: 'sync' })

  async function refreshDevices(force = false) {
    const networkId = selectedNetworkId.value
    const revision = sessionRevision.value
    if (!username.value || !networkId || revision === undefined || (devicesPending && !force)) return
    const request = ++devicesRequest
    devicesPending = true
    devicesLoading.value = devices.value.length === 0 && !devicesError.value
    try {
      const roster = await invoke<AccessDeviceRoster | null>('android_access_devices', { revision })
      if (request !== devicesRequest) return
      // The server resolves the selected network itself. A response from the
      // previous network must never appear under the newly selected network.
      if (roster?.network_id === networkId) devices.value = roster.devices
      else devices.value = []
      devicesError.value = ''
    } catch (reason) {
      if (request === devicesRequest) {
        devicesError.value = typeof reason === 'string' ? reason : '暂时无法读取设备列表。'
      }
    } finally {
      if (request === devicesRequest) {
        devicesPending = false
        devicesLoading.value = false
      }
    }
  }

  async function renameDevice(name: string) {
    if (!authorized.value) throw '登录状态正在确认，请稍后重试。'
    const savedName = await renameCurrentDevice(name)
    void refreshDevices(true)
    return savedName
  }

  async function stopConnection() {
    if (connection.value === 'disconnecting') return
    connection.value = 'disconnecting'
    connectDeadline = 0
    try {
      await Promise.all([setAndroidVpnNetwork(null), invoke('android_access_disconnect')])
    } catch {
      error.value = '断开连接失败，请重试。'
    } finally {
      connection.value = selectedNetworkId.value && hasAndroidVpnAttachment(selectedNetworkId.value)
        ? 'connected' : 'disconnected'
    }
  }

  async function disconnect() {
    automaticStart = false
    error.value = ''
    await stopConnection()
  }

  async function connect() {
    const network = selectedNetworkId.value
    const revision = sessionRevision.value
    if (!username.value || !network || !hasNetwork(network) || revision === undefined
      || connection.value !== 'disconnected') return
    automaticStart = false
    error.value = ''
    connection.value = 'connecting'
    connectDeadline = Date.now() + 45_000
    const generation = sessionGeneration
    try {
      await setAndroidVpnNetwork(network)
      await invoke('android_access_connect', { networkId: network, revision })
      if (generation !== sessionGeneration || connection.value !== 'connecting') return
      void refreshDevices(true)
      // The runtime may be accepted before its address is ready. Reconciliation
      // keeps watching that selected network until a TUN descriptor is attached.
      await syncMobileVpnService()
    } catch (reason) {
      if (generation !== sessionGeneration || connection.value !== 'connecting') return
      error.value = typeof reason === 'string' ? reason : '连接失败，请重试。'
      await stopConnection()
    }
  }

  async function selectNetwork(id: string) {
    if (!authorized.value || !hasNetwork(id) || connection.value === 'connecting' || connection.value === 'disconnecting') return
    if (selectedNetworkId.value === id && connection.value === 'connected') return
    if (connection.value === 'connected') await disconnect()
    if (connection.value !== 'disconnected') return
    selectedNetworkId.value = id
    await connect()
  }

  async function clearSession() {
    sessionGeneration += 1
    resetDevices()
    sessionRevision.value = undefined
    rememberedRevision = undefined
    username.value = null
    networks.value = []
    selectedNetworkId.value = null
    error.value = ''
    automaticStart = true
    accountResolved.value = true
    await Promise.all([invoke('android_access_forget').catch(() => {}), stopConnection()])
  }

  async function useAccount(account: AccessAccount) {
    const resumed = account.device_login_revision === sessionRevision.value && account.username === username.value
    if (!resumed) {
      sessionGeneration += 1
      resetDevices()
      sessionRevision.value = account.device_login_revision
      rememberedRevision = account.device_login_revision
      username.value = account.username
      automaticStart = true
    }
    networks.value = account.networks
    const previous = selectedNetworkId.value
    if (!resumed || !hasNetwork(previous)) {
      const selected = networks.value.length === 1 ? networks.value[0].network_id
        : hasNetwork(account.selected_network_id) ? account.selected_network_id : null
      if (previous && previous !== selected && connection.value !== 'disconnected') await stopConnection()
      selectedNetworkId.value = selected
    }
    if (automaticStart && selectedNetworkId.value) void connect()
    else if (!resumed || previous !== selectedNetworkId.value) void refreshDevices()
  }

  async function refreshNetworks() {
    if (!ready.value || refreshPending || busy.value) return
    refreshPending = true
    const generation = sessionGeneration
    try {
      const enrollment = await getControllerEnrollmentStatus()
      if (generation !== sessionGeneration || busy.value) return
      if (enrollment?.revoked || enrollment?.account_suspended) {
        if (username.value) await clearSession()
        else accountResolved.value = true
        return
      }
      if (!enrollment?.registered) {
        if (!authorized.value && Date.now() - validationStarted >= 5000)
          error.value = '暂时无法确认账号状态，请稍后重试。'
        return
      }
      const account = await invoke<AccessAccount | null>('android_access_status')
      if (generation !== sessionGeneration || busy.value) return
      if (account) {
        if (!authorized.value) error.value = ''
        await useAccount(account)
      } else if (username.value) await clearSession()
      else error.value = ''
      accountResolved.value = true
    } catch (reason) {
      if (generation === sessionGeneration && !busy.value && connection.value === 'disconnected')
        error.value = typeof reason === 'string' ? reason : '暂时无法读取网络，请重试。'
    } finally {
      refreshPending = false
    }
  }

  async function refreshCaptcha() {
    captcha.value = ''
    try {
      captcha.value = await invoke<string>('android_access_captcha')
    } catch (reason) {
      error.value = typeof reason === 'string' ? reason : '验证码加载失败，请重试。'
    }
  }

  async function authenticate(name: string, password: string, code?: string) {
    if (busy.value || !ready.value) return
    busy.value = true
    error.value = ''
    try {
      const account = await invoke<AccessAccount>('android_access_sign_in', {
        username: name, password, captcha: code,
      })
      await useAccount(account)
      accountResolved.value = true
    } catch (reason) {
      error.value = typeof reason === 'string' ? reason : '登录失败，请重试。'
      if (code) await refreshCaptcha()
    } finally {
      busy.value = false
    }
  }

  async function check() {
    if (!ready.value || checking) return
    checking = true
    const generation = sessionGeneration
    try {
      const enrollment = await getControllerEnrollmentStatus()
      if (generation === sessionGeneration && username.value && (enrollment?.revoked || enrollment?.account_suspended
        || (enrollment?.registered && (enrollment.device_login_revision ?? 0) > (rememberedRevision ?? 0)))) {
        await clearSession()
      }
      const selected = selectedNetworkId.value
      if (connection.value === 'connecting' && selected && hasAndroidVpnAttachment(selected)) {
        connection.value = 'connected'
        connectDeadline = 0
      } else if (connection.value === 'connecting' && Date.now() >= connectDeadline) {
        error.value = '连接超时，请重新打开开关。'
        await stopConnection()
      } else if (connection.value === 'connected' && selected && !hasAndroidVpnAttachment(selected)) {
        connection.value = 'connecting'
        connectDeadline = Date.now() + 45_000
        void syncMobileVpnService().catch(() => {})
      }
      if (Date.now() >= nextSessionUpdate) {
        nextSessionUpdate = Date.now() + (authorized.value ? 5_000 : 750)
        void refreshNetworks()
        void refreshDevices()
      }
    } catch {
      // A transient IPC error does not withdraw the authenticated account.
    } finally {
      checking = false
    }
  }

  async function retryStartup() {
    if (busy.value || ready.value) return
    busy.value = true
    error.value = ''
    try {
      if (!accountResolved.value && !username.value) {
        // Present the last confirmed account immediately. Only useAccount,
        // reached with an online reply, grants access to the live operations.
        const cached = await invoke<AccessAccount | null>('android_access_restore').catch(() => null)
        if (cached) {
          rememberedRevision = cached.device_login_revision
          username.value = cached.username
          networks.value = cached.networks
          selectedNetworkId.value = cached.networks.length === 1 ? cached.networks[0].network_id
            : hasNetwork(cached.selected_network_id) ? cached.selected_network_id : null
        }
      }
      if (!await invoke<string | null>('managed_controller_url')) throw new Error('当前版本未配置控制台地址。')
      await bootstrapAndroidManagement()
      await initRpcConnection(true)
      unlisten ??= await listenGlobalEvents()
      if (!vpnListenersReady) {
        await initMobileVpnService()
        vpnListenersReady = true
      }
      await setAndroidVpnNetwork(null)
      await initWebClient()
      validationStarted = Date.now()
      ready.value = true
    } catch (reason) {
      error.value = reason instanceof Error && reason.message === '当前版本未配置控制台地址。'
        ? reason.message : '启动失败，请重试。'
    } finally {
      busy.value = false
    }
    if (ready.value) {
      await check()
      void consumePendingMobileVpnTileAction().catch(() => {})
    }
  }

  onMounted(() => {
    setMobileVpnErrorHandler(() => {
      error.value = '无法建立系统 VPN，请确认已授权后重试。'
      void stopConnection()
    })
    setMobileVpnTileActionHandler(action => action === 'stop' ? disconnect() : connect())
    timer = setInterval(() => { void check() }, 750)
    void retryStartup()
  })
  onUnmounted(() => {
    clearInterval(timer)
    resetDevices()
    unlisten?.()
    setMobileVpnErrorHandler()
    setMobileVpnTileActionHandler()
  })

  return {
    ready, accountResolved, authorized, username, networks, selectedNetworkId, connection, error, busy, captcha,
    devices, devicesLoading, devicesError,
    loadDeviceName: getManagedDeviceName, renameDevice,
    refreshDevices: () => refreshDevices(true), refreshCaptcha,
    signIn: (name: string, password: string) => authenticate(name, password),
    register: authenticate, selectNetwork, connect, disconnect, refreshNetworks, retryStartup,
  }
}
