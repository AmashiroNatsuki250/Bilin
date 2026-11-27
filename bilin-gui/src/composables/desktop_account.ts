import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { type } from '@tauri-apps/plugin-os'
import { computed, onUnmounted, ref, shallowRef, watch, type Ref } from 'vue'
import type { ControllerEnrollmentStatus } from './backend'

export const DESKTOP_ACCESS_EVENT = 'desktop-access-result'
export const DESKTOP_ACCESS_LABEL = 'desktop-access'

const STORAGE_KEY = 'bilin.desktop-account.v1'
const DEVICE_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
const NETWORK_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const PAIRING = /^[0-9a-f]{16}$/
const STATE = /^[0-9a-f]{32}$/
const HOSTED_ACCESS_MARKER = '/assets/desktop-access-ready-v1.png'

interface DesktopAccount {
  origin: string
  device: string
  revision: number
  account: string
}

type ReturnMode = 'windows' | 'windows-https' | 'macos' | 'dev'

interface Attempt {
  origin: string
  device: string
  revision: number
  state: string
  deadline: number
  setup: boolean
}

export interface DesktopAccessResult {
  state: string
  account: string
  network?: string
}

interface DesktopLoginWindow {
  window: WebviewWindow
  phase: 'opening' | 'visible' | 'closing'
  launch: Promise<void>
  displacedMain: boolean
  unlistenClose?: UnlistenFn
}

let desktopLoginWindow: DesktopLoginWindow | undefined

export function focusDesktopAccessWindow() {
  const access = desktopLoginWindow
  if (!access) return false
  if (access.phase === 'visible') {
    void (async () => {
      const minimized = await access.window.isMinimized()
      if (access.phase !== 'visible') return
      if (minimized) await access.window.unminimize()
      if (access.phase === 'visible') await access.window.setFocus()
    })().catch(console.warn)
  }
  return true
}

function hostedAccessAvailable(origin: string) {
  // Older controllers return their regular console HTML for unmatched paths.
  // Image decoding distinguishes that fallback without running remote script in the app.
  return new Promise<boolean>(resolve => {
    const image = new Image()
    const finish = (valid: boolean) => {
      clearTimeout(timeout)
      image.onload = image.onerror = null
      image.removeAttribute('src')
      resolve(valid)
    }
    const timeout = setTimeout(() => finish(false), 5000)
    const url = new URL(HOSTED_ACCESS_MARKER, origin)
    url.searchParams.set('check', String(Date.now()))
    image.referrerPolicy = 'no-referrer'
    image.onload = () => finish(image.naturalWidth === 1 && image.naturalHeight === 1)
    image.onerror = () => finish(false)
    image.src = url.href
  })
}

function controllerOrigin(input: string | null | undefined) {
  try {
    const url = new URL(input || '')
    if (url.protocol === 'https:' && url.pathname === '/' && !url.username && !url.password && !url.search && !url.hash)
      return url.origin
  } catch { /* No usable configured controller. */ }
  return null
}

function returnMode(url: Location): ReturnMode | null {
  if (url.hostname === 'tauri.localhost' && !url.port) {
    if (url.protocol === 'https:') return 'windows-https'
    if (url.protocol === 'http:') return 'windows'
  }
  if (url.protocol === 'tauri:' && url.hostname === 'localhost' && !url.port) return 'macos'
  if (url.protocol === 'http:' && url.hostname === 'localhost' && url.port === '1420') return 'dev'
  return null
}

function readAccount(): DesktopAccount | null {
  try {
    const account = JSON.parse(localStorage.getItem(STORAGE_KEY) || 'null') as DesktopAccount | null
    if (account && controllerOrigin(account.origin) === account.origin && DEVICE_ID.test(account.device)
      && Number.isSafeInteger(account.revision) && account.revision > 0 && validAccountName(account.account)) return account
  } catch { /* Login can proceed when saved presentation data is unavailable. */ }
  return null
}

