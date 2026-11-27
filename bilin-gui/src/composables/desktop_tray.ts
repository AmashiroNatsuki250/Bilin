import type { TrayIconEvent } from '@tauri-apps/api/tray'
import { invoke } from '@tauri-apps/api/core'
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi'
import { emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { currentMonitor, getCurrentWindow, monitorFromPoint } from '@tauri-apps/api/window'
import { type } from '@tauri-apps/plugin-os'
import { onMounted, onUnmounted, watch } from 'vue'
import { focusDesktopAccessWindow } from './desktop_account'

export const DESKTOP_TRAY_WINDOW = 'desktop-tray'
export const DESKTOP_TRAY_STATE = 'bilin-desktop-tray-state'
export const DESKTOP_TRAY_ACTION = 'bilin-desktop-tray-action'

const MAIN_WINDOW = 'main'
const WIDTH = 378
const HEIGHT = 464
const COMPACT_WIDTH = 228
const COMPACT_HEIGHT = 152

export interface DesktopTrayPeer {
  id: string
  name: string
  address?: string
  online: boolean
  kind?: 'desktop' | 'mobile' | 'server'
}

export interface DesktopTrayState {
  enabled: boolean
  networkName?: string
  address?: string
  connected: boolean
  connectionEnabled?: boolean
  busy?: boolean
  canToggle?: boolean
  webConsoleAvailable?: boolean
  peers: DesktopTrayPeer[]
}

export type DesktopTrayAction =
  | { type: 'open-main' | 'settings' | 'toggle-connection' | 'web-console' | 'quit' }
  | { type: 'open-peer', peerId: string }
export type DesktopTrayEvent = DesktopTrayAction | { type: 'ready' } | { type: 'dismiss' }

let readState: (() => DesktopTrayState) | undefined
let panel: WebviewWindow | undefined
let panelAnchor: TrayIconEvent | undefined
let changingPanel = false

export function hasDesktopTrayPanel() {
  const platform = type()
  return platform === 'windows' || platform === 'macos'
}

export async function focusDesktopMainWindow() {
  if (focusDesktopAccessWindow()) return
  const main = getCurrentWindow()
  if (type() === 'macos')
    await invoke('set_dock_visibility', { visible: true })
  const minimized = await main.isMinimized()
  if (focusDesktopAccessWindow()) return
  if (minimized)
    await main.unminimize()
  if (focusDesktopAccessWindow()) return
  await main.show()
  if (!focusDesktopAccessWindow()) await main.setFocus()
}

export async function hideDesktopTray() {
  await panel?.hide()
}

async function publishState() {
  if (!panel || !readState)
    return
  await emitTo(DESKTOP_TRAY_WINDOW, DESKTOP_TRAY_STATE, readState())
}

async function createPanel() {
  const existing = await WebviewWindow.getByLabel(DESKTOP_TRAY_WINDOW)
  const signedIn = readState?.().enabled
  const window = existing ?? new WebviewWindow(DESKTOP_TRAY_WINDOW, {
    url: 'index.html?desktop-tray=1',
    title: '比邻快捷面板',
    width: signedIn ? WIDTH : COMPACT_WIDTH,
    height: signedIn ? HEIGHT : COMPACT_HEIGHT,
    visible: false,
    focus: false,
    decorations: false,
    resizable: false,
    closable: false,
    alwaysOnTop: true,
    shadow: true,
    skipTaskbar: true,
    acceptFirstMouse: true,
  })
  if (!existing) {
    await new Promise<void>((resolve, reject) => {
      void window.once('tauri://created', () => resolve()).catch(reject)
      void window.once('tauri://error', ({ payload }) => reject(payload)).catch(reject)
    })
  }
  let hadFocus = false
  await window.onFocusChanged(({ payload: focused }) => {
    if (!focused && hadFocus)
      void window.hide().catch(console.warn)
    hadFocus = focused
  })
  return window
}

async function placePanel(window: WebviewWindow, event: TrayIconEvent) {
  const monitor = await monitorFromPoint(event.position.x, event.position.y) ?? await currentMonitor()
  if (!monitor)
    return

  const { position, size } = monitor.workArea
  const scale = monitor.scaleFactor
  const margin = Math.round(8 * scale)
  const signedIn = readState?.().enabled
  const width = Math.max(1, Math.min(Math.round((signedIn ? WIDTH : COMPACT_WIDTH) * scale), size.width - 2 * margin))
  const height = Math.max(1, Math.min(Math.round((signedIn ? HEIGHT : COMPACT_HEIGHT) * scale), size.height - 2 * margin))
  const { position: anchor, size: anchorSize } = event.rect
  const right = position.x + size.width
  const bottom = position.y + size.height
  const clamp = (value: number, minimum: number, maximum: number) => Math.max(minimum, Math.min(maximum, value))
  const x = clamp(anchor.x + (anchorSize.width - width) / 2, position.x + margin, right - width - margin)
  const below = anchor.y + anchorSize.height + margin
  const above = anchor.y - height - margin
  const y = clamp(below + height <= bottom - margin ? below : above, position.y + margin, bottom - height - margin)

  await window.setSize(new PhysicalSize(width, height))
  await window.setPosition(new PhysicalPosition(Math.round(x), Math.round(y)))
}

export async function toggleDesktopTray(event: TrayIconEvent) {
  if (changingPanel)
    return

  changingPanel = true
  try {
    panel ??= await createPanel()
    if (await panel.isVisible()) {
      await hideDesktopTray()
      return
    }
    panelAnchor = event
    await placePanel(panel, event)
    await publishState()
    await panel.show()
    await panel.setFocus()
  }
  catch (error) {
    console.warn('Could not open desktop tray panel:', error)
    await hideDesktopTray().catch(console.warn)
    if (readState?.().enabled) await focusDesktopMainWindow()
  }
  finally {
    changingPanel = false
  }
}

export function useDesktopTrayBridge(
  state: () => DesktopTrayState,
  onAction: (action: DesktopTrayAction) => void | Promise<void>,
) {
  if (!hasDesktopTrayPanel())
    return
  readState = state

  let stopListening: UnlistenFn | undefined
  let disposed = false

  onMounted(async () => {
    try {
      const stop = await listen<DesktopTrayEvent>(DESKTOP_TRAY_ACTION, async ({ payload }) => {
        if (payload.type === 'ready') {
          await publishState().catch(console.warn)
          return
        }
        if (payload.type === 'dismiss') {
          await hideDesktopTray().catch(console.warn)
          return
        }
        if (!state().enabled && payload.type !== 'open-main' && payload.type !== 'quit')
          return
        try {
          if (payload.type !== 'toggle-connection')
            await hideDesktopTray()
          await onAction(payload)
        }
        catch (error) {
          console.warn('Desktop tray action failed:', error)
        }
      })
      if (disposed)
        stop()
      else stopListening = stop
    }
    catch (error) {
      console.warn('Could not listen for desktop tray actions:', error)
    }
  })

  watch(() => JSON.stringify(state()), () => {
    void publishState().catch(console.warn)
  })
  watch(() => state().enabled, async () => {
    try {
      if (panel && panelAnchor && await panel.isVisible())
        await placePanel(panel, panelAnchor)
    }
    catch (error) {
      console.warn('Could not resize desktop tray panel:', error)
    }
  })

  onUnmounted(() => {
    disposed = true
    if (readState === state)
      readState = undefined
    stopListening?.()
    void hideDesktopTray().catch(console.warn)
  })
}

export function sendDesktopTrayEvent(event: DesktopTrayEvent) {
  return emitTo(MAIN_WINDOW, DESKTOP_TRAY_ACTION, event)
}
