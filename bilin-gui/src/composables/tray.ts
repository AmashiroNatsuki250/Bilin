import { invoke } from '@tauri-apps/api/core'
import { Menu, MenuItem, PredefinedMenuItem } from '@tauri-apps/api/menu'
import { TrayIcon } from '@tauri-apps/api/tray'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { type } from '@tauri-apps/plugin-os'
import pkg from '~/../package.json'
import inactiveIcon from '../../src-tauri/icons/icon-inactive.ico?url'
import activeIcon from '../../src-tauri/icons/icon.ico?url'
import { focusDesktopAccessWindow } from './desktop_account'
import { focusDesktopMainWindow, hasDesktopTrayPanel, hideDesktopTray, toggleDesktopTray } from './desktop_tray'

const DEFAULT_TRAY_NAME = 'main'
const DESKTOP_TRAY_NAME = 'bilin-desktop-tray'
let desktopTrayRequest: Promise<TrayIcon | null> | undefined

interface TrayMenuLabels {
  show: string
  quit: string
}

function usesUserTray() {
  return invoke<boolean>('uses_user_tray')
}

async function toggleVisibility() {
  if (focusDesktopAccessWindow()) return
  if (await getCurrentWindow().isVisible()) {
    await getCurrentWindow().hide()
  }
  else {
    if (focusDesktopAccessWindow()) return
    await getCurrentWindow().show()
    await getCurrentWindow().setFocus()
  }
}

async function toggleMainWindow() {
  await hideDesktopTray().catch(console.warn)
  if (focusDesktopAccessWindow()) return
  const main = getCurrentWindow()
  if (await main.isVisible() && !await main.isMinimized() && await main.isFocused()) {
    await main.hide()
    if (type() === 'macos')
      await invoke('set_dock_visibility', { visible: false })
  }
  else {
    await focusDesktopMainWindow()
  }
}

async function readTrayIcon(url: string) {
  const response = await fetch(url)
  if (!response.ok)
    throw new Error('Could not load the desktop tray icon')
  return response.arrayBuffer()
}

async function createDesktopTray() {
  try {
    const current = await TrayIcon.getById(DESKTOP_TRAY_NAME)
    if (current)
      return current

    // The app creates a native tray at startup. Install the new callback before
    // removing it so a failed creation keeps the original entry accessible.
    const tray = await TrayIcon.new({
      id: DESKTOP_TRAY_NAME,
      icon: await readTrayIcon(activeIcon),
      iconAsTemplate: type() === 'macos',
      tooltip: `Bilin\n${pkg.version}`,
      showMenuOnLeftClick: false,
      action: (event) => {
        if (event.type !== 'Click' || event.buttonState !== 'Up')
          return
        if (event.button === 'Right')
          void toggleDesktopTray(event).catch(console.warn)
        if (event.button === 'Left')
          void toggleMainWindow().catch(console.warn)
      },
    })
    try {
      if (await TrayIcon.getById(DEFAULT_TRAY_NAME))
        await TrayIcon.removeById(DEFAULT_TRAY_NAME)
    }
    catch (error) {
      await TrayIcon.removeById(DESKTOP_TRAY_NAME).catch(console.warn)
      throw error
    }
    return tray
  }
  catch (error) {
    console.warn('Error while creating desktop tray panel entry:', error)
    return null
  }
}

export async function useTray(init: boolean = false) {
  let tray
  try {
    // The Linux user-session process owns the tray; never recreate it as root.
    if (await usesUserTray())
      return null

    if (hasDesktopTrayPanel()) {
      desktopTrayRequest ??= createDesktopTray()
      const desktopTray = await desktopTrayRequest
      if (desktopTray)
        return desktopTray
    }

    tray = await TrayIcon.getById(DEFAULT_TRAY_NAME)
    if (!tray) {
      tray = await TrayIcon.new({
        tooltip: `Bilin\n${pkg.version}`,
        title: `Bilin\n${pkg.version}`,
        id: DEFAULT_TRAY_NAME,
        menu: await Menu.new({
          id: 'main',
          items: await generateMenuItem(),
        }),
        action: async () => {
          toggleVisibility()
        },
      })
    }
  }
  catch (error) {
    console.warn('Error while creating tray icon:', error)
    return null
  }

  if (init) {
    tray.setTooltip(`Bilin\n${pkg.version}`)
    tray.setMenuOnLeftClick(false)
    tray.setMenu(await Menu.new({
      id: 'main',
      items: await generateMenuItem(),
    }))
  }

  return tray
}

export async function generateMenuItem() {
  return [
    await MenuItemShow('Show / Hide'),
    await PredefinedMenuItem.new({ item: 'Separator' }),
    await MenuItemExit('Exit'),
  ]
}

export async function MenuItemExit(text: string) {
  return await PredefinedMenuItem.new({
    text,
    item: 'Quit',
  })
}

export async function MenuItemShow(text: string) {
  return await MenuItem.new({
    id: 'show',
    text,
    action: async () => {
      await toggleVisibility()
    },
  })
}

export async function setTrayMenu(labels?: TrayMenuLabels) {
  if (await usesUserTray()) {
    await invoke('update_user_tray', {
      update: { type: 'menu', ...(labels ?? { show: 'Show / Hide', quit: 'Exit' }) },
    })
    return
  }
  const tray = await useTray()
  if (!tray)
    return
  if (tray.id === DESKTOP_TRAY_NAME)
    return
  const menu = await Menu.new({
    id: 'main',
    items: labels
      ? [await MenuItemShow(labels.show), await MenuItemExit(labels.quit)]
      : await generateMenuItem(),
  })
  tray.setMenu(menu)
}

export async function setTrayRunState(isRunning: boolean = false) {
  if (await usesUserTray()) {
    await invoke('update_user_tray', { update: { type: 'run_state', running: isRunning } })
    return
  }
  const tray = await useTray()
  if (!tray)
    return
  if (tray.id === DESKTOP_TRAY_NAME) {
    await tray.setIcon(await readTrayIcon(isRunning ? inactiveIcon : activeIcon))
    return
  }
  tray.setIcon(isRunning ? 'icons/icon-inactive.ico' : 'icons/icon.ico')
}

export async function setTrayTooltip(tooltip: string) {
  if (tooltip) {
    const text = `Bilin\n${pkg.version}\n${tooltip}`
    if (await usesUserTray()) {
      await invoke('update_user_tray', { update: { type: 'tooltip', tooltip: text } })
      return
    }
    const tray = await useTray()
    if (!tray)
      return
    tray.setTooltip(text)
    if (tray.id !== DESKTOP_TRAY_NAME)
      tray.setTitle(text)
  }
}
