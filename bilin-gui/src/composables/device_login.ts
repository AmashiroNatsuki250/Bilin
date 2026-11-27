import type { ControllerEnrollmentStatus } from './backend'

export interface DeviceLoginAttempt {
  revision: number
  bound: boolean
}

export function deviceLoginStorageKey(status: ControllerEnrollmentStatus) {
  return `bilin.device-login:${status.controller_origin}:${status.device_id}`
}

export function loadDeviceLoginAttempt(key: string): DeviceLoginAttempt | null {
  try {
    const saved = JSON.parse(localStorage.getItem(key) ?? 'null') as Partial<DeviceLoginAttempt> | null
    if (!saved || typeof saved.revision !== 'number' || !Number.isSafeInteger(saved.revision)
      || saved.revision <= 0 || typeof saved.bound !== 'boolean') return null
    return { revision: saved.revision, bound: saved.bound }
  } catch {
    return null
  }
}

export function saveDeviceLoginAttempt(key: string, attempt: DeviceLoginAttempt | null) {
  try {
    if (attempt) localStorage.setItem(key, JSON.stringify(attempt))
    else localStorage.removeItem(key)
  } catch {
    // The current login attempt still works if the WebView cannot persist its preferences.
  }
}

export function updateDeviceLoginAttempt(
  attempt: DeviceLoginAttempt | null,
  status: Pick<ControllerEnrollmentStatus, 'registered' | 'bound' | 'revoked' | 'device_login_revision'>,
): DeviceLoginAttempt | null {
  if (!attempt) return null
  if (status.revoked) return null
  if (!status.registered) return attempt
  const revision = status.device_login_revision ?? attempt.revision
  if (status.bound) return attempt.bound && revision <= attempt.revision
    ? attempt
    : { revision: Math.max(attempt.revision, revision), bound: true }
  if (attempt.bound || revision > attempt.revision) return null
  return attempt
}