function validAccountName(value: unknown): value is string {
  return typeof value === 'string' && value.trim() === value && value.length > 0 && value.length <= 256 && !/\p{Cc}/u.test(value)
}

function saveAccount(account: DesktopAccount | null) {
  try {
    if (account) localStorage.setItem(STORAGE_KEY, JSON.stringify(account))
    else localStorage.removeItem(STORAGE_KEY)
  } catch { /* Authorization is still available for this run. */ }
}

export function readDesktopAccessResult(query: URLSearchParams): DesktopAccessResult | null {
  const state = query.get('desktop-access-result')
  const account = query.get('account')
  if (!state || !STATE.test(state) || !validAccountName(account)) return null
  const network = query.get('network')
  return network && NETWORK_ID.test(network) ? { state, account, network } : { state, account }
}

export function useDesktopAccount(
  enabled: boolean,
  controller: Ref<string | null>,
  initialized: Ref<boolean>,
  enrollment: Ref<ControllerEnrollmentStatus | null>,
) {
  const saved = shallowRef<DesktopAccount | null>(enabled ? readAccount() : null)
  const verified = ref(false)
  const opening = ref(false)
  const error = ref('')
  const desktopNetworkSetupBusy = ref(false)
  const desktopNetworkSetupError = ref('')
  const pendingNetwork = ref<string>()
  const attempt = shallowRef<Attempt | null>(null)
  let submitted: DesktopAccessResult | null = null
  let networkSyncTimeout: ReturnType<typeof setTimeout> | undefined
  let closing: Promise<void> | undefined
  let listenPromise: Promise<UnlistenFn> | undefined
  let mounted = true
  let automaticallyOpened = false

  const managed = computed(() => enabled && !!(controller.value || (!initialized.value && saved.value)))
  const showDesktopStartup = computed(() => enabled && !initialized.value && !saved.value)
  const showDesktopAccount = computed(() => managed.value && initialized.value && !saved.value)
  const canOpenDesktopAccount = computed(() => {
    const current = enrollment.value
    return !!(showDesktopAccount.value && controllerOrigin(controller.value) && current?.registered
      && current.device_login_revision && !current.revoked && !current.account_suspended
      && (current.bound || PAIRING.test(current.pairing_code ?? '')))
  })
  const canOpenNetworkSetup = computed(() => {
    const current = enrollment.value
    return !!(enabled && verified.value && saved.value && controllerOrigin(controller.value)
      && current?.registered && current.bound && current.device_login_revision
      && !current.revoked && !current.account_suspended)
  })

  function clearAttempt() {
    submitted = null
    attempt.value = null
  }

  function finishNetworkSync() {
    clearTimeout(networkSyncTimeout)
    pendingNetwork.value = undefined
  }

  function rejectAccount(message = '') {
    const hadSaved = !!saved.value
    saved.value = null
    verified.value = false
    clearAttempt()
    finishNetworkSync()
    error.value = message
    desktopNetworkSetupError.value = ''
    if (hadSaved) saveAccount(null)
  }

  async function restoreMainWindow() {
    const main = getCurrentWindow()
    if (type() === 'macos') await invoke('set_dock_visibility', { visible: true }).catch(console.warn)
    if (await main.isMinimized()) await main.unminimize()
    await main.show()
    await main.setFocus()
  }

  function closeAccessWindow() {
    if (closing) return closing
    const access = desktopLoginWindow
    if (!access) return Promise.resolve()
    access.phase = 'closing'
    closing = (async () => {
      await access.launch.catch(() => {})
      await Promise.resolve(access.unlistenClose?.()).catch(console.warn)
      // The platform may hide the window itself when the user closes it.
      // Explicitly hide it before restoring main so ordering does not depend on the platform.
      const hidden = await access.window.hide().then(() => true, () => false)
      const destroyed = await access.window.destroy().then(() => true, () => false)
      if (!hidden && !destroyed) {
        const remaining = await WebviewWindow.getByLabel(DESKTOP_ACCESS_LABEL)
        if (remaining) await remaining.hide()
      }
      if (mounted && access.displacedMain) await restoreMainWindow()
    })().finally(() => {
      if (desktopLoginWindow === access) desktopLoginWindow = undefined
      closing = undefined
    })
    return closing
  }

  function reconcile(current: ControllerEnrollmentStatus | null) {
    if (!enabled) return
    const origin = controllerOrigin(controller.value)
    if (initialized.value && !controller.value && saved.value) rejectAccount()
    if (!current) return
    if (current.revoked || current.account_suspended) {
      rejectAccount(current.account_suspended ? '账号已被停用。' : '设备绑定已被移除，请重新登录。')
      void closeAccessWindow().catch(console.warn)
      return
    }
    if (pendingNetwork.value && current.networks.some(network => network.network_id === pendingNetwork.value)) {
      finishNetworkSync()
    }
    const cached = saved.value
    if (cached && (origin !== cached.origin || current.device_id !== cached.device
      || (current.registered && (!current.bound || current.bound_account !== cached.account
        || current.device_login_revision !== cached.revision)))) {
      rejectAccount('设备绑定已变化，请重新登录。')
      void closeAccessWindow().catch(console.warn)
    }
    if (!current.registered || !current.bound || !current.device_login_revision || !origin) return
    const active = saved.value
    if (active) {
      verified.value = active.origin === origin && active.device === current.device_id
        && active.revision === current.device_login_revision && active.account === current.bound_account
      if (!verified.value || !attempt.value?.setup) return
    }
    const pending = attempt.value
    if (!pending || !submitted || pending.state !== submitted.state) return
    if (Date.now() > pending.deadline || pending.origin !== origin || pending.device !== current.device_id
      || pending.revision !== current.device_login_revision || current.bound_account !== submitted.account) {
      const message = '设备绑定状态已变化，请重新打开登录窗口。'
      if (pending.setup) {
        clearAttempt()
        desktopNetworkSetupError.value = message
      } else {
        rejectAccount(message)
      }
      void closeAccessWindow().catch(console.warn)
      return
    }
    if (!active) {
      const account = { origin, device: current.device_id, revision: current.device_login_revision, account: submitted.account }
      saveAccount(account)
      saved.value = account
      verified.value = true
    }
    const joinedNetwork = submitted.network
    if (joinedNetwork && NETWORK_ID.test(joinedNetwork) && !current.networks.some(network => network.network_id === joinedNetwork)) {
      finishNetworkSync()
      pendingNetwork.value = joinedNetwork
      networkSyncTimeout = setTimeout(finishNetworkSync, 15000)
    }
    clearAttempt()
    error.value = ''
    desktopNetworkSetupError.value = ''
    void closeAccessWindow().catch(console.warn)
  }

  function ensureResultListener() {
    listenPromise ??= getCurrentWindow().listen<DesktopAccessResult>(DESKTOP_ACCESS_EVENT, ({ payload }) => {
      if (!mounted || !attempt.value || payload?.state !== attempt.value.state || !validAccountName(payload.account)) return
      submitted = payload
      if (!attempt.value.setup) error.value = '正在确认本机绑定状态…'
      reconcile(enrollment.value)
    }).catch((cause) => {
      listenPromise = undefined
      throw cause
    })
    return listenPromise
  }

  async function openAccountWindow(setup: boolean) {
    const canOpen = setup ? canOpenNetworkSetup : canOpenDesktopAccount
    const busy = setup ? desktopNetworkSetupBusy : opening
    const launchError = setup ? desktopNetworkSetupError : error
    if (focusDesktopAccessWindow() || opening.value || desktopNetworkSetupBusy.value || !canOpen.value) return
    const origin = controllerOrigin(controller.value)
    const destination = returnMode(window.location)
    if (!origin || !destination) {
      launchError.value = '控制台地址或本机登记信息无效。'
      return
    }
    busy.value = true
    launchError.value = ''
    try {
      await ensureResultListener()
      const available = await hostedAccessAvailable(origin)
      if (!mounted || !canOpen.value || controllerOrigin(controller.value) !== origin) return
      if (!available) {
        launchError.value = setup ? '网络设置暂时无法使用，请稍后重试。' : '登录页面暂时无法使用，请稍后重试。'
        return
      }
      const current = enrollment.value
      if (!current?.device_login_revision || !DEVICE_ID.test(current.device_id)
        || controllerOrigin(current.controller_origin) !== origin) {
        launchError.value = '控制台地址或本机登记信息无效。'
        return
      }
      const previous = await WebviewWindow.getByLabel(DESKTOP_ACCESS_LABEL)
      await previous?.hide()
      await previous?.destroy()
      if (!mounted || !canOpen.value || controllerOrigin(controller.value) !== origin) return
      const pairing = current.bound ? null : current.pairing_code ?? null
      const state = Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, '0')).join('')
      const params = new URLSearchParams({ device: current.device_id, state, return: destination })
      if (current.bound) params.set('bound', '1')
      else if (pairing) params.set('pairing', pairing)
      if (setup) params.set('setup', '1')
      attempt.value = { origin, device: current.device_id, revision: current.device_login_revision, state, deadline: Date.now() + 10 * 60_000, setup }
      submitted = null
      const url = new URL('/', origin)
      url.hash = `/desktop-access?${params}`
      const authWindow = new WebviewWindow(DESKTOP_ACCESS_LABEL, {
        url: url.toString(), title: '比邻', width: 420, height: 590, minWidth: 360, minHeight: 520,
        center: true, visible: false, focus: false, resizable: true, maximizable: false,
        useHttpsScheme: destination === 'windows-https',
      })
      const access: DesktopLoginWindow = {
        window: authWindow, phase: 'opening', launch: Promise.resolve(), displacedMain: false,
      }
      desktopLoginWindow = access
      access.launch = (async () => {
        await new Promise<void>((resolve, reject) => {
          void authWindow.once('tauri://created', () => resolve()).catch(reject)
          void authWindow.once('tauri://error', ({ payload }) => reject(payload)).catch(reject)
        })
        if (access.phase === 'closing') return
        access.unlistenClose = await authWindow.onCloseRequested(event => {
          event.preventDefault()
          if (access.phase === 'closing') return
          if (setup) clearAttempt()
          else rejectAccount()
          void closeAccessWindow().catch(console.warn)
        })
        if (access.phase !== 'opening') return
        access.displacedMain = true
        await getCurrentWindow().hide()
        if (access.phase !== 'opening') return
        await authWindow.show()
        if (access.phase !== 'opening') return
        access.phase = 'visible'
        await authWindow.setFocus()
      })()
      await access.launch
    } catch {
      if (mounted && setup) {
        clearAttempt()
        launchError.value = '无法打开网络设置，请重试。'
      } else if (mounted && showDesktopAccount.value) {
        rejectAccount('无法打开登录窗口，请重试。')
      }
      await closeAccessWindow().catch(console.warn)
    } finally {
      busy.value = false
    }
  }

  watch([controller, initialized, enrollment], ([, , current]) => reconcile(current), { immediate: true })
  watch(canOpenDesktopAccount, (canOpen) => {
    if (!canOpen || automaticallyOpened || error.value) return
    automaticallyOpened = true
    void openAccountWindow(false)
  })
  onUnmounted(() => {
    mounted = false
    void listenPromise?.then(unlisten => unlisten()).catch(() => {})
    finishNetworkSync()
    if (enabled) void closeAccessWindow().catch(console.warn)
  })

  return { managed, verified, showDesktopStartup, showDesktopAccount, canOpenDesktopAccount,
    desktopAccountOpening: opening, desktopAccountError: error, openDesktopAccount: () => openAccountWindow(false),
    desktopNetworkSyncing: computed(() => !!pendingNetwork.value),
    desktopNetworkSetupBusy, desktopNetworkSetupError, openDesktopNetworkSetup: () => openAccountWindow(true) }
}
