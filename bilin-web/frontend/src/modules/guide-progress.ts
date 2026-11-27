import type { CentralNetworkSummary, ControllerDevice } from './api'

export type GuideStep = 0 | 1 | 2 | 3 | 4

export type GuideProgress = {
    step: Exclude<GuideStep, 0>
    viewedStep?: GuideStep
    deviceCode?: string
    networkId?: string
    networkName?: string
}

function storageKey(username: string) {
    return `bilin.guide-progress.v1:${username}`
}

export function loadGuideProgress(
    username: string,
    devices: readonly ControllerDevice[],
    networks: readonly CentralNetworkSummary[],
): GuideProgress | null {
    try {
        const stored = window.localStorage.getItem(storageKey(username))
        if (!stored) return null
        const entry: unknown = JSON.parse(stored)
        if (!entry || typeof entry !== 'object') return null
        const { step, viewedStep, deviceCode, networkId } = entry as Partial<GuideProgress>
        if (step !== 1 && step !== 2 && step !== 3 && step !== 4) return null

        const progress: GuideProgress = { step }
        if (step >= 3) {
            const device = devices.find(item => item.device_code === deviceCode)
            if (device) progress.deviceCode = device.device_code
            else progress.step = 2
        }
        if (progress.step === 4) {
            const network = networks.find(item => item.network_id === networkId)
            if (network) {
                progress.networkId = network.network_id
                progress.networkName = network.display_name
            } else {
                progress.step = 3
            }
        }
        if (typeof viewedStep === 'number' && Number.isInteger(viewedStep)) {
            progress.viewedStep = Math.min(Math.max(0, viewedStep), progress.step) as GuideStep
        }
        return progress
    } catch {
        return null
    }
}

export function writeGuideProgress(username: string, progress: GuideProgress | null) {
    try {
        const key = storageKey(username)
        if (progress === null) {
            window.localStorage.removeItem(key)
            return
        }
        const { step, viewedStep, deviceCode, networkId } = progress
        window.localStorage.setItem(key, JSON.stringify({ step, viewedStep, deviceCode, networkId }))
    } catch {
        // The wizard remains usable when browser storage is unavailable.
    }
}
